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

pub mod acars;
pub mod adsb;
pub mod ais;
pub mod aprs;
pub mod morse;
pub mod navtex;
pub mod pocsag;
pub mod rds;
pub mod rtty;
pub mod subghz;

use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_wire::Sideband;

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

/// A real sinusoid in ±1 at `amplitude` — the modulating waveform the analog-mode generators
/// carry, and the audio a demodulator test expects back out.
#[must_use]
pub fn tone_audio(freq_hz: f64, amplitude: f32, rate: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|k| amplitude * (TAU * freq_hz * k as f64 / rate).cos() as f32)
        .collect()
}

/// Amplitude-modulate `audio` (in ±1) onto a carrier at complex baseband: the envelope is
/// `1 + depth·audio`, so `depth` is the modulation index — 1.0 is 100 % modulation, and past
/// it the envelope folds through zero, which no envelope detector can undo.
#[must_use]
pub fn am_modulate(audio: &[f32], depth: f32) -> Vec<Complex<f32>> {
    audio
        .iter()
        .map(|&s| Complex::new(1.0 + depth * s, 0.0))
        .collect()
}

/// Length of the Hilbert transformer behind [`ssb_modulate`]. Odd, so the in-phase path is
/// delayed by a whole number of samples; long enough that the quadrature path holds its 90°
/// across a voice channel at 48 kHz — the response necessarily decays toward DC and Nyquist,
/// so audio below a few hundred Hz leaks into the opposite sideband.
const HILBERT_TAPS: usize = 257;

/// Windowed Hilbert transformer: `2/(πn)` at odd offsets from the centre, zero elsewhere.
fn hilbert_taps() -> Vec<f32> {
    let center = (HILBERT_TAPS / 2) as i64;
    (0..HILBERT_TAPS)
        .map(|k| {
            let n = k as i64 - center;
            if n.unsigned_abs().is_multiple_of(2) {
                return 0.0;
            }
            // Blackman, spelled out rather than taken from `dsp`: a generator sharing a window
            // with the filters it is meant to test could hide an error in either.
            let phase = TAU * k as f64 / (HILBERT_TAPS - 1) as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            (window * 2.0 / (PI * n as f64)) as f32
        })
        .collect()
}

/// Single-sideband at complex baseband: `audio` (in ±1) sent as its analytic signal —
/// `a + j·H{a}` for the upper sideband, its conjugate for the lower.
///
/// This is the phasing method a real exciter uses, and it shares nothing with the decoder's
/// filter-one-side-then-take-real path, so an error in either shows up as a failed test rather
/// than cancelling out. The first and last `HILBERT_TAPS / 2` samples are the transformer's
/// transient — start measuring past them.
#[must_use]
pub fn ssb_modulate(audio: &[f32], sideband: Sideband) -> Vec<Complex<f32>> {
    let taps = hilbert_taps();
    let delay = taps.len() / 2;
    let sign = match sideband {
        Sideband::Usb => 1.0,
        Sideband::Lsb => -1.0,
    };
    (0..audio.len())
        .map(|k| {
            let quadrature: f32 = taps
                .iter()
                .enumerate()
                .filter_map(|(m, &h)| Some(h * audio[k.checked_sub(m)?]))
                .sum();
            let in_phase = k.checked_sub(delay).map_or(0.0, |i| audio[i]);
            Complex::new(in_phase, sign * quadrature)
        })
        .collect()
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

    const RATE: f64 = 48_000.0;

    /// Magnitude of the component of `iq` at `freq_hz` — a single-bin DFT, which needs no
    /// power-of-two length and lets the test name the frequency it cares about. A unit complex
    /// exponential at that frequency reads 1.0.
    fn component(iq: &[Complex<f32>], freq_hz: f64, rate: f64) -> f64 {
        let step = TAU * freq_hz / rate;
        let sum: Complex<f64> = iq
            .iter()
            .enumerate()
            .map(|(k, s)| {
                Complex::new(f64::from(s.re), f64::from(s.im))
                    * Complex::from_polar(1.0, -step * k as f64)
            })
            .sum();
        sum.norm() / iq.len() as f64
    }

    #[test]
    fn am_modulate_puts_the_requested_depth_on_the_envelope() {
        let iq = am_modulate(&tone_audio(1_000.0, 1.0, RATE, 4_800), 0.5);
        let peak = iq.iter().map(|s| s.norm()).fold(f32::MIN, f32::max);
        let trough = iq.iter().map(|s| s.norm()).fold(f32::MAX, f32::min);
        assert!((peak - 1.5).abs() < 1e-3, "peak envelope {peak}");
        assert!((trough - 0.5).abs() < 1e-3, "trough envelope {trough}");
    }

    /// The whole point of an SSB exciter: the audio lands on one side of DC and the mirror
    /// image is gone, not merely damped.
    #[test]
    fn ssb_modulate_leaves_the_image_40_db_down() {
        let settled = HILBERT_TAPS..24_000;
        for (sideband, sign) in [(Sideband::Usb, 1.0), (Sideband::Lsb, -1.0)] {
            for tone_hz in [700.0, 1_900.0, 2_700.0] {
                let iq = ssb_modulate(&tone_audio(tone_hz, 1.0, RATE, 24_000), sideband);
                let wanted = component(&iq[settled.clone()], sign * tone_hz, RATE);
                let image = component(&iq[settled.clone()], -sign * tone_hz, RATE);
                assert!(
                    (wanted - 1.0).abs() < 0.02,
                    "{sideband:?} {tone_hz} Hz: wanted sideband {wanted}"
                );
                assert!(
                    image < 0.01,
                    "{sideband:?} {tone_hz} Hz: image {image} against {wanted}"
                );
            }
        }
    }

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
