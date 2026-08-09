//! Complex-coefficient FIR (PLAN §7): a real lowpass prototype modulated to an arbitrary
//! center frequency. Unlike a real filter, the response is one-sided — the SSB band-selection
//! primitive (USB keeps positive frequencies, LSB negative).

use std::f64::consts::TAU;

use num_complex::Complex;

use crate::fir::StreamFir;

#[derive(Clone, Debug)]
pub struct FirC {
    core: StreamFir<Complex<f32>, Complex<f32>>,
}

impl FirC {
    /// Modulate real lowpass `taps` to `center_norm` (normalized to the sample rate, may be
    /// negative): `c[k] = taps[k]·e^(j·2π·center·k)`.
    #[must_use]
    pub fn from_lowpass(taps: &[f32], center_norm: f64) -> Self {
        let coeffs: Vec<Complex<f32>> = taps
            .iter()
            .enumerate()
            .map(|(k, &t)| {
                let phi = TAU * center_norm * k as f64;
                Complex::new(
                    (phi.cos() * f64::from(t)) as f32,
                    (phi.sin() * f64::from(t)) as f32,
                )
            })
            .collect();
        Self {
            core: StreamFir::new(&coeffs, 1),
        }
    }

    /// Replaces `out` with one filtered sample per input sample.
    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.core.process(input, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design_lowpass,
        testutil::{complex_tone, rms_c},
    };

    #[test]
    fn usb_filter_passes_positive_and_rejects_negative() {
        // USB selection at 48 kHz: lowpass ±2.4 kHz shifted up by 2.4 kHz → passband 0…4.8 kHz.
        let lp = design_lowpass(257, 0.05);
        let mut usb = FirC::from_lowpass(&lp, 0.05);
        let mut out = Vec::new();

        let f = 1_000.0 / 48_000.0;
        usb.process(&complex_tone(f, 8_192), &mut out);
        let pass = rms_c(&out[512..]);
        assert!((0.89..1.05).contains(&pass), "+1 kHz rms {pass}");

        let mut usb = FirC::from_lowpass(&lp, 0.05);
        usb.process(&complex_tone(-f, 8_192), &mut out);
        let reject = rms_c(&out[512..]);
        assert!(reject < 0.01, "−1 kHz leak rms {reject}");
    }
}
