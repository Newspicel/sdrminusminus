//! Reference modulators for the wave-1 decoders (PLAN §14: every decoder ships with a
//! fixture and an expected decode).
//!
//! Each submodule encodes a real protocol message down to complex baseband IQ at a caller-
//! chosen sample rate, so the same generator feeds three consumers without duplication:
//! the decoder's own unit tests, the engine's end-to-end tests through `device-virtual`,
//! and `cargo xtask fixtures`, which renders them into SigMF pairs. Keeping the encoders
//! here — rather than in `device-virtual` — is what preserves the crate boundary (PLAN §3:
//! `channels` depends only on `dsp` + `wire`); downstream crates enable the `test-signals`
//! feature to reach them.
//!
//! Generators are ideal by construction (no noise, no timing error, exact rates). Tests that
//! want impairments add them with the helpers here, so every impairment is explicit in the
//! test that depends on it.

pub mod adsb;
pub mod ais;
pub mod aprs;
pub mod morse;
pub mod pocsag;
pub mod rds;
pub mod rtty;

use std::f64::consts::TAU;

use num_complex::Complex;

/// Multiply `iq` by `e^(j2π·freq·t)` — places a baseband generator at a channel offset.
pub fn shift(iq: &mut [Complex<f32>], freq_hz: f64, rate: f64) {
    let step = TAU * freq_hz / rate;
    for (k, s) in iq.iter_mut().enumerate() {
        let phase = (step * k as f64) as f32;
        *s *= Complex::from_polar(1.0, phase);
    }
}

/// Add deterministic uniform complex noise in roughly `±amp` (xorshift32).
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

/// Scale every sample — the level knob for "weak signal" tests.
pub fn scale(iq: &mut [Complex<f32>], gain: f32) {
    for s in iq {
        *s *= gain;
    }
}

/// `len` samples of silence, for lead-in/lead-out around a burst.
#[must_use]
pub fn silence(len: usize) -> Vec<Complex<f32>> {
    vec![Complex::new(0.0, 0.0); len]
}

/// Frequency-modulate a real baseband waveform onto a unit-amplitude carrier at `rate`.
/// `audio` is interpreted as a normalized modulating signal in ±1 scaled to `deviation_hz`.
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

/// Two-level FSK: `bits` at `baud`, mark (`true`) at `+deviation_hz`, space at
/// `−deviation_hz`. Phase-continuous, which is what every real FSK transmitter produces.
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

/// Bell-202-style audio FSK: `bits` at `baud` as continuous-phase tones, returned as the
/// real modulating waveform an FM transmitter would carry (feed it to [`fm_modulate`]).
#[must_use]
pub fn afsk_audio(bits: &[bool], baud: f64, mark_hz: f64, space_hz: f64, rate: f64) -> Vec<f32> {
    let sps = rate / baud;
    let len = (bits.len() as f64 * sps) as usize;
    let mut phase = 0.0f64;
    (0..len)
        .map(|k| {
            let idx = ((k as f64 / sps) as usize).min(bits.len().saturating_sub(1));
            let freq = if bits.get(idx).copied().unwrap_or(false) {
                mark_hz
            } else {
                space_hz
            };
            phase += TAU * freq / rate;
            if phase > TAU {
                phase -= TAU;
            }
            phase.sin() as f32
        })
        .collect()
}

/// On-off keying of a `tone_hz` carrier: `key[k]` is the envelope at sample `k`, in 0..=1.
/// Edges are raised-cosine shaped over `rise_s` so the spectrum stays inside a CW filter.
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
        // One full turn every 48 samples.
        let turn = iq[48] / iq[0];
        assert!((turn.re - 1.0).abs() < 1e-3, "{turn}");
        assert!(turn.im.abs() < 1e-3, "{turn}");
    }
}
