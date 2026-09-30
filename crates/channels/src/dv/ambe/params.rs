use std::f64::consts::{FRAC_PI_2, PI, SQRT_2};

use super::{
    frame::Info,
    tables::{BLOCK_LENGTHS, GAIN_STEPS, HARMONICS, HIGHER_ORDER, PRBA_HIGH, PRBA_LOW},
};

pub(super) const MAX_HARMONICS: usize = 56;
const MIN_HARMONICS: usize = 9;
const MAX_INTERPOLATION_INDEX: usize = MAX_HARMONICS - 1;
const TONE_PITCH: usize = 0x7E;
const PREDICTION_WEIGHT: f32 = 0.65;

const PITCH: [u8; 7] = [0, 1, 2, 3, 4, 5, 48];
const VOICING: [u8; 4] = [38, 39, 40, 41];
const GAIN: [u8; 6] = [6, 7, 8, 9, 42, 43];
const PRBA_LOW_BITS: [u8; 9] = [10, 11, 12, 13, 14, 15, 16, 44, 45];
const PRBA_HIGH_BITS: [u8; 7] = [17, 18, 19, 20, 21, 46, 47];
const HIGHER_ORDER_BITS: [&[u8]; 4] = [
    &[22, 23, 25, 26],
    &[27, 28, 29, 30],
    &[31, 32, 33, 34],
    &[35, 36, 37],
];

pub(super) type Bands<T> = [T; MAX_HARMONICS + 1];

#[derive(Clone, Debug)]
pub(super) struct Params {
    pub(super) w0: f32,
    pub(super) harmonics: usize,
    pub(super) voiced: Bands<bool>,
    pub(super) magnitude: Bands<f32>,
    pub(super) log2_magnitude: Bands<f32>,
    pub(super) phase: Bands<f32>,
    pub(super) phase_track: Bands<f32>,
    pub(super) gamma: f32,
    pub(super) repeats: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            w0: 0.093_78,
            harmonics: 30,
            voiced: [false; MAX_HARMONICS + 1],
            magnitude: [0.0; MAX_HARMONICS + 1],
            log2_magnitude: [0.0; MAX_HARMONICS + 1],
            phase: [0.0; MAX_HARMONICS + 1],
            phase_track: [FRAC_PI_2 as f32; MAX_HARMONICS + 1],
            gamma: 0.0,
            repeats: 0,
        }
    }
}

impl Params {
    fn extend_to(&mut self, harmonics: usize) {
        let last = self.harmonics;
        for l in last + 1..=harmonics {
            self.magnitude[l] = self.magnitude[last];
            self.log2_magnitude[l] = self.log2_magnitude[last];
        }
        self.log2_magnitude[0] = self.log2_magnitude[1];
        self.magnitude[0] = self.magnitude[1];
    }

