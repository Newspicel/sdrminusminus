use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_modem::{
    cpm::{CpmMod, CpmParams, Mapping},
    pulse::{self, Norm},
};

use super::{shift, silence};

const TAIL_S: f64 = 0.7;
const LEAD_IN_S: f64 = 0.05;
const CARRIER_OFFSET_HZ: f64 = 30_000.0;

#[derive(Clone, Debug)]
pub struct Pwm {
    pub bits: Vec<bool>,
    pub short_us: u32,
    pub long_multiple: u32,
    pub sync_gap_multiple: u32,
    pub repeats: u32,
}

#[must_use]
pub fn pwm_timings(frame: &Pwm) -> Vec<u32> {
    let short = frame.short_us;
    let long = short * frame.long_multiple;
    let mut out = Vec::with_capacity(frame.bits.len() * 2 + 2);
    for &bit in &frame.bits {
        if bit {
            out.extend_from_slice(&[long, short]);
        } else {
            out.extend_from_slice(&[short, long]);
        }
    }
    out.push(short);
    out.push(short * frame.sync_gap_multiple);
    out
}

#[derive(Clone, Debug)]
pub struct Ppm {
    pub bits: Vec<bool>,
    pub pulse_us: u32,
    pub short_gap_us: u32,
    pub long_gap_us: u32,
    pub sync_gap_us: u32,
    pub repeats: u32,
}

#[must_use]
pub fn ppm_timings(frame: &Ppm) -> Vec<u32> {
    let mut out = Vec::with_capacity(frame.bits.len() * 2 * frame.repeats.max(1) as usize);
    for _ in 0..frame.repeats.max(1) {
        for &bit in &frame.bits {
            out.push(frame.pulse_us);
            out.push(if bit {
                frame.long_gap_us
            } else {
                frame.short_gap_us
            });
        }
        out.push(frame.pulse_us);
        out.push(frame.sync_gap_us);
    }
    out
}

#[derive(Clone, Debug)]
pub struct PwmBurst {
    pub bits: Vec<bool>,
    pub short_us: u32,
    pub long_us: u32,
    pub preamble_us: u32,
    pub preamble_pulses: u32,
    pub repeats: u32,
}

#[must_use]
pub fn pwm_burst_timings(frame: &PwmBurst) -> Vec<u32> {
    let mut out = Vec::new();
    for _ in 0..frame.repeats.max(1) {
        for _ in 0..frame.preamble_pulses {
            out.push(frame.preamble_us);
            out.push(frame.preamble_us);
        }
        for &bit in &frame.bits {
            let (pulse, gap) = if bit {
                (frame.long_us, frame.short_us)
            } else {
                (frame.short_us, frame.long_us)
            };
            out.push(pulse);
            out.push(gap);
        }
    }
    out
}

#[must_use]
pub fn manchester_cells(bits: &[bool]) -> Vec<bool> {
    bits.iter().flat_map(|&b| [b, !b]).collect()
}

#[must_use]
pub fn manchester_timings(bits: &[bool], half_cell_us: u32) -> Vec<u32> {
    let cells = manchester_cells(bits);
    let mut out = Vec::new();
    let mut level = true;
    let mut run = 0u32;
    for &cell in &cells {
        if cell == level {
            run += half_cell_us;
        } else {
            out.push(run);
            level = cell;
            run = half_cell_us;
        }
    }
    out.push(run);
    if !cells.first().copied().unwrap_or(true) && out.len() >= 2 {
        out.drain(..2);
    }
    out
}

#[must_use]
pub fn envelope(timings_us: &[u32], rate: f64) -> Vec<f32> {
    let samples = |us: u32| (f64::from(us) * 1e-6 * rate).round() as usize;
    let mut out = vec![0.0; samples((LEAD_IN_S * 1e6) as u32)];
    let mut high = true;
    for &us in timings_us {
        out.extend(std::iter::repeat_n(
            if high { 1.0 } else { 0.0 },
            samples(us),
        ));
        high = !high;
    }
    out.extend(std::iter::repeat_n(0.0, samples((TAIL_S * 1e6) as u32)));
    out
}

#[must_use]
pub fn keyed(timings_us: &[u32], rate: f64) -> Vec<Complex<f32>> {
    let step = TAU * CARRIER_OFFSET_HZ / rate;
    envelope(timings_us, rate)
        .iter()
        .enumerate()
        .map(|(k, &env)| Complex::from_polar(env, (step * k as f64) as f32))
        .collect()
}

