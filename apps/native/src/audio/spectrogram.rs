use num_complex::Complex;
use sdrmm_dsp::{
    fft::FftPair,
    window::{coherent_gain, hann},
};

pub const FFT_SIZE: usize = 1024;
pub const HOP: usize = 512;
pub const DB_MIN: f32 = -90.0;
pub const DB_MAX: f32 = 0.0;

pub struct AudioSpectrogram {
    size: usize,
    hop: usize,
    window: Vec<f32>,
    inverse_gain: f32,
    fft: FftPair,
    buffer: Vec<Complex<f32>>,
    history: Vec<f32>,
    write: usize,
    since: usize,
    row: Vec<u8>,
}

impl AudioSpectrogram {
    #[must_use]
    pub fn new(size: usize, hop: usize) -> Self {
        let window = hann(size);
        let inverse_gain = 1.0 / coherent_gain(&window).max(f32::MIN_POSITIVE);
        Self {
            size,
            hop: hop.max(1),
            window,
            inverse_gain,
            fft: FftPair::new(size),
            buffer: vec![Complex::default(); size],
            history: vec![0.0; size],
            write: 0,
            since: 0,
            row: vec![0; size / 2],
        }
    }

    #[cfg(test)]
    #[must_use]
    pub const fn bins(&self) -> usize {
        self.size / 2
    }

    pub fn push(&mut self, pcm: &[f32], channels: usize, mut emit: impl FnMut(&[u8])) {
        let lanes = channels.max(1);
        for frame in pcm.chunks_exact(lanes) {
            self.history[self.write] = frame.iter().sum::<f32>() / lanes as f32;
            self.write = (self.write + 1) % self.size;
            self.since += 1;
            if self.since >= self.hop {
                self.since = 0;
                self.transform();
                emit(&self.row);
            }
        }
    }

    fn transform(&mut self) {
        for (i, slot) in self.buffer.iter_mut().enumerate() {
            let sample = self.history[(self.write + i) % self.size] * self.window[i];
            *slot = Complex::new(sample, 0.0);
        }
        self.fft.forward(&mut self.buffer);
        let span = DB_MAX - DB_MIN;
        for (k, byte) in self.row.iter_mut().enumerate() {
            let fold = if k == 0 { 1.0 } else { 2.0 };
            let magnitude = self.buffer[k].norm() * self.inverse_gain * fold;
            let db = 20.0 * (magnitude + 1e-12).log10();
            *byte = ((db - DB_MIN) / span * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[must_use]
pub fn tick_fraction(hz: f32, sample_rate: f32) -> f32 {
    let nyquist = sample_rate / 2.0;
    if nyquist <= 0.0 { 0.0 } else { hz / nyquist }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, frames: usize, channels: usize, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let value = (TAU * hz * frame as f32 / RATE).sin() * amplitude;
                std::iter::repeat_n(value, channels)
            })
            .collect()
    }

    fn last_row(spectrogram: &mut AudioSpectrogram, pcm: &[f32], channels: usize) -> Vec<u8> {
        let mut row = Vec::new();
        spectrogram.push(pcm, channels, |emitted| row = emitted.to_vec());
        row
    }

    fn db(byte: u8) -> f32 {
        DB_MIN + f32::from(byte) / 255.0 * (DB_MAX - DB_MIN)
    }

    fn peak(row: &[u8]) -> (usize, u8) {
        row.iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, value)| *value)
            .unwrap_or_default()
    }

    #[test]
    fn emits_one_row_per_hop_whatever_size_the_blocks_arrive_in() {
        let mut spectrogram = AudioSpectrogram::new(256, 128);
        let mut rows = 0;
        for _ in 0..8 {
            spectrogram.push(&tone(1_000.0, 128, 1, 1.0), 1, |_| rows += 1);
        }
        assert_eq!(rows, 8);
        let mut single = AudioSpectrogram::new(256, 128);
        let mut rows = 0;
        for sample in tone(3_000.0, 256, 1, 1.0) {
            single.push(&[sample], 1, |_| rows += 1);
        }
        assert_eq!(rows, 2);
    }

    #[test]
    fn puts_a_full_scale_tone_in_its_bin_at_the_top_of_the_window() {
        let mut spectrogram = AudioSpectrogram::new(FFT_SIZE, HOP);
        let row = last_row(&mut spectrogram, &tone(3_000.0, 4_096, 1, 1.0), 1);
        let (bin, value) = peak(&row);
        assert!(bin.abs_diff(64) <= 1);
        assert!(db(value) > -3.0);
        assert_eq!(row.len(), spectrogram.bins());
    }

    #[test]
    fn scales_with_amplitude_the_way_decibels_say() {
        let full = peak(&last_row(
            &mut AudioSpectrogram::new(FFT_SIZE, HOP),
            &tone(3_000.0, 4_096, 1, 1.0),
            1,
        ))
        .1;
        let half = peak(&last_row(
            &mut AudioSpectrogram::new(FFT_SIZE, HOP),
            &tone(3_000.0, 4_096, 1, 0.5),
            1,
        ))
        .1;
        assert!((db(full) - db(half) - 6.02).abs() < 0.5);
    }

    #[test]
    fn averages_the_channels_of_a_stereo_block() {
        let mono = peak(&last_row(
            &mut AudioSpectrogram::new(FFT_SIZE, HOP),
            &tone(3_000.0, 4_096, 1, 1.0),
            1,
        ))
        .1;
        let stereo = peak(&last_row(
            &mut AudioSpectrogram::new(FFT_SIZE, HOP),
            &tone(3_000.0, 4_096, 2, 1.0),
            2,
        ))
        .1;
        assert!(mono.abs_diff(stereo) <= 2);
    }

    #[test]
    fn floors_silence_and_survives_a_nonsense_channel_count() {
        let mut spectrogram = AudioSpectrogram::new(256, 128);
        let row = last_row(&mut spectrogram, &[0.0; 1_024], 1);
        assert_eq!(peak(&row).1, 0);
        assert!(!last_row(&mut spectrogram, &tone(3_000.0, 1_024, 1, 1.0), 0).is_empty());
    }

    #[test]
    fn places_a_tick_at_its_share_of_the_band() {
        assert!((tick_fraction(3_000.0, RATE) - 0.125).abs() < f32::EPSILON);
        assert!((tick_fraction(24_000.0, RATE) - 1.0).abs() < f32::EPSILON);
        assert!(tick_fraction(3_000.0, 0.0).abs() < f32::EPSILON);
    }
}