    pub(super) fn clear_bands(&mut self, bands: std::ops::RangeInclusive<usize>) {
        for l in bands {
            self.magnitude[l] = 0.0;
            self.voiced[l] = true;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Quantized {
    pitch: usize,
    voicing: usize,
    gain: usize,
    prba_low: usize,
    prba_high: usize,
    higher_order: [usize; 4],
}

impl Quantized {
    pub(super) fn read(info: Info) -> Option<Self> {
        let pitch = info.field(&PITCH);
        if pitch & TONE_PITCH == TONE_PITCH {
            return None;
        }
        let mut higher_order = HIGHER_ORDER_BITS.map(|bits| info.field(bits));
        higher_order[3] <<= 1;
        Some(Self {
            pitch,
            voicing: info.field(&VOICING),
            gain: info.field(&GAIN),
            prba_low: info.field(&PRBA_LOW_BITS),
            prba_high: info.field(&PRBA_HIGH_BITS),
            higher_order,
        })
    }
}

pub(super) fn decode(quantized: &Quantized, current: &mut Params, previous: &mut Params) {
    let f0 = fundamental(quantized.pitch);
    let harmonics = usize::from(HARMONICS[quantized.pitch]);
    current.w0 = ((f0 * 2.0) as f64 * PI) as f32;
    current.harmonics = harmonics;
    for l in 1..=harmonics {
        let band = (l as f32 * 16.0 * f0) as usize;
        current.voiced[l] = quantized.voicing >> (3 - band / 2) & 1 == 1;
    }
    current.gamma = GAIN_STEPS[quantized.gain] + 0.5 * previous.gamma;
    let residuals = residuals(quantized, harmonics);
    previous.extend_to(harmonics);
    predict_magnitudes(current, previous, &residuals);
}

fn fundamental(pitch: usize) -> f32 {
    let exponent = -4.311_767_578_125 - 2.1336e-2 * (pitch as f64 + 0.5);
    2f32.powf(exponent as f32)
}

fn residuals(quantized: &Quantized, harmonics: usize) -> Bands<f32> {
    let gains = prba_gains(quantized);
    let lengths = BLOCK_LENGTHS[harmonics - MIN_HARMONICS];
    let scale = (1.0 / (2.0 * SQRT_2)) as f32;
    let mut residuals = [0.0; MAX_HARMONICS + 1];
    let mut l = 1;
    for (block, (&length, &index)) in lengths.iter().zip(&quantized.higher_order).enumerate() {
        let [low, high] = [gains[2 * block], gains[2 * block + 1]];
        let [h0, h1, h2, h3] = HIGHER_ORDER[block][index];
        let coefficients = [0.5 * (low + high), scale * (low - high), h0, h1, h2, h3];
        let length = usize::from(length);
        let used = &coefficients[..length.min(coefficients.len())];
        for j in 0..length {
            residuals[l] = inverse_dct(used, length, j);
            l += 1;
        }
    }
    residuals
}

fn prba_gains(quantized: &Quantized) -> [f32; 8] {
    let [g2, g3, g4] = PRBA_LOW[quantized.prba_low];
    let [g5, g6, g7, g8] = PRBA_HIGH[quantized.prba_high];
    let vector = [0.0, g2, g3, g4, g5, g6, g7, g8];
    std::array::from_fn(|i| inverse_dct(&vector, vector.len(), i))
}

fn inverse_dct(coefficients: &[f32], length: usize, index: usize) -> f32 {
    coefficients.iter().enumerate().fold(0.0, |sum, (k, &c)| {
        let weight = if k == 0 { 1.0 } else { 2.0 };
        let angle = (PI * k as f64 * (index as f64 + 0.5) / length as f64) as f32;
        sum + weight * c * angle.cos()
    })
}

fn predict_magnitudes(current: &mut Params, previous: &Params, residuals: &Bands<f32>) {
    let harmonics = current.harmonics;
    let count = harmonics as f32;
    let ratio = previous.harmonics as f32 / count;
    let neighbours = |l: usize| {
        let position = ratio * l as f32;
        let k = (position as usize).min(MAX_INTERPOLATION_INDEX);
        let delta = position - k as f32;
        (
            1.0 - delta,
            previous.log2_magnitude[k],
            delta,
            previous.log2_magnitude[k + 1],
        )
    };

    let mean_prediction = (1..=harmonics).fold(0.0, |sum, l| {
        let (w_low, low, w_high, high) = neighbours(l);
        sum + ((w_low * low) + (w_high * high))
    }) * (PREDICTION_WEIGHT / count);
    let mean_residual = residuals[1..=harmonics].iter().fold(0.0, |sum, &t| sum + t) / count;
    let offset = (f64::from(current.gamma)
        - 0.5 * (f64::from(count).ln() / 2f64.ln())
        - f64::from(mean_residual)) as f32;
    let unvoiced_scale = 0.2046 / current.w0.sqrt();

    for (l, &residual) in residuals.iter().enumerate().take(harmonics + 1).skip(1) {
        let (w_low, low, w_high, high) = neighbours(l);
        let log2 = residual + PREDICTION_WEIGHT * w_low * low + PREDICTION_WEIGHT * w_high * high
            - mean_prediction
            + offset;
        let linear = f64::from(0.693 * log2).exp();
        current.log2_magnitude[l] = log2;
        current.magnitude[l] = if current.voiced[l] {
            linear as f32
        } else {
            (f64::from(unvoiced_scale) * linear) as f32
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_voice_pitch_keeps_its_bands_in_the_voicing_table() {
        for pitch in (0..128).filter(|pitch| pitch & TONE_PITCH != TONE_PITCH) {
            let f0 = fundamental(pitch);
            let harmonics = usize::from(HARMONICS[pitch]);
            assert!((MIN_HARMONICS..=MAX_HARMONICS).contains(&harmonics));
            let top_band = (harmonics as f32 * 16.0 * f0) as usize;
            assert!(top_band < 8, "pitch {pitch} reaches band {top_band}");
        }
    }

    #[test]
    fn block_lengths_cover_every_harmonic() {
        for (row, lengths) in BLOCK_LENGTHS.iter().enumerate() {
            let total: usize = lengths.iter().map(|&length| usize::from(length)).sum();
            assert_eq!(total, row + MIN_HARMONICS);
        }
    }

    #[test]
    fn tone_frames_are_not_voice() {
        let tone = Info::from_bits(0b11_1111 << 43 | 1);
        assert!(Quantized::read(tone).is_none());
        assert!(Quantized::read(Info::from_bits(0)).is_some());
    }
}
