use sdrmm_wire::AvhrrChannel;

use super::{
    geometry::{LINE_WORDS, Side, WEDGE_LINES},
    telemetry::{self, Calibration},
    track::Track,
};
use crate::VideoPicture;

pub(crate) const MAX_LINES: usize = 2_400;
const RAW_SCALE: f32 = 16_384.0;
const HISTOGRAM_SHIFT: u32 = 6;
const HISTOGRAM_BINS: usize = 1 << (u16::BITS - HISTOGRAM_SHIFT);
const STRETCH_TAIL: f64 = 0.005;
const TELEMETRY_MARGIN: usize = 8;
const PROGRESS_LINES: usize = 8;

pub(crate) struct Picture {
    pub(crate) active: bool,
    pub(crate) lines: usize,
    pub(crate) synced_lines: usize,
    pub(crate) started: f64,
    raw: Vec<u16>,
    column_a: Vec<f32>,
    column_b: Vec<f32>,
    histogram: Vec<u32>,
    since_progress: usize,
    calibration: Option<Calibration>,
    pub(crate) channel_a: Option<AvhrrChannel>,
    pub(crate) channel_b: Option<AvhrrChannel>,
}

fn quantize(amplitude: f32) -> u16 {
    (amplitude * RAW_SCALE)
        .round()
        .clamp(0.0, f32::from(u16::MAX)) as u16
}

impl Picture {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            lines: 0,
            synced_lines: 0,
            started: 0.0,
            raw: vec![0; MAX_LINES * LINE_WORDS],
            column_a: vec![0.0; MAX_LINES],
            column_b: vec![0.0; MAX_LINES],
            histogram: vec![0; HISTOGRAM_BINS],
            since_progress: 0,
            calibration: None,
            channel_a: None,
            channel_b: None,
        }
    }

    pub(crate) fn begin(&mut self, started: f64) {
        self.active = true;
        self.lines = 0;
        self.synced_lines = 0;
        self.started = started;
        self.histogram.fill(0);
        self.since_progress = 0;
        self.calibration = None;
        self.channel_a = None;
        self.channel_b = None;
    }

    pub(crate) fn full(&self) -> bool {
        self.lines >= MAX_LINES
    }

    pub(crate) fn store_line(&mut self, track: &Track, start: f64, len: f64, synced: bool) {
        if self.full() {
            return;
        }
        let line = self.lines;
        let row = &mut self.raw[line * LINE_WORDS..(line + 1) * LINE_WORDS];
        let word = len / LINE_WORDS as f64;
        for (index, value) in row.iter_mut().enumerate() {
            *value = quantize(track.at(start + (index as f64 + 0.5) * word));
        }
        for side in [Side::A, Side::B] {
            for &value in &row[side.image()] {
                self.histogram[usize::from(value >> HISTOGRAM_SHIFT)] += 1;
            }
        }
        self.column_a[line] = column_mean(row, Side::A);
        self.column_b[line] = column_mean(row, Side::B);
        self.lines += 1;
        self.since_progress += 1;
        if synced {
            self.synced_lines = self.lines;
        }
        if self.lines.is_multiple_of(WEDGE_LINES) {
            self.read_telemetry();
        }
    }

    fn read_telemetry(&mut self) {
        let lines = self.lines;
        let Some(reading) = telemetry::read(&self.column_a[..lines], &self.column_b[..lines])
        else {
            return;
        };
        self.calibration = Some(reading.calibration);
        self.channel_a = reading.channel_a.or(self.channel_a);
        self.channel_b = reading.channel_b.or(self.channel_b);
    }

    pub(crate) fn progress_due(&mut self) -> bool {
        if self.since_progress < PROGRESS_LINES {
            return false;
        }
        self.since_progress = 0;
        true
    }

    pub(crate) fn calibration(&self) -> Calibration {
        self.calibration.unwrap_or_else(|| self.stretch())
    }

    fn stretch(&self) -> Calibration {
        let total: u64 = self.histogram.iter().map(|&count| u64::from(count)).sum();
        let tail = (total as f64 * STRETCH_TAIL) as u64;
        let bin_value = |bin: usize| (bin << HISTOGRAM_SHIFT) as f32;
        let mut seen = 0u64;
        let mut black = 0.0;
        let mut white = f32::from(u16::MAX);
        let mut black_found = false;
        for (bin, &count) in self.histogram.iter().enumerate() {
            seen += u64::from(count);
            if !black_found && seen > tail {
                black = bin_value(bin);
                black_found = true;
            }
            if seen >= total.saturating_sub(tail) {
                white = bin_value(bin + 1);
                break;
            }
        }
        Calibration { black, white }
    }

    pub(crate) fn mode(&self) -> String {
        match (self.channel_a, self.channel_b) {
            (Some(a), Some(b)) => format!("APT {}/{}", a.label(), b.label()),
            _ => "APT".to_owned(),
        }
    }

    pub(crate) fn snapshot(&self, lines: usize) -> VideoPicture {
        let lines = lines.min(self.lines);
        let calibration = self.calibration();
        let luma: Vec<u8> = self.raw[..lines * LINE_WORDS]
            .iter()
            .map(|&raw| calibration.level(raw))
            .collect();
        let rgb = luma.iter().flat_map(|&level| [level; 3]).collect();
        VideoPicture {
            width: LINE_WORDS as u16,
            height: lines as u16,
            luma,
            rgb,
        }
    }
}

fn column_mean(row: &[u16], side: Side) -> f32 {
    let range = side.telemetry();
    let words = &row[range.start + TELEMETRY_MARGIN..range.end - TELEMETRY_MARGIN];
    words.iter().map(|&v| f32::from(v)).sum::<f32>() / words.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_telemetry_the_picture_stretches_its_histogram() {
        let mut picture = Picture::new();
        picture.begin(0.0);
        let mut track = Track::new();
        for k in 0..20_000 {
            track.push(0.2 + 0.6 * (k % 7) as f32 / 6.0);
        }
        for line in 0..4 {
            picture.store_line(&track, line as f64 * 4_000.0, 4_000.0, true);
        }
        let calibration = picture.calibration();
        assert!(calibration.level(quantize(0.2)) < 8);
        assert!(calibration.level(quantize(0.8)) > 247);
        assert!(picture.calibration.is_none());
    }

    #[test]
    fn the_mode_names_both_channels_once_known() {
        let mut picture = Picture::new();
        assert_eq!(picture.mode(), "APT");
        picture.channel_a = Some(AvhrrChannel::Ch2);
        picture.channel_b = Some(AvhrrChannel::Ch4);
        assert_eq!(picture.mode(), "APT 2/4");
    }

    #[test]
    fn a_full_picture_stops_taking_lines() {
        let mut picture = Picture::new();
        picture.begin(0.0);
        let track = Track::new();
        for _ in 0..MAX_LINES + 3 {
            picture.store_line(&track, 0.0, 6_000.0, false);
        }
        assert_eq!(picture.lines, MAX_LINES);
        assert!(picture.full());
    }
}
