use num_complex::Complex;

use crate::{CubicInterpolator, Decimator, FracResampler, Nco, fir::design_lowpass};

const PASSBAND_FRAC: f64 = 0.4;
const PROTECT_FRAC: f64 = 0.5;

#[must_use]
pub fn flat_bandwidth_hz(output_rate: f64) -> f64 {
    2.0 * PASSBAND_FRAC * output_rate
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DdcError {
    #[error("rates must be positive and finite (input {input} Hz, output {output} Hz)")]
    InvalidRates { input: f64, output: f64 },
}

#[derive(Clone, Debug)]
enum Fraction {
    None,
    Down(FracResampler),
    Up(CubicInterpolator),
}

impl Fraction {
    fn for_ratio(ratio: f64) -> Self {
        if (ratio - 1.0).abs() <= 1e-12 {
            Self::None
        } else if ratio < 1.0 {
            Self::Down(FracResampler::new(ratio))
        } else {
            Self::Up(CubicInterpolator::new(ratio))
        }
    }

    fn reset(&mut self) {
        match self {
            Self::None => {}
            Self::Down(r) => r.reset(),
            Self::Up(r) => r.reset(),
        }
    }

    fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        match self {
            Self::None => {
                out.clear();
                out.extend_from_slice(input);
            }
            Self::Down(r) => r.process(input, out),
            Self::Up(r) => r.process(input, out),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Ddc {
    input_rate: f64,
    nco: Nco,
    stages: Vec<Decimator>,
    fraction: Fraction,
    work_in: Vec<Complex<f32>>,
    work_out: Vec<Complex<f32>>,
}

impl Ddc {
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

        let mut stages = Vec::new();
        let mut rate = input_rate;
        if output_rate < input_rate {
            for factor in prime_factors_desc(integer_decimation(input_rate / output_rate)) {
                stages.push(stage(rate, factor, output_rate));
                rate /= factor as f64;
            }
        }
        Ok(Self {
            input_rate,
            nco: Nco::new((-offset_hz) as f32, input_rate as f32),
            stages,
            fraction: Fraction::for_ratio(output_rate / rate),
            work_in: Vec::new(),
            work_out: Vec::new(),
        })
    }

    pub fn reset(&mut self) {
        self.nco.reset();
        for stage in &mut self.stages {
            stage.reset();
        }
        self.fraction.reset();
        self.work_in.clear();
        self.work_out.clear();
    }

    pub fn set_offset(&mut self, offset_hz: f64) {
        self.nco
            .set_freq((-offset_hz) as f32, self.input_rate as f32);
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.work_in.resize(input.len(), Complex::new(0.0, 0.0));
        self.nco.mix_into(input, &mut self.work_in);
        for stage in &mut self.stages {
            stage.process(&self.work_in, &mut self.work_out);
            std::mem::swap(&mut self.work_in, &mut self.work_out);
        }
        self.fraction.process(&self.work_in, out);
    }
}

fn integer_decimation(quotient: f64) -> usize {
    let rounded = quotient.round();
    let mut candidate = if (quotient - rounded).abs() < 1e-9 {
        rounded as usize
    } else {
        quotient.floor() as usize
    };
    let minimum = (candidate / 2).max(candidate.saturating_sub(256)).max(1);
    let mut best = 1usize << candidate.max(1).ilog2();
    let mut best_cost = decimation_cost(quotient, best);
    while candidate >= minimum {
        let mut remaining = candidate;
        for factor in [2, 3, 5, 7, 11, 13] {
            while remaining.is_multiple_of(factor) {
                remaining /= factor;
            }
        }
        if remaining == 1 {
            let cost = decimation_cost(quotient, candidate);
            if cost < best_cost {
                best = candidate;
                best_cost = cost;
            }
        }
        candidate -= 1;
    }
    best
}

fn decimation_cost(quotient: f64, decimation: usize) -> f64 {
    let mut rate = quotient;
    let mut cost = 0.0;
    for factor in prime_factors_desc(decimation) {
        let (taps, _) = stage_filter(rate, factor, 1.0);
        rate /= factor as f64;
        cost += taps as f64 * rate / quotient;
    }
    if (rate - 1.0).abs() > 1e-12 {
        cost += 2.0 * crate::resamp::taps_per_phase(rate.recip()) as f64 / quotient;
    }
    cost
}

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
    let (taps, cutoff) = stage_filter(input_rate, factor, output_rate);
    Decimator::new(&design_lowpass(taps, cutoff), factor)
}

fn stage_filter(input_rate: f64, factor: usize, output_rate: f64) -> (usize, f64) {
    let stage_out = input_rate / factor as f64;
    let pass = PASSBAND_FRAC * output_rate / input_rate;
    let stop = (stage_out - PROTECT_FRAC * output_rate) / input_rate;
    let taps = (((5.5 / (stop - pass)).ceil() as usize) | 1).max(11);
    (taps, (pass + stop) / 2.0)
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
    fn reset_does_not_splice_old_filter_history_into_a_fresh_signal() {
        for (input_rate, output_rate) in [(240_000.0, 48_000.0), (240_000.0, 44_100.0)] {
            let mut used = Ddc::new(input_rate, output_rate, 1234.0).expect("rates");
            let mut fresh = used.clone();
            let mut actual = Vec::new();
            used.process(&vec![Complex::new(1.0, 0.5); 4001], &mut actual);
            used.reset();
            let signal = tone_at_rate(5678.0, input_rate, 4096);
            used.process(&signal, &mut actual);
            let mut expected = Vec::new();
            fresh.process(&signal, &mut expected);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn rejects_rates_that_are_not_positive_and_finite() {
        for (input, output) in [
            (f64::NAN, 48_000.0),
            (48_000.0, f64::INFINITY),
            (0.0, 48_000.0),
            (48_000.0, -1.0),
        ] {
            assert!(
                matches!(
                    Ddc::new(input, output, 0.0),
                    Err(DdcError::InvalidRates { .. })
                ),
                "{input}→{output}"
            );
        }
    }

    #[test]
    fn upsampling_keeps_the_tone_and_delivers_the_output_rate() {
        for (fs_in, fs_out) in [
            (2_000_000.0f64, 2_400_000.0f64),
            (2_048_000.0, 2_400_000.0),
            (2_400_000.0, 16_000_000.0),
        ] {
            let offset = 0.1 * fs_in;
            let mut ddc = Ddc::new(fs_in, fs_out, offset).unwrap();
            let total_in = (fs_in / 8.0) as usize;
            let collected = run(
                &mut ddc,
                &tone_at_rate(offset + 0.05 * fs_in, fs_in, total_in),
            );
            let ideal = (total_in as f64 * fs_out / fs_in) as i64;
            assert!(
                (collected.len() as i64 - ideal).abs() <= 2,
                "{fs_in}→{fs_out}: got {} S, ideal {ideal}",
                collected.len()
            );
            let settled = &collected[1024..];
            let rms = rms_c(settled);
            assert!((0.97..1.03).contains(&rms), "{fs_in}→{fs_out}: rms {rms}");
            let freq = mean_freq_hz(settled, fs_out);
            let want = 0.05 * fs_in;
            assert!(
                (freq - want).abs() < 0.001 * want,
                "{fs_in}→{fs_out}: tone at {freq} Hz, wanted {want} Hz"
            );
        }
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
        for (fs_in, fs_out) in [
            (2_048_000.0f64, 48_000.0f64),
            (2_400_000.0, 240_000.0),
            (20_000_000.0, 240_000.0),
            (19_920_000.0, 240_000.0),
        ] {
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
                "{fs_in}→{fs_out}: got {count} S/s, ideal {ideal}"
            );
        }
    }

    #[test]
    fn prime_ratios_keep_passband_and_reject_aliases() {
        for input_rate in [19_920_000.0, 20_000_000.0, 8_000_000.0, 3_200_000.0] {
            let output_rate = 240_000.0;
            let offset = 100_000.0;
            let intermediate = input_rate / integer_decimation(input_rate / output_rate) as f64;
            for relative in [
                0.0,
                0.35 * output_rate,
                -0.35 * output_rate,
                0.4 * output_rate,
                -0.4 * output_rate,
                0.6 * output_rate,
                -0.6 * output_rate,
                intermediate - 0.35 * output_rate,
                intermediate + 0.35 * output_rate,
                input_rate / 2.0 - offset - 1000.0,
            ] {
                let mut ddc = Ddc::new(input_rate, output_rate, offset).expect("rates");
                let input = tone_at_rate(offset + relative, input_rate, 262_144);
                let output = run(&mut ddc, &input);
                let settled = &output[512..];
                let rms = rms_c(settled);
                if relative.abs() <= 0.4 * output_rate {
                    assert!(
                        (0.97..1.03).contains(&rms),
                        "{input_rate} {relative}: passband {rms}"
                    );
                    assert!((mean_freq_hz(settled, output_rate) - relative).abs() < 3.0);
                } else {
                    assert!(rms < 3.16e-3, "{input_rate} {relative}: alias {rms}");
                }
            }
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
