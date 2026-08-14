//! Reference modulators for the decoders that have no transmitter of their own (:
//! every decoder ships with a fixture and an expected decode).
//!
//! Each submodule encodes a real protocol message down to complex baseband IQ at a caller-
//! chosen sample rate, so the same generator feeds three consumers without duplication:
//! the decoder's own unit tests, the engine's end-to-end tests through `device-virtual`,
//! and `cargo xtask fixtures`, which renders them into SigMF pairs. Keeping the encoders
//! here — rather than in `device-virtual` — is what preserves the crate boundary (:
//! `channels` depends only on `dsp` + `wire`); downstream crates enable the `test-signals`
//! feature to reach them.
//!
//! A mode that ships a [`ChannelTx`](crate::ChannelTx) has no generator here: the modulator
//! *is* the reference, and [`burst`] pulls a whole transmission out of one. What that costs is
//! the independence the rest of this module keeps — a modulator and its demodulator sit in the
//! same module — and what pays for it is that neither shares the other's code: AM and SSB
//! transmit by the method their receivers do not (envelope against detector, phasing against
//! filtering), and AX.25 hand-rolls the stuffing, checksum and NRZI that the receive path
//! undoes with `dsp` primitives.
//!
//! Generators are ideal by construction (no noise, no timing error, exact rates). Tests that
//! want impairments add them with the helpers here, so every impairment is explicit in the
//! test that depends on it.

pub mod acars;
pub mod adsb;
pub mod ais;
pub mod atv;
pub mod dv;
pub mod morse;
pub mod navtex;
pub mod nfm;
pub mod pocsag;
pub mod rds;
pub mod rtty;
pub mod subghz;
pub mod wfm;

use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::FracResampler;

use crate::ChannelTx;

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

/// Pull a modulator's whole burst out at its own rate: submit first, then call this. Stops on
/// the short fill that says the transmitter has ramped its carrier down.
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

/// Move a generated signal from the rate it was made at to the rate a device will replay it at.
/// A modulator produces its mode's channel rate and nothing else; a fixture or an end-to-end
/// run needs the device rate, which is deliberately not the same one.
#[must_use]
pub fn resample(iq: &[Complex<f32>], from: f64, to: f64) -> Vec<Complex<f32>> {
    let ratio = to / from;
    let mut resampler = FracResampler::new(ratio);
    let mut out = Vec::with_capacity((iq.len() as f64 * ratio) as usize + 1);
    resampler.process(iq, &mut out);
    out
}

/// A real sinusoid in ±1 at `amplitude` — the modulating waveform the analog-mode generators
/// carry, and the audio a demodulator test expects back out.
#[must_use]
pub fn tone_audio(freq_hz: f64, amplitude: f32, rate: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|k| amplitude * (TAU * freq_hz * k as f64 / rate).cos() as f32)
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
