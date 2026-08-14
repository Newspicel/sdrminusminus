//! Analog-front-end impairments: IQ imbalance, DC offset, clipping, and quantisation — what
//! the mixer, the ADC and an overdriven input stage do to a signal. The relative ones (DC,
//! clip threshold, quantiser full scale) are stated against the waveform's measured RMS, so
//! the instrument means the same thing whatever absolute level the modulator emitted.

use num_complex::Complex;

use super::{Impairment, rms};
use crate::ber::rng::Rng;

/// Receive IQ imbalance in the standard image model `y = a·x + b·conj(x)` with
/// `a = (1 + g·e^{jφ})/2`, `b = (1 − g·e^{jφ})/2`, where `g` is the linear gain ratio of the
/// Q branch to the I branch and `φ` its phase error. Balanced hardware gives `a = 1, b = 0`;
/// the `b·conj(x)` term is the image, and its level is what the calibration measures.
#[derive(Clone, Copy, Debug)]
pub struct IqImbalance {
    a: Complex<f64>,
    b: Complex<f64>,
}

impl IqImbalance {
    #[must_use]
    pub fn new(gain_db: f64, phase_deg: f64) -> Self {
        let g = 10f64.powf(gain_db / 20.0);
        let (sin, cos) = phase_deg.to_radians().sin_cos();
        let ge = Complex::new(g * cos, g * sin);
        Self {
            a: (Complex::new(1.0, 0.0) + ge) / 2.0,
            b: (Complex::new(1.0, 0.0) - ge) / 2.0,
        }
    }

    /// Closed-form image rejection, `|a|²/|b|²` in dB — the number the calibration test
    /// measures back from a tone, and the number an IQ-imbalance limits row is stated in.
    #[must_use]
    pub fn image_rejection_db(&self) -> f64 {
        10.0 * (self.a.norm_sqr() / self.b.norm_sqr()).log10()
    }
}

impl Impairment for IqImbalance {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        for s in x.iter_mut() {
            let z = Complex::new(f64::from(s.re), f64::from(s.im));
            let y = self.a * z + self.b * z.conj();
            s.re = y.re as f32;
            s.im = y.im as f32;
        }
    }
}

/// Additive DC offset, stated as a complex fraction of the waveform's RMS — the residual a
/// zero-IF front end leaves after imperfect DC cancellation.
#[derive(Clone, Copy, Debug)]
pub struct DcOffset {
    relative: Complex<f64>,
}

impl DcOffset {
    #[must_use]
    pub fn new(relative: Complex<f64>) -> Self {
        Self { relative }
    }
}

impl Impairment for DcOffset {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let c = self.relative * rms(x);
        for s in x.iter_mut() {
            s.re += c.re as f32;
            s.im += c.im as f32;
        }
    }
}

/// Hard magnitude limiting at `overdrive_db` above the waveform RMS, phase preserved — a
/// saturating front end. 0 dB clips at the RMS itself; a typical linear stage is quoted by
/// how many dB of peak-to-RMS headroom it grants before this happens.
#[derive(Clone, Copy, Debug)]
pub struct Clipping {
    overdrive_db: f64,
}

impl Clipping {
    #[must_use]
    pub fn new(overdrive_db: f64) -> Self {
        Self { overdrive_db }
    }
}

impl Impairment for Clipping {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let limit = rms(x) * 10f64.powf(self.overdrive_db / 20.0);
        for s in x.iter_mut() {
            let mag = f64::from(s.re).hypot(f64::from(s.im));
            if mag > limit {
                let scale = (limit / mag) as f32;
                s.re *= scale;
                s.im *= scale;
            }
        }
    }
}

/// Mid-rise uniform quantisation of I and Q to `bits` bits, full scale set `full_scale_db`
/// above the waveform RMS. Values beyond full scale saturate at the outermost level — an ADC
/// clips, it does not wrap.
#[derive(Clone, Copy, Debug)]
pub struct Quantiser {
    bits: u32,
    full_scale_db: f64,
}

impl Quantiser {
    #[must_use]
    pub fn new(bits: u32, full_scale_db: f64) -> Self {
        debug_assert!((1..=32).contains(&bits));
        Self {
            bits,
            full_scale_db,
        }
    }
}

impl Impairment for Quantiser {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let full_scale = rms(x) * 10f64.powf(self.full_scale_db / 20.0);
        if full_scale <= 0.0 {
            return;
        }
        let levels = 2u64.pow(self.bits.min(32)) as f64;
        let delta = 2.0 * full_scale / levels;
        let top = levels as i64 / 2 - 1;
        let bottom = -(levels as i64) / 2;
        let q = |v: f32| -> f32 {
            let idx = (f64::from(v) / delta).floor() as i64;
            let idx = idx.clamp(bottom, top);
            ((idx as f64 + 0.5) * delta) as f32
        };
        for s in x.iter_mut() {
            s.re = q(s.re);
            s.im = q(s.im);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, f64::consts::TAU};

    use num_complex::Complex;

