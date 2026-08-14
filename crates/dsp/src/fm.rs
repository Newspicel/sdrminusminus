use std::f64::consts::TAU;

use num_complex::Complex;

/// `y = arg(x[n]·conj(x[n−1])) · rate / (2π·deviation)`, so ±deviation reads as ±1.0.
#[derive(Clone, Debug)]
pub struct FmDemod {
    prev: Complex<f32>,
    scale: f32,
    primed: bool,
}

impl FmDemod {
    #[must_use]
    pub fn new(rate: f64, deviation_hz: f64) -> Self {
        assert!(
            rate > 0.0 && deviation_hz > 0.0,
            "rate and deviation must be positive"
        );
        Self {
            prev: Complex::new(1.0, 0.0),
            scale: (rate / (TAU * deviation_hz)) as f32,
            primed: false,
        }
    }

    /// Replaces `out` with one demodulated sample per input sample. The phase reference
    /// seeds from the first sample ever seen, so the first output after construction is
    /// exactly 0.0 instead of an arbitrary-phase impulse into the audio chain.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        out.clear();
        if !self.primed
            && let Some(&first) = iq.first()
        {
            self.prev = first;
            self.primed = true;
        }
        for &x in iq {
            out.push((x * self.prev.conj()).arg() * self.scale);
            self.prev = x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_tone_demodulates_to_unit_amplitude() {
        let rate = 48_000.0;
        let (f_mod, deviation) = (1_000.0, 2_500.0);
        let n = 48_000;

        // Phase-accumulated FM: increment d[k] = 2π·dev·cos(2π·f_mod·k/rate)/rate, so the
        // discriminator must return exactly cos(2π·f_mod·k/rate).
        let mut phase = 0.0f64;
        let iq: Vec<Complex<f32>> = (0..n)
            .map(|k| {
                phase += TAU * deviation * (TAU * f_mod * k as f64 / rate).cos() / rate;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();

        let mut demod = FmDemod::new(rate, deviation);
        let mut out = Vec::new();
        demod.process(&iq, &mut out);

        assert_eq!(out[0], 0.0);
        let y = &out[1..];
        let rms =
            (y.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / y.len() as f64).sqrt();
        let amplitude = rms * std::f64::consts::SQRT_2;
        assert!((0.95..1.05).contains(&amplitude), "amplitude {amplitude}");

        for (k, &v) in y.iter().enumerate() {
            let expected = (TAU * f_mod * (k + 1) as f64 / rate).cos() as f32;
            assert!((v - expected).abs() < 0.02, "sample {k}: {v} vs {expected}");
        }
    }

    #[test]
    fn first_output_is_zero_for_any_carrier_phase() {
        let mut demod = FmDemod::new(48_000.0, 2_500.0);
        let mut out = Vec::new();
        // An empty call must not consume the seed.
        demod.process(&[], &mut out);

        // An unmodulated carrier at an arbitrary phase must demodulate to silence from
        // sample 0 — no startup impulse from the (1, 0) construction reference.
        let iq: Vec<Complex<f32>> = (0..64).map(|_| Complex::from_polar(1.0, 3.1)).collect();
        demod.process(&iq, &mut out);
        assert_eq!(out.len(), iq.len());
        assert_eq!(out[0], 0.0);
        for (k, &v) in out.iter().enumerate() {
            assert!(v.abs() < 1e-4, "sample {k}: startup impulse {v}");
        }
    }
}
