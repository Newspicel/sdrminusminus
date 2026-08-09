//! Digital down-converter (PLAN §7): NCO mix by −offset → integer polyphase decimation
//! stages → fractional resampler for the residual ratio, so the output rate is exact.
//!
//! Every stage's anti-alias filter protects the full output Nyquist band, not just its own —
//! whatever survives a stage can never fold into the final passband with less than the
//! design stopband (~74 dB Blackman, spec floor 50 dB).

use num_complex::Complex;

use crate::{Decimator, FracResampler, Nco, fir::design_lowpass};

/// Flat passband each side of DC as a fraction of the output rate (80% total).
const PASSBAND_FRAC: f64 = 0.4;
/// Alias-protected band each side of DC as a fraction of the output rate.
const PROTECT_FRAC: f64 = 0.5;

/// Widest signal, in Hz, a *resampling* DDC can deliver at `output_rate`.
///
/// Every rate conversion needs somewhere to put its filter transition: the band between the
/// flat passband and the protected edge. A channel that occupies the full output rate leaves
/// no room for it and can only be served by a transparent DDC — one whose input rate already
/// equals its output rate. Callers must refuse such a channel on any other device rate rather
/// than hand its decoder a smeared signal that silently decodes nothing.
#[must_use]
pub fn resamplable_bandwidth_hz(output_rate: f64) -> f64 {
    2.0 * PROTECT_FRAC * output_rate
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DdcError {
    #[error("rates must be positive and finite (input {input} Hz, output {output} Hz)")]
    InvalidRates { input: f64, output: f64 },
    #[error("output rate {output} Hz exceeds input rate {input} Hz")]
    OutputAboveInput { input: f64, output: f64 },
}

/// One channel's front end: complex input at the device rate in, complex baseband at exactly
/// `output_rate` out.
#[derive(Clone, Debug)]
pub struct Ddc {
    input_rate: f64,
    nco: Nco,
    stages: Vec<Decimator>,
    resamp: Option<FracResampler>,
    work_in: Vec<Complex<f32>>,
    work_out: Vec<Complex<f32>>,
}

impl Ddc {
    /// `offset_hz` is where the wanted channel sits relative to the device center frequency.
    pub fn new(input_rate: f64, output_rate: f64, offset_hz: f64) -> Result<Self, DdcError> {
        if !input_rate.is_finite()
            || !output_rate.is_finite()
            || input_rate <= 0.0
            || output_rate <= 0.0
        {
            return Err(DdcError::InvalidRates {
                input: input_rate,
                output: output_rate,
            });
        }
        if output_rate > input_rate {
            return Err(DdcError::OutputAboveInput {
                input: input_rate,
                output: output_rate,
            });
        }

        let mut stages = Vec::new();
        let mut rate = input_rate;
        for factor in prime_factors_desc(integer_decimation(input_rate / output_rate)) {
            stages.push(stage(rate, factor, output_rate));
            rate /= factor as f64;
        }
        let ratio = output_rate / rate;
        // An exact integer chain needs no fractional stage — skipping it also keeps the
        // interpolation kernel's rolloff out of the passband entirely.
        let resamp = ((ratio - 1.0).abs() > 1e-12).then(|| FracResampler::new(ratio));

        Ok(Self {
            input_rate,
            nco: Nco::new((-offset_hz) as f32, input_rate as f32),
            stages,
            resamp,
            work_in: Vec::new(),
            work_out: Vec::new(),
        })
    }

    /// Retunes the mixer only — phase-continuous, no filter state reset, cheap.
    pub fn set_offset(&mut self, offset_hz: f64) {
        self.nco
            .set_freq((-offset_hz) as f32, self.input_rate as f32);
    }

    /// Replaces `out` with the baseband samples fully computable from history + `input`.
    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let nco = &mut self.nco;
        self.work_in.clear();
        self.work_in
            .extend(input.iter().map(|&s| s * nco.next_sample()));
        for stage in &mut self.stages {
            stage.process(&self.work_in, &mut self.work_out);
            std::mem::swap(&mut self.work_in, &mut self.work_out);
        }
        match &mut self.resamp {
            Some(r) => r.process(&self.work_in, out),
            None => {
                out.clear();
                out.extend_from_slice(&self.work_in);
            }
        }
    }
}

