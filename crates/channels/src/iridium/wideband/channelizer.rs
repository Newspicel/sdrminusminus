use num_complex::Complex;
use sdrmm_dsp::fft::FftPair;

use super::super::CHANNEL_RATE;

pub const CHANNEL_BINS: usize = 256;
pub const HOP_OUT: usize = CHANNEL_BINS / 2;
pub const KEEP_FROM: usize = CHANNEL_BINS / 4;
pub const HISTORY: usize = 16;
pub const BIN_HZ: f64 = CHANNEL_RATE / CHANNEL_BINS as f64;
const PASS_HZ: f64 = 20_000.0;
const STOP_HZ: f64 = 32_000.0;

pub struct Channelizer {
    size: usize,
    hop: usize,
    wide: FftPair,
    narrow: FftPair,
    pending: Vec<Complex<f32>>,
    consumed: usize,
    spectra: Vec<Complex<f32>>,
    frames: u64,
    response: [f32; CHANNEL_BINS],
    scratch: Vec<Complex<f32>>,
}

fn signed_bin(index: usize, len: usize) -> f64 {
    if index < len / 2 {
        index as f64
    } else {
        index as f64 - len as f64
    }
}

fn gain(hz: f64) -> f64 {
    let hz = hz.abs();
    if hz <= PASS_HZ {
        1.0
    } else if hz >= STOP_HZ {
        0.0
    } else {
        0.5 * (1.0 + (std::f64::consts::PI * (hz - PASS_HZ) / (STOP_HZ - PASS_HZ)).cos())
    }
}

impl Channelizer {
    pub fn new(decimation: usize) -> Self {
        let size = decimation * CHANNEL_BINS;
        let scale = 1.0 / size as f64;
        Self {
            size,
            hop: size / 2,
            wide: FftPair::new(size),
            narrow: FftPair::new(CHANNEL_BINS),
            pending: Vec::new(),
            consumed: 0,
            spectra: vec![Complex::default(); HISTORY * size],
            frames: 0,
            response: std::array::from_fn(|i| {
                (gain(signed_bin(i, CHANNEL_BINS) * BIN_HZ) * scale) as f32
            }),
            scratch: vec![Complex::default(); CHANNEL_BINS],
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn oldest(&self) -> u64 {
        self.frames.saturating_sub(HISTORY as u64)
    }

    pub fn push(&mut self, input: &[Complex<f32>]) {
        self.pending.extend_from_slice(input);
    }

    pub fn next_frame(&mut self) -> Option<u64> {
        let from = self.consumed;
        if self.pending.len() < from + self.size {
            return None;
        }
        let frame = self.frames;
        let range = self.slot(frame);
        let spectrum = &mut self.spectra[range];
        spectrum.copy_from_slice(&self.pending[from..from + self.size]);
        self.wide.forward(spectrum);
        self.consumed += self.hop;
        self.frames += 1;
        Some(frame)
    }

    pub fn compact(&mut self) {
        self.pending.drain(..self.consumed);
        self.consumed = 0;
    }

    pub fn spectrum(&self, frame: u64) -> &[Complex<f32>] {
        &self.spectra[self.slot(frame)]
    }

    pub fn extract(&mut self, frame: u64, bin: usize, out: &mut Vec<Complex<f32>>) {
        let range = self.slot(frame);
        let spectrum = &self.spectra[range];
        let sign = if bin % 2 == 1 && frame % 2 == 1 {
            -1.0
        } else {
            1.0
        };
        let wrap = self.size - CHANNEL_BINS;
        for (i, (slot, gain)) in self.scratch.iter_mut().zip(&self.response).enumerate() {
            let offset = if i < CHANNEL_BINS / 2 { i } else { wrap + i };
            *slot = spectrum[(bin + offset) % self.size] * (gain * sign);
        }
        self.narrow.inverse(&mut self.scratch);
        out.extend_from_slice(&self.scratch[KEEP_FROM..KEEP_FROM + HOP_OUT]);
    }

    fn slot(&self, frame: u64) -> std::ops::Range<usize> {
        let start = (frame % HISTORY as u64) as usize * self.size;
        start..start + self.size
    }
}
