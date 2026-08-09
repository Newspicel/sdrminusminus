//! Numerically-controlled oscillator / complex mixer (PLAN §7). The DDC front-end shifts a
//! channel to baseband by multiplying IQ with a rotating phasor; the same primitive drives
//! test-signal generation. No allocation in `mix_into` — safe for the hot path.

use std::f32::consts::PI;

use num_complex::Complex;

/// A unit-magnitude phasor advanced by a fixed phase increment per sample.
#[derive(Clone, Debug)]
pub struct Nco {
    phase: f32,
    step: f32,
}

impl Nco {
    /// Create an NCO for `freq_hz` at `sample_rate` (Hz). Positive frequency rotates
    /// counter-clockwise; use a negative frequency to shift a signal down to baseband.
    #[must_use]
    pub fn new(freq_hz: f32, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            step: wrap_pi(2.0 * PI * freq_hz / sample_rate),
        }
    }

    /// Retune without discontinuity: phase is preserved, only the increment changes.
    pub fn set_freq(&mut self, freq_hz: f32, sample_rate: f32) {
        self.step = wrap_pi(2.0 * PI * freq_hz / sample_rate);
    }

    /// Advance one sample and return the current phasor.
    #[must_use]
    pub fn next_sample(&mut self) -> Complex<f32> {
        let c = Complex::from_polar(1.0, self.phase);
        self.phase += self.step;
        // `step` is pre-normalized to (-PI, PI], so one correction always suffices to keep the
        // accumulated phase bounded and f32 precision from eroding over long runs.
        if self.phase >= PI {
            self.phase -= 2.0 * PI;
        } else if self.phase < -PI {
            self.phase += 2.0 * PI;
        }
        c
    }

    /// Mix `input` by this oscillator into `out` (element-wise complex multiply). Lengths
    /// must match. Allocation-free.
    pub fn mix_into(&mut self, input: &[Complex<f32>], out: &mut [Complex<f32>]) {
        debug_assert_eq!(input.len(), out.len());
        for (i, o) in input.iter().zip(out.iter_mut()) {
            *o = *i * self.next_sample();
        }
    }
}

/// Reduce a per-sample phase increment into `(-PI, PI]`. Because `e^(j·θ·n)` is periodic in
/// `2π` per integer sample, this is exact — it yields the correctly-aliased tone for any input
/// frequency (including `|freq| > fs`), and keeps the increment small enough that a single wrap
/// in `next_sample` always bounds the phase.
fn wrap_pi(x: f32) -> f32 {
    (x + PI).rem_euclid(2.0 * PI) - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_a_tone_to_baseband_yields_dc() {
        let fs = 48_000.0;
        let f = 6_000.0;
        // A pure complex tone at +f Hz.
        let mut src = Nco::new(f, fs);
        let tone: Vec<Complex<f32>> = (0..1024).map(|_| src.next_sample()).collect();

        // Mix down by -f: result should be a (near-)constant DC phasor.
        let mut mixer = Nco::new(-f, fs);
        let mut out = vec![Complex::new(0.0, 0.0); tone.len()];
        mixer.mix_into(&tone, &mut out);

        let mean: Complex<f32> = out.iter().sum::<Complex<f32>>() / out.len() as f32;
        // All samples align, so |mean| ~= 1.0.
        assert!(
            (mean.norm() - 1.0).abs() < 1e-3,
            "mean norm {}",
            mean.norm()
        );
    }

    #[test]
    fn out_of_range_frequency_aliases_and_stays_bounded() {
        let fs = 48_000.0;
        // 1.5·fs aliases to -0.5·fs; the phasor sequence must match a tone at the aliased freq
        // and the phase must stay bounded over a long run (no f32 erosion).
        let mut aliased = Nco::new(1.5 * fs, fs);
        let mut reference = Nco::new(-0.5 * fs, fs);
        let mut max_err = 0.0f32;
        for _ in 0..100_000 {
            let a = aliased.next_sample();
            let r = reference.next_sample();
            max_err = max_err.max((a - r).norm());
        }
        assert!(
            max_err < 1e-2,
            "aliased phasor diverged from reference: {max_err}"
        );
    }
}