/// Largest integer decimation that keeps the intermediate rate at or above the output rate,
/// guarded against `41.999…` float quotients picking an off-by-one factor.
fn integer_decimation(quotient: f64) -> usize {
    let rounded = quotient.round();
    if (quotient - rounded).abs() < 1e-9 {
        rounded as usize
    } else {
        quotient.floor() as usize
    }
}

/// Descending prime factors: big cheap stages first at the high rate, so the tight final
/// filter runs at the lowest possible rate.
fn prime_factors_desc(mut n: usize) -> Vec<usize> {
    let mut factors = Vec::new();
    let mut d = 2;
    while d * d <= n {
        while n.is_multiple_of(d) {
            factors.push(d);
            n /= d;
        }
        d += 1;
    }
    if n > 1 {
        factors.push(n);
    }
    factors.reverse();
    factors
}

fn stage(input_rate: f64, factor: usize, output_rate: f64) -> Decimator {
    let stage_out = input_rate / factor as f64;
    let pass = PASSBAND_FRAC * output_rate / input_rate;
    let stop = (stage_out - PROTECT_FRAC * output_rate) / input_rate;
    // Blackman transition width is 5.5/taps: size each stage for exactly the transition it
    // needs, so early wide-band stages stay short.
    let taps = (((5.5 / (stop - pass)).ceil() as usize) | 1).max(11);
    Decimator::new(&design_lowpass(taps, (pass + stop) / 2.0), factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::rms_c;

    const FS_IN: f64 = 2_048_000.0;
    const FS_OUT: f64 = 48_000.0;
    const BLOCK: usize = 16_384;

    fn tone_at_rate(freq_hz: f64, rate: f64, len: usize) -> Vec<Complex<f32>> {
        let mut nco = Nco::new(freq_hz as f32, rate as f32);
        (0..len).map(|_| nco.next_sample()).collect()
    }

    fn tone(freq_hz: f64, len: usize) -> Vec<Complex<f32>> {
        tone_at_rate(freq_hz, FS_IN, len)
    }

    fn run(ddc: &mut Ddc, input: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        let mut collected = Vec::new();
        for chunk in input.chunks(BLOCK) {
            ddc.process(chunk, &mut out);
            collected.extend_from_slice(&out);
        }
        collected
    }

    fn mean_freq_hz(out: &[Complex<f32>], rate: f64) -> f64 {
        let mut sum = 0.0f64;
        for pair in out.windows(2) {
            sum += f64::from((pair[1] * pair[0].conj()).arg());
        }
        sum / (out.len() - 1) as f64 * rate / std::f64::consts::TAU
    }

    #[test]
    fn rejects_output_rate_above_input() {
        assert!(matches!(
            Ddc::new(48_000.0, 2_048_000.0, 0.0),
            Err(DdcError::OutputAboveInput { .. })
        ));
        assert!(matches!(
            Ddc::new(f64::NAN, 48_000.0, 0.0),
            Err(DdcError::InvalidRates { .. })
        ));
    }

    #[test]
    fn tone_at_offset_lands_at_dc() {
        let offset = 400_000.0;
        let mut ddc = Ddc::new(FS_IN, FS_OUT, offset).unwrap();
        let collected = run(&mut ddc, &tone(offset, 262_144));
        let settled = &collected[512..];
        for (i, y) in settled.iter().enumerate() {
            let mag = y.norm();
            assert!((0.97..1.03).contains(&mag), "sample {i}: |y| = {mag}");
        }
        let freq = mean_freq_hz(settled, FS_OUT);
        assert!(freq.abs() < 2.0, "residual frequency {freq} Hz");
    }

    #[test]
    fn tone_1_2x_output_rate_away_suppressed_over_50_db() {
        let offset = 400_000.0;
        let mut ddc = Ddc::new(FS_IN, FS_OUT, offset).unwrap();
        let collected = run(&mut ddc, &tone(offset + 1.2 * FS_OUT, 262_144));
        let rms = rms_c(&collected[512..]);
        assert!(rms < 3.16e-3, "leak rms {rms}");
    }

    #[test]
    fn quotient_below_two_still_suppresses_folding_blockers_over_50_db() {
        // Quotients in (1, 2) plan zero integer stages, so the fractional kernel alone must
        // hold the ≥50 dB floor. 460k→240k: +145 kHz folds to −95 kHz inside the flat
        // ±96 kHz passband; 76.8k→48k: +29 kHz folds to −19 kHz inside ±19.2 kHz.
        for (fs_in, fs_out, blocker_hz) in [
            (460_000.0, 240_000.0, 145_000.0),
            (76_800.0, 48_000.0, 29_000.0),
        ] {
            let mut ddc = Ddc::new(fs_in, fs_out, 0.0).unwrap();
            let collected = run(&mut ddc, &tone_at_rate(blocker_hz, fs_in, 262_144));
            let rms = rms_c(&collected[512..]);
            assert!(rms < 3.16e-3, "{fs_in}→{fs_out}: blocker leak rms {rms}");

            let mut ddc = Ddc::new(fs_in, fs_out, 0.0).unwrap();
            let inband = run(&mut ddc, &tone_at_rate(0.35 * fs_out, fs_in, 262_144));
            let rms = rms_c(&inband[512..]);
            assert!(
                (0.97..1.03).contains(&rms),
                "{fs_in}→{fs_out}: in-band rms {rms}"
            );
        }
    }

    #[test]
    fn exact_long_run_output_rate() {
        for (fs_in, fs_out) in [(2_048_000.0f64, 48_000.0f64), (2_400_000.0, 240_000.0)] {
            let mut ddc = Ddc::new(fs_in, fs_out, 0.0).unwrap();
            let total_in = fs_in as usize;
            let input = vec![Complex::new(1.0f32, 0.0); total_in];
            let mut out = Vec::new();
            let mut count = 0i64;
            for chunk in input.chunks(BLOCK) {
                ddc.process(chunk, &mut out);
                count += out.len() as i64;
            }
            let ideal = fs_out as i64;
            assert!(
                (count - ideal).abs() <= 2,
                "{fs_in}→{fs_out}: got {count} samples/s, ideal {ideal}"
            );
        }
    }

    #[test]
    fn set_offset_retunes_within_one_block() {
        let (f1, f2) = (300_000.0, -250_000.0);
        let mut ddc = Ddc::new(FS_IN, FS_OUT, f1).unwrap();
        let mut out = Vec::new();

        let phase1 = tone(f1, 20 * BLOCK);
        let mut settled = Vec::new();
        for (i, chunk) in phase1.chunks(BLOCK).enumerate() {
            ddc.process(chunk, &mut out);
            if i >= 1 {
                settled.extend_from_slice(&out);
            }
        }
        for y in &settled {
            assert!(
                (0.9..1.1).contains(&y.norm()),
                "pre-retune |y| = {}",
                y.norm()
            );
        }

        ddc.set_offset(f2);
        let phase2 = tone(f2, 20 * BLOCK);
        settled.clear();
        for (i, chunk) in phase2.chunks(BLOCK).enumerate() {
            ddc.process(chunk, &mut out);
            // Only the block straddling the retune may glitch.
            if i >= 1 {
                settled.extend_from_slice(&out);
            }
        }
        for y in &settled {
            assert!(
                (0.9..1.1).contains(&y.norm()),
                "post-retune |y| = {}",
                y.norm()
            );
        }
        let freq = mean_freq_hz(&settled, FS_OUT);
        assert!(freq.abs() < 2.0, "post-retune residual frequency {freq} Hz");
    }
}
