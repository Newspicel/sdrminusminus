pub mod acars;
pub mod adsb;
pub mod ais;
pub mod atv;
pub mod dv;
pub mod ermes;
pub mod flex;
pub mod gnss;
pub mod morse;
pub mod navtex;
pub mod nfm;
pub mod pocsag;
pub mod psk;
pub mod radio_clock;
pub mod rds;
pub mod rtty;
pub mod selcall;
pub mod sstv;
pub mod subghz;
pub mod vor;
pub mod weak_signal;
pub mod wfm;

use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::FracResampler;

use crate::ChannelTx;

pub fn shift(iq: &mut [Complex<f32>], freq_hz: f64, rate: f64) {
    let step = TAU * freq_hz / rate;
    for (k, s) in iq.iter_mut().enumerate() {
        let phase = (step * k as f64) as f32;
        *s *= Complex::from_polar(1.0, phase);
    }
}

pub fn add_noise(iq: &mut [Complex<f32>], seed: u32, amp: f32) {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
    };
    for s in iq {
        *s += Complex::new(next(), next());
    }
}

pub fn scale(iq: &mut [Complex<f32>], gain: f32) {
    for s in iq {
        *s *= gain;
    }
}

#[must_use]
pub fn silence(len: usize) -> Vec<Complex<f32>> {
    vec![Complex::new(0.0, 0.0); len]
}

#[must_use]
pub fn burst(tx: &mut dyn ChannelTx) -> Vec<Complex<f32>> {
    let mut iq = Vec::new();
    let mut block = [Complex::new(0.0, 0.0); 4_096];
    loop {
        let n = tx.generate(&mut block);
        iq.extend_from_slice(&block[..n]);
        if n < block.len() {
            return iq;
        }
    }
}

#[must_use]
pub fn resample(iq: &[Complex<f32>], from: f64, to: f64) -> Vec<Complex<f32>> {
    let ratio = to / from;
    let mut resampler = FracResampler::new(ratio);
    let mut out = Vec::with_capacity((iq.len() as f64 * ratio) as usize + 1);
    resampler.process(iq, &mut out);
    out
}

#[must_use]
pub fn tone_audio(freq_hz: f64, amplitude: f32, rate: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|k| amplitude * (TAU * freq_hz * k as f64 / rate).cos() as f32)
        .collect()
}

#[must_use]
pub fn fm_modulate(audio: &[f32], deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let mut phase = 0.0f64;
    audio
        .iter()
        .map(|&s| {
            phase += TAU * deviation_hz * f64::from(s) / rate;
            if phase > TAU {
                phase -= TAU;
            } else if phase < -TAU {
                phase += TAU;
            }
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

#[must_use]
pub fn fsk(bits: &[bool], baud: f64, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let sps = rate / baud;
    let symbols: Vec<f32> = (0..(bits.len() as f64 * sps) as usize)
        .map(|k| {
            let idx = ((k as f64 / sps) as usize).min(bits.len() - 1);
            if bits[idx] { 1.0 } else { -1.0 }
        })
        .collect();
    fm_modulate(&symbols, deviation_hz, rate)
}

#[must_use]
pub fn ook(key: &[f32], tone_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let step = TAU * tone_hz / rate;
    key.iter()
        .enumerate()
        .map(|(k, &env)| Complex::from_polar(env, (step * k as f64) as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsk_produces_the_requested_duration_and_unit_magnitude() {
        let bits = [true, false, true, true, false, false, true, false];
        let iq = fsk(&bits, 1_200.0, 4_500.0, 48_000.0);
        assert_eq!(iq.len(), 8 * 40);
        for s in &iq {
            assert!((s.norm() - 1.0).abs() < 1e-3, "magnitude {}", s.norm());
        }
    }

    #[test]
    fn shift_moves_a_dc_carrier_to_the_requested_offset() {
        let mut iq = vec![Complex::new(1.0f32, 0.0); 4_800];
        shift(&mut iq, 1_000.0, 48_000.0);
        let turn = iq[48] / iq[0];
        assert!((turn.re - 1.0).abs() < 1e-3, "{turn}");
        assert!(turn.im.abs() < 1e-3, "{turn}");
    }
}