#[must_use]
pub fn pwm(frame: &Pwm, rate: f64) -> Vec<Complex<f32>> {
    let one = pwm_timings(frame);
    let mut timings = Vec::new();
    for _ in 0..frame.repeats.max(1) {
        timings.extend_from_slice(&one);
    }
    keyed(&timings, rate)
}

#[must_use]
pub fn manchester(bits: &[bool], half_cell_us: u32, repeats: u32, rate: f64) -> Vec<Complex<f32>> {
    let one = manchester_timings(bits, half_cell_us);
    let mut timings = Vec::new();
    for i in 0..repeats.max(1) {
        if i > 0 {
            timings.push(20_000);
        }
        timings.extend_from_slice(&one);
    }
    keyed(&timings, rate)
}

#[must_use]
pub fn fsk_nrz(bits: &[bool], bit_us: u32, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let baud = 1e6 / f64::from(bit_us);
    let sps = rate / baud;
    let symbols: Vec<u8> = bits.iter().map(|&bit| u8::from(bit)).collect();
    let mut modulator = CpmMod::new(CpmParams::from_deviation(
        Mapping::new(vec![-1.0, 1.0]),
        deviation_hz,
        baud,
        pulse::rect(sps, Norm::Area),
        sps,
    ));
    let mut burst = Vec::new();
    modulator.modulate(&symbols, &mut burst);
    shift(&mut burst, CARRIER_OFFSET_HZ, rate);
    let mut iq = silence((LEAD_IN_S * rate) as usize);
    iq.extend(burst);
    iq.extend(silence((TAIL_S * rate) as usize));
    iq
}

#[must_use]
pub fn pwm_fsk(frame: &Pwm, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let one = pwm_timings(frame);
    let mut symbols = Vec::new();
    for _ in 0..frame.repeats.max(1) {
        let mut high = true;
        for &us in &one {
            symbols.extend(std::iter::repeat_n(
                u8::from(high),
                (us / frame.short_us) as usize,
            ));
            high = !high;
        }
    }
    let base_baud = 1e6 / f64::from(frame.short_us);
    let sps = rate / base_baud;
    let mut modulator = CpmMod::new(CpmParams::from_deviation(
        Mapping::new(vec![-1.0, 1.0]),
        deviation_hz,
        base_baud,
        pulse::rect(sps, Norm::Area),
        sps,
    ));
    let mut burst = Vec::new();
    modulator.modulate(&symbols, &mut burst);
    shift(&mut burst, CARRIER_OFFSET_HZ, rate);
    let mut iq = silence((LEAD_IN_S * rate) as usize);
    iq.extend(burst);
    iq.extend(silence((TAIL_S * rate) as usize));
    iq
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Pwm {
        Pwm {
            bits: vec![true, false, true, true],
            short_us: 320,
            long_multiple: 3,
            sync_gap_multiple: 31,
            repeats: 2,
        }
    }

    #[test]
    fn a_bit_is_a_long_short_pair_the_right_way_round() {
        let timings = pwm_timings(&frame());
        assert_eq!(&timings[..8], &[960, 320, 320, 960, 960, 320, 960, 320]);
        assert_eq!(
            &timings[8..],
            &[320, 9_920],
            "the sync pulse and its gap close the frame"
        );
    }

    #[test]
    fn manchester_runs_are_one_or_two_half_cells() {
        let timings = manchester_timings(&[true, true, false, false, true], 250);
        assert!(timings.iter().all(|&d| d == 250 || d == 500), "{timings:?}");
        assert_eq!(timings, [250, 250, 250, 500, 250, 250, 500, 250]);
    }

    #[test]
    fn ook_keys_the_carrier_fully_off_between_pulses() {
        let iq = keyed(&[500, 500], 250_000.0);
        let lead = (LEAD_IN_S * 250_000.0) as usize;
        let pulse = iq[lead + 10].norm();
        let gap = iq[lead + (500.0 * 1e-6 * 250_000.0) as usize + 10].norm();
        assert!((pulse - 1.0).abs() < 1e-3, "pulse {pulse}");
        assert!(gap < 1e-6, "gap {gap}");
    }

    #[test]
    fn fsk_holds_the_carrier_through_the_frame_and_drops_it_after() {
        let iq = pwm_fsk(&frame(), 40_000.0, 250_000.0);
        let lead = (LEAD_IN_S * 250_000.0) as usize;
        assert!((iq[lead + 100].norm() - 1.0).abs() < 1e-3);
        assert!(iq[iq.len() - 100].norm() < 1e-6);
    }
}
