use sdrmm_wire::{WefaxIoc, WefaxLpm};

use super::track::Track;
use crate::VideoPicture;

pub(super) const MAX_LINES: u16 = 4_000;
pub(super) const MAX_WIDTH: usize = 1_810;

pub(super) struct Picture {
    pub active: bool,
    pub ioc: WefaxIoc,
    pub lpm: WefaxLpm,
    pub line: u16,
    pub decoded: u16,
    pub started: u64,
    pub since_progress: u16,
    width: usize,
    origin: f64,
    period: f64,
    luma: Vec<u8>,
}

impl Picture {
    pub(super) fn empty() -> Self {
        Self {
            active: false,
            ioc: WefaxIoc::default(),
            lpm: WefaxLpm::default(),
            line: 0,
            decoded: 0,
            started: 0,
            since_progress: 0,
            width: 0,
            origin: 0.0,
            period: 1.0,
            luma: vec![0; MAX_WIDTH * usize::from(MAX_LINES)],
        }
    }

    pub(super) fn begin(&mut self, ioc: WefaxIoc, lpm: WefaxLpm, origin: f64, period: f64) {
        self.active = true;
        self.ioc = ioc;
        self.lpm = lpm;
        self.line = 0;
        self.decoded = 0;
        self.since_progress = 0;
        self.width = usize::from(ioc.width()).min(MAX_WIDTH);
        self.origin = origin;
        self.period = period;
    }

    pub(super) fn width(&self) -> u16 {
        self.width as u16
    }

    pub(super) fn full(&self) -> bool {
        self.line >= MAX_LINES
    }

    pub(super) fn line_start(&self, line: u16) -> f64 {
        self.origin + self.period * f64::from(line)
    }

    pub(super) fn line_end(&self, line: u16) -> f64 {
        self.line_start(line) + self.period
    }

    pub(super) fn scan_line(&mut self, track: &Track) {
        let start = self.line_start(self.line);
        let step = self.period / self.width as f64;
        let base = usize::from(self.line) * self.width;
        let row = &mut self.luma[base..base + self.width];
        for (x, pixel) in row.iter_mut().enumerate() {
            let from = start + step * x as f64;
            let unit = track.mean(from.round() as u64, (from + step).round() as u64);
            *pixel = (unit * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        self.line += 1;
        self.decoded = self.line;
        self.since_progress += 1;
    }

    pub(super) fn truncate(&mut self, limit: f64) {
        let fitting = ((limit - self.origin) / self.period).floor().max(0.0);
        self.decoded = self.decoded.min(fitting.min(f64::from(MAX_LINES)) as u16);
    }

    pub(super) fn label(&self) -> String {
        format!("IOC {} · {} LPM", self.ioc.value(), self.lpm.value())
    }

    pub(super) fn snapshot(&self) -> VideoPicture {
        let luma = self.luma[..self.width * usize::from(self.decoded)].to_vec();
        let rgb = luma.iter().flat_map(|&level| [level; 3]).collect();
        VideoPicture {
            width: self.width(),
            height: self.decoded,
            luma,
            rgb,
        }
    }
}