    use super::{Clipping, DcOffset, IqImbalance, Quantiser};
    use crate::ber::{
        impair::{
            Impairment, mean_power, rms,
            testutil::{tone, white},
        },
        rng::Rng,
    };

    /// Applied == measured: the image-rejection ratio measured on a tone (signal bin vs its
    /// mirror) matches the closed form from the constructed a, b. The tone frequency is an
    /// exact DFT bin so the two projections are orthogonal.
    #[test]
    fn image_rejection_measured_on_a_tone_matches_closed_form() {
        let n = 4096usize;
        let f = 451.0 / n as f64;
        for (gain_db, phase_deg) in [(1.0, 5.0), (0.2, 1.0), (-0.5, -2.0)] {
            let applied = IqImbalance::new(gain_db, phase_deg);
            let mut y = tone(f, n);
            applied.apply(&mut y, &mut Rng::new(0));
            let project = |sign: f64| -> f64 {
                let mut acc = Complex::new(0.0f64, 0.0);
                for (i, s) in y.iter().enumerate() {
                    let ph = TAU * f * i as f64 * sign;
                    let (sin, cos) = ph.sin_cos();
                    acc += Complex::new(f64::from(s.re), f64::from(s.im)) * Complex::new(cos, sin);
                }
                acc.norm_sqr()
            };
            let measured = 10.0 * (project(-1.0) / project(1.0)).log10();
            let closed = applied.image_rejection_db();
            assert!(
                (measured - closed).abs() < 0.5,
                "gain {gain_db} dB / phase {phase_deg}°: closed {closed} dB, measured {measured} dB"
            );
        }
    }

    /// Applied == measured: the mean of the offset waveform is the stated fraction of the
    /// original RMS.
    #[test]
    fn dc_offset_measured_as_the_mean() {
        let applied = Complex::new(0.1, -0.05);
        let x = white(&mut Rng::new(0xdc), 200_000);
        let reference = rms(&x);
        let mut y = x.clone();
        DcOffset::new(applied).apply(&mut y, &mut Rng::new(0));
        let n = y.len() as f64;
        let mean = Complex::new(
            y.iter().map(|s| f64::from(s.re)).sum::<f64>() / n,
            y.iter().map(|s| f64::from(s.im)).sum::<f64>() / n,
        );
        let err = (mean / reference - applied).norm();
        assert!(
            err < 0.01,
            "applied {applied}, measured {}",
            mean / reference
        );
    }

    /// Applied == measured: the maximum output magnitude is the constructed limit — reached,
    /// because Gaussian input has peaks past 3 dB above RMS in any 100k samples.
    #[test]
    fn clipping_bounds_the_peak_at_the_stated_overdrive() {
        let overdrive_db = 3.0;
        let mut x = white(&mut Rng::new(0xc119), 100_000);
        let limit = rms(&x) * 10f64.powf(overdrive_db / 20.0);
        Clipping::new(overdrive_db).apply(&mut x, &mut Rng::new(0));
        let max = x
            .iter()
            .map(|s| f64::from(s.re).hypot(f64::from(s.im)))
            .fold(0.0f64, f64::max);
        assert!(max <= limit * 1.0001, "max {max} above limit {limit}");
        assert!(
            max >= limit * 0.999,
            "nothing reached the limit: {max} vs {limit}"
        );
    }

    /// Applied == measured, structurally: N bits produce at most 2^N distinct values per
    /// component, even with input peaks beyond full scale.
    #[test]
    fn quantiser_level_count_is_bounded_by_bits() {
        let bits = 4;
        let mut x = white(&mut Rng::new(0x9a7), 50_000);
        Quantiser::new(bits, 3.0).apply(&mut x, &mut Rng::new(0));
        let re_levels: BTreeSet<u32> = x.iter().map(|s| s.re.to_bits()).collect();
        let im_levels: BTreeSet<u32> = x.iter().map(|s| s.im.to_bits()).collect();
        assert!(re_levels.len() <= 1 << bits, "{} I levels", re_levels.len());
        assert!(im_levels.len() <= 1 << bits, "{} Q levels", im_levels.len());
    }

    /// Applied == measured against the classic 6.02·N + 1.76 dB: SQNR of a full-scale
    /// complex tone through the 8-bit quantiser. The formula assumes the quantisation error
    /// is uniform, which a non-bin-aligned tone approximates — hence the loose ±2.5 dB gate.
    #[test]
    fn quantiser_sqnr_matches_six_db_per_bit() {
        let bits = 8;
        let x = tone(0.123_456_7, 100_000);
        let mut y = x.clone();
        Quantiser::new(bits, 0.0).apply(&mut y, &mut Rng::new(0));
        let err: Vec<Complex<f32>> = y.iter().zip(&x).map(|(a, b)| a - b).collect();
        let sqnr = 10.0 * (mean_power(&x) / mean_power(&err)).log10();
        let expected = 6.02 * f64::from(bits) + 1.76;
        assert!(
            (sqnr - expected).abs() < 2.5,
            "expected ~{expected} dB, measured {sqnr} dB"
        );
    }
}
