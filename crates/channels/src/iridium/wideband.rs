mod channelizer;
mod detect;
#[cfg(test)]
mod tests;

use num_complex::Complex;

use self::channelizer::{BIN_HZ, Channelizer, HOP_OUT, KEEP_FROM};
use self::detect::{DEPTH, Detector};
use super::CHANNEL_RATE;
use super::decode::Reassembly;
use super::ira::IridiumFrame;
use super::receiver::{Heard, WindowDemod, reassemble};
use crate::ChannelError;

pub const USABLE_FRACTION: f64 = 0.4;
const LEAD: u64 = 12;
const POST: u64 = 12;
const MAX_FRAMES: u64 = 196;

struct Burst {
    bin: usize,
    start: u64,
    first: u64,
    last_hot: u64,
    samples: Vec<Complex<f32>>,
}

pub struct WidebandDecoder {
    channelizer: Channelizer,
    detector: Detector,
    window: WindowDemod,
    reassembly: Reassembly,
    active: Vec<Burst>,
    spare: Vec<Vec<Complex<f32>>>,
    found: Vec<usize>,
}

pub fn decimation(input_rate: f64) -> Result<usize, ChannelError> {
    let ratio = input_rate / CHANNEL_RATE;
    let rounded = ratio.round();
    if rounded >= 2.0 && (ratio - rounded).abs() < 1e-9 {
        Ok(rounded as usize)
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "Iridium span needs a multiple of {CHANNEL_RATE} Hz, got {input_rate} Hz"
        )))
    }
}

fn unshifted(bin: usize, size: usize) -> usize {
    (bin + size / 2) % size
}

impl WidebandDecoder {
    pub fn new(input_rate: f64) -> Result<Self, ChannelError> {
        let channelizer = Channelizer::new(decimation(input_rate)?);
        let usable_bins = (USABLE_FRACTION * input_rate / BIN_HZ) as usize;
        Ok(Self {
            detector: Detector::new(channelizer.size(), usable_bins),
            channelizer,
            window: WindowDemod::new(),
            reassembly: Reassembly::new(),
            active: Vec::new(),
            spare: Vec::new(),
            found: Vec::new(),
        })
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<IridiumFrame>) {
        self.channelizer.push(input);
        while let Some(frame) = self.channelizer.next_frame() {
            self.step(frame, out);
        }
        self.channelizer.compact();
    }

    fn step(&mut self, frame: u64, out: &mut Vec<IridiumFrame>) {
        self.detector.measure(self.channelizer.spectrum(frame));
        if !self.detector.ready() {
            return;
        }
        self.detector.begin();
        for burst in &mut self.active {
            if self.detector.holds(burst.bin) {
                burst.last_hot = frame;
            }
            self.detector.mask(burst.bin);
        }
        self.found.clear();
        self.detector.claim(&mut self.found);
        self.detector.settle();
        self.start_bursts(frame);
        let size = self.channelizer.size();
        for burst in &mut self.active {
            self.channelizer
                .extract(frame, unshifted(burst.bin, size), &mut burst.samples);
        }
        self.finish_bursts(frame, out);
    }

    fn start_bursts(&mut self, frame: u64) {
        for index in 0..self.found.len() {
            let bin = self.found[index];
            let first = frame.saturating_sub(LEAD).max(self.channelizer.oldest());
            let mut samples = self.spare.pop().unwrap_or_default();
            samples.clear();
            let wide_bin = unshifted(bin, self.channelizer.size());
            for past in first..frame {
                self.channelizer.extract(past, wide_bin, &mut samples);
            }
            self.active.push(Burst {
                bin,
                start: frame,
                first,
                last_hot: frame,
                samples,
            });
        }
    }

    fn finish_bursts(&mut self, frame: u64, out: &mut Vec<IridiumFrame>) {
        let mut index = 0;
        while index < self.active.len() {
            let burst = &self.active[index];
            if frame - burst.last_hot >= POST || frame - burst.start >= MAX_FRAMES {
                let burst = self.active.swap_remove(index);
                self.decode(&burst, frame, out);
                self.spare.push(burst.samples);
            } else {
                index += 1;
            }
        }
    }

    fn decode(&mut self, burst: &Burst, frame: u64, out: &mut Vec<IridiumFrame>) {
        let onset_abs = HOP_OUT as u64 * (burst.start + 1).saturating_sub(DEPTH as u64);
        let window_abs = HOP_OUT as u64 * burst.first + KEEP_FROM as u64;
        let onset = onset_abs.saturating_sub(window_abs) as f64;
        let center_hz = (burst.bin as f64 - (self.channelizer.size() / 2) as f64) * BIN_HZ;
        let time = (frame * HOP_OUT as u64) as f64 / CHANNEL_RATE;
        for demodulated in self.window.bursts(&burst.samples, onset) {
            let heard = Heard {
                time,
                freq: center_hz + demodulated.cfo_hz,
                center_hz,
            };
            reassemble(&mut self.reassembly, &demodulated, &heard, out);
        }
    }
}
