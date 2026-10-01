use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_wire::{WefaxIoc, WefaxLpm};

use crate::wefax::{BLACK_HZ, PULSE_FRACTION, STOP_TONE_HZ, WHITE_HZ, level_to_hz};

const START_MS: f64 = 5_000.0;
const PHASING_MS: f64 = 30_000.0;
const STOP_MS: f64 = 5_000.0;
const BLACK_MS: f64 = 10_000.0;
const BAR_LEVELS: [u8; 8] = [255, 0, 220, 40, 180, 90, 140, 255];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chart {
    pub width: u16,
    pub height: u16,
    pub levels: Vec<u8>,
}

impl Chart {
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            levels: vec![0; usize::from(width) * usize::from(height)],
        }
    }

    #[must_use]
    pub fn pixel(&self, x: u16, y: u16) -> u8 {
        self.levels[usize::from(y) * usize::from(self.width) + usize::from(x)]
    }

    pub fn set(&mut self, x: u16, y: u16, level: u8) {
        self.levels[usize::from(y) * usize::from(self.width) + usize::from(x)] = level;
    }

    fn row(&self, y: u16) -> &[u8] {
        let width = usize::from(self.width);
        let base = usize::from(y) * width;
        &self.levels[base..base + width]
    }
}

#[must_use]
pub fn bars(ioc: WefaxIoc, lines: u16) -> Chart {
    let width = ioc.width();
    let mut chart = Chart::new(width, lines);
    for y in 0..lines {
        for x in 0..width {
            let bar = usize::from(x) * BAR_LEVELS.len() / usize::from(width);
            chart.set(x, y, BAR_LEVELS[bar.min(BAR_LEVELS.len() - 1)]);
        }
    }
    chart
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub start_ms: f64,
    pub phasing_lines: u16,
    pub stop_ms: f64,
    pub black_ms: f64,
}

impl Plan {
    #[must_use]
    pub fn standard(lpm: WefaxLpm) -> Self {
        Self {
            start_ms: START_MS,
            phasing_lines: (PHASING_MS / lpm.line_ms()).round() as u16,
            stop_ms: STOP_MS,
            black_ms: BLACK_MS,
        }
    }
}

struct Writer {
    rate: f64,
    phase: f64,
    carry: f64,
    out: Vec<Complex<f32>>,
}

impl Writer {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            phase: 0.0,
            carry: 0.0,
            out: Vec::new(),
        }
    }

    fn count(&mut self, ms: f64) -> usize {
        let total = ms * self.rate / 1_000.0 + self.carry;
        let count = total.round() as usize;
        self.carry = total - count as f64;
        count
    }

    fn emit(&mut self, hz: f64) {
        self.out.push(Complex::from_polar(1.0, self.phase as f32));
        self.phase = (self.phase + TAU * hz / self.rate).rem_euclid(TAU);
    }

    fn tone(&mut self, hz: f64, ms: f64) {
        for _ in 0..self.count(ms) {
            self.emit(hz);
        }
    }

    fn alternate(&mut self, rate_hz: f64, ms: f64) {
        for index in 0..self.count(ms) {
            let cycle = (index as f64 * rate_hz / self.rate).fract();
            self.emit(if cycle < 0.5 { WHITE_HZ } else { BLACK_HZ });
        }
    }

    fn sweep(&mut self, levels: &[u8], ms: f64) {
        let count = self.count(ms);
        for index in 0..count {
            let pixel = (index * levels.len() / count.max(1)).min(levels.len() - 1);
            self.emit(level_to_hz(levels[pixel]));
        }
    }

    fn phasing_line(&mut self, ms: f64) {
        let count = self.count(ms);
        let pulse = (count as f64 * PULSE_FRACTION).round() as usize;
        for index in 0..count {
            self.emit(if index < pulse { WHITE_HZ } else { BLACK_HZ });
        }
    }
}

#[must_use]
pub fn start_signal(ioc: WefaxIoc, ms: f64, rate: f64) -> Vec<Complex<f32>> {
    let mut writer = Writer::new(rate);
    writer.alternate(ioc.start_tone_hz(), ms);
    writer.out
}

#[must_use]
pub fn transmission(ioc: WefaxIoc, lpm: WefaxLpm, chart: &Chart, rate: f64) -> Vec<Complex<f32>> {
    transmission_with(ioc, lpm, chart, rate, &Plan::standard(lpm))
}

#[must_use]
pub fn transmission_with(
    ioc: WefaxIoc,
    lpm: WefaxLpm,
    chart: &Chart,
    rate: f64,
    plan: &Plan,
) -> Vec<Complex<f32>> {
    let line_ms = lpm.line_ms();
    let mut writer = Writer::new(rate);
    writer.alternate(ioc.start_tone_hz(), plan.start_ms);
    for _ in 0..plan.phasing_lines {
        writer.phasing_line(line_ms);
    }
    for y in 0..chart.height {
        writer.sweep(chart.row(y), line_ms);
    }
    writer.alternate(STOP_TONE_HZ, plan.stop_ms);
    writer.tone(BLACK_HZ, plan.black_ms);
    writer.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transmission_lasts_its_plan() {
        let rate = 12_000.0;
        let lpm = WefaxLpm::Lpm240;
        let chart = bars(WefaxIoc::Ioc576, 10);
        let plan = Plan {
            start_ms: 1_000.0,
            phasing_lines: 4,
            stop_ms: 1_000.0,
            black_ms: 500.0,
        };
        let iq = transmission_with(WefaxIoc::Ioc576, lpm, &chart, rate, &plan);
        let ms = 2_500.0 + 14.0 * lpm.line_ms();
        assert_eq!(iq.len(), (ms * rate / 1_000.0) as usize);
    }

    #[test]
    fn the_standard_plan_phases_for_thirty_seconds() {
        assert_eq!(Plan::standard(WefaxLpm::Lpm120).phasing_lines, 60);
        assert_eq!(Plan::standard(WefaxLpm::Lpm240).phasing_lines, 120);
    }
}
