use num_complex::Complex;

use crate::fir::{DelayLine, Isa, design_lowpass, plane_dot, plane_interpolated};

const PHASES: usize = 128;
const TAP_MULTIPLE: usize = 8;
const MAX_EXACT_PHASES: usize = 512;
const MAX_EXACT_TAPS: usize = 16_384;
const BATCH: usize = 4;

pub(crate) fn taps_per_phase(ratio: f64) -> usize {
    taps_for(0.1 * ratio.min(1.0))
}

fn taps_for(transition: f64) -> usize {
    (5.5 / transition).ceil() as usize
}

#[derive(Clone, Debug)]
enum Clock {
    Exact {
        phases: usize,
        whole: usize,
        rest: usize,
        phase: usize,
    },
    Free {
        step: f64,
        fraction: f64,
    },
}

impl Clock {
    fn for_ratio(ratio: f64, taps_per_phase: usize) -> Self {
        let step = ratio.recip();
        let exact = (1..=MAX_EXACT_PHASES)
            .take_while(|phases| phases * taps_per_phase <= MAX_EXACT_TAPS)
            .find_map(|phases| {
                let advance = (phases as f64 * step).round();
                let exact = (advance - phases as f64 * step).abs() <= 1e-9 * advance.max(1.0);
                (exact && advance >= 1.0).then_some((phases, advance as usize))
            });
        match exact {
            Some((phases, advance)) => Self::Exact {
                phases,
                whole: advance / phases,
                rest: advance % phases,
                phase: 0,
            },
            None => Self::Free {
                step,
                fraction: 0.0,
            },
        }
    }

    fn phases(&self) -> usize {
        match self {
            Self::Exact { phases, .. } => *phases,
            Self::Free { .. } => PHASES,
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Exact { phase, .. } => *phase = 0,
            Self::Free { fraction, .. } => *fraction = 0.0,
        }
    }
}

struct Sequence<I> {
    ticks: usize,
    stride: usize,
    outputs: usize,
    slots: I,
}

#[derive(Clone, Debug)]
pub struct FracResampler {
    rows: Vec<f32>,
    slopes: Vec<f32>,
    taps_per_phase: usize,
    clock: Clock,
    position: usize,
    re: DelayLine<f32>,
    im: DelayLine<f32>,
    isa: Isa,
}

impl FracResampler {
    #[must_use]
    pub fn new(ratio: f64) -> Self {
        assert!(ratio.is_finite() && ratio > 0.0, "ratio must be positive");
        Self::design(ratio, 0.45 * ratio.min(1.0), taps_per_phase(ratio))
    }

    #[must_use]
    pub fn keeping(ratio: f64, keep: f64) -> Self {
        assert!(ratio.is_finite() && ratio > 0.0, "ratio must be positive");
        let narrower = keep.is_finite() && keep > 0.0 && keep < 1.0;
        if ratio >= 1.0 || !narrower {
            return Self::new(ratio);
        }
        let taps = taps_for(ratio * (1.0 - keep));
        if taps >= taps_per_phase(ratio) {
            return Self::new(ratio);
        }
        Self::design(ratio, 0.5 * ratio, taps)
    }

    fn design(ratio: f64, cutoff: f64, taps: usize) -> Self {
        let taps_per_phase = taps.next_multiple_of(TAP_MULTIPLE);
        let clock = Clock::for_ratio(ratio, taps_per_phase);
        let phases = clock.phases();
        let proto = design_lowpass(phases * taps + 1, cutoff / phases as f64);
        let mut rows = vec![0.0f32; (phases + 1) * taps_per_phase];
        for (p, row) in rows.chunks_exact_mut(taps_per_phase).enumerate() {
            let branch = &mut row[taps_per_phase - taps..];
            for (j, slot) in branch.iter_mut().enumerate() {
                *slot = proto[j * phases + p];
            }
            let sum: f32 = branch.iter().sum();
            debug_assert!(sum > 0.0, "degenerate polyphase branch");
            for v in branch.iter_mut() {
                *v /= sum;
            }
            branch.reverse();
        }
        let slopes = match clock {
            Clock::Exact { .. } => Vec::new(),
            Clock::Free { .. } => rows[taps_per_phase..]
                .iter()
                .zip(&rows)
                .map(|(upper, lower)| upper - lower)
                .collect(),
        };
        Self {
            rows,
            slopes,
            taps_per_phase,
            clock,
            position: taps_per_phase - 1,
            re: DelayLine::new(taps_per_phase - 1, 1, 0),
            im: DelayLine::new(taps_per_phase - 1, 1, 0),
            isa: Isa::detect(),
        }
    }

    pub fn reset(&mut self) {
        self.position = self.taps_per_phase - 1;
        self.clock.reset();
        self.re.reset();
        self.im.reset();
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        for chunk in input.chunks(self.re.room()) {
            self.re.push_with(chunk, |sample| sample.re);
            self.im.push_with(chunk, |sample| sample.im);
            match self.clock {
                Clock::Exact { .. } => self.emit_exact(out),
                Clock::Free { .. } => self.emit_free(out),
            }
            let tpp = self.taps_per_phase;
            let consumed = self.position.saturating_sub(tpp - 1).min(self.re.len());
            self.re.consume(consumed);
            self.im.consume(consumed);
            self.position -= consumed;
        }
    }

    fn emit_exact(&mut self, out: &mut Vec<Complex<f32>>) {
        let Clock::Exact {
            phases,
            whole,
            rest,
            phase,
        } = self.clock
        else {
            return;
        };
        let advance = whole * phases + rest;
        let ticks = self.position * phases + phase;
        let count = (self.re.len() * phases)
            .saturating_sub(ticks)
            .div_ceil(advance);
        let first = out.len();
        out.resize(first + count, Complex::new(0.0, 0.0));
        for lead in 0..phases.min(count) {
            let mut sequence = Sequence {
                ticks: ticks + lead * advance,
                stride: advance,
                outputs: (count - lead).div_ceil(phases),
                slots: out[first + lead..].iter_mut().step_by(phases),
            };
            self.fill::<BATCH>(&mut sequence, phases);
            self.fill::<1>(&mut sequence, phases);
        }
        let ticks = ticks + count * advance;
        self.position = ticks / phases;
        self.clock = Clock::Exact {
            phases,
            whole,
            rest,
            phase: ticks % phases,
        };
    }

    fn fill<'a, const N: usize>(
        &self,
        sequence: &mut Sequence<impl Iterator<Item = &'a mut Complex<f32>>>,
        phases: usize,
    ) {
        let tpp = self.taps_per_phase;
        let row = sequence.ticks % phases;
        let taps = &self.rows[row * tpp..(row + 1) * tpp];
        let (re, im) = (self.re.rows_from(0), self.im.rows_from(0));
        while sequence.outputs >= N {
            let window = sequence.ticks / phases + 1 - tpp;
            let starts: [usize; N] = std::array::from_fn(|index| window + index * sequence.stride);
            let values = plane_dot(self.isa, re, im, starts, taps);
            for (value, slot) in values.into_iter().zip(sequence.slots.by_ref()) {
                *slot = value;
            }
            sequence.ticks += N * sequence.stride * phases;
            sequence.outputs -= N;
        }
    }

    fn emit_free(&mut self, out: &mut Vec<Complex<f32>>) {
        let Clock::Free {
            step,
            ref mut fraction,
        } = self.clock
        else {
            return;
        };
        let tpp = self.taps_per_phase;
        while self.position < self.re.len() {
            let phase = *fraction * PHASES as f64;
            let row = phase as usize;
            let mu = (phase - row as f64) as f32;
            let taps = row * tpp..(row + 1) * tpp;
            let window = self.position + 1 - tpp;
            out.push(plane_interpolated(
                self.isa,
                &self.re.rows_from(window)[..tpp],
                &self.im.rows_from(window)[..tpp],
                &self.rows[taps.clone()],
                &self.slopes[taps],
                mu,
            ));
            *fraction += step;
            let advance = *fraction as usize;
            self.position += advance;
            *fraction -= advance as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{complex_tone, rms_c, tone_peak_and_snr};

    #[test]
    fn ragged_resampling_matches_double_precision_convolution() {
        let input: Vec<_> = (0..16_381)
            .map(|index| {
                Complex::new(
                    ((index * 37) % 251) as f32 / 251.0 - 0.5,
                    ((index * 71) % 257) as f32 / 257.0 - 0.5,
                )
            })
            .collect();
        for ratio in [
            0.2,
            0.75,
            0.768,
            0.96,
            48_000.0 / 44_100.0,
            1.2,
            0.8736,
            0.618_033_988_7,
            1.618_033_988_7,
        ] {
            let mut resampler = FracResampler::new(ratio);
            let taps = resampler.taps_per_phase;
            let mut history = vec![Complex::new(0.0, 0.0); taps - 1];
            history.extend_from_slice(&input);
            let mut expected = Vec::new();
            for (sample, row, fraction) in schedule(&resampler.clock, taps - 1, history.len()) {
                let mut sum = Complex::new(0.0f64, 0.0);
                for (index, value) in history[sample + 1 - taps..=sample].iter().enumerate() {
                    let a = f64::from(resampler.rows[row * taps + index]);
                    let b = f64::from(resampler.rows[(row + 1) * taps + index]);
                    sum += Complex::new(f64::from(value.re), f64::from(value.im))
                        * (a + (b - a) * fraction);
                }
                expected.push(sum);
            }
            let mut actual = Vec::new();
            let mut block = Vec::new();
            for input in input.chunks(997) {
                resampler.process(input, &mut block);
                actual.extend_from_slice(&block);
            }
            assert_eq!(actual.len(), expected.len(), "ratio={ratio}");
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                let actual = Complex::new(f64::from(actual.re), f64::from(actual.im));
                assert!(
                    (actual - expected).norm() < 1e-6,
                    "ratio={ratio} sample={index}"
                );
            }
        }
    }

    fn schedule(clock: &Clock, first: usize, len: usize) -> Vec<(usize, usize, f64)> {
        match *clock {
            Clock::Exact {
                phases,
                whole,
                rest,
                ..
            } => (0..)
                .map(|index: usize| index * (whole * phases + rest))
                .map(|ticks| (first + ticks / phases, ticks % phases, 0.0))
                .take_while(|&(sample, ..)| sample < len)
                .collect(),
            Clock::Free { step, .. } => (0..)
                .map(|index| first as f64 + index as f64 * step)
                .take_while(|&time| (time as usize) < len)
                .map(|time| {
                    let phase = time.fract() * PHASES as f64;
                    (time as usize, phase as usize, phase.fract())
                })
                .collect(),
        }
    }

    #[test]
    fn small_rational_ratios_run_on_an_exact_clock() {
        for (ratio, phases) in [(48_000.0 / 44_100.0, 160), (0.2, 1), (0.75, 3), (1.2, 6)] {
            let clock = FracResampler::new(ratio).clock;
            assert!(
                matches!(clock, Clock::Exact { phases: p, .. } if p == phases),
                "{ratio}: {clock:?}"
            );
        }
        for ratio in [0.8736, 0.618_033_988_7] {
            let clock = FracResampler::new(ratio).clock;
            assert!(matches!(clock, Clock::Free { .. }), "{ratio}: {clock:?}");
        }
    }

    #[test]
    fn out_of_band_tone_at_5x_downsample_suppressed_over_50_db() {
        let mut r = FracResampler::new(48_000.0 / 240_000.0);
        let input = complex_tone(30_000.0 / 240_000.0, 5 * 4096);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let rms = rms_c(&out[512..]);
        assert!(rms < 3.16e-3, "alias leak rms {rms}");
    }

    #[test]
    fn downsample_5x_keeps_frequency_and_snr() {
        let mut r = FracResampler::new(48_000.0 / 240_000.0);
        let input = complex_tone(100.0 / (4096.0 * 5.0), 5 * 4096 + 1024);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let (peak, snr) = tone_peak_and_snr(&out[64..64 + 4096]);
        assert_eq!(peak, 100, "output frequency shifted");
        assert!(snr > 40.0, "snr {snr} dB");
    }

    #[test]
    fn awkward_ratio_44100_to_48000_keeps_frequency_and_snr() {
        let mut r = FracResampler::new(48_000.0 / 44_100.0);
        let input = complex_tone(1500.0 / 44_100.0, 4400);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let (peak, snr) = tone_peak_and_snr(&out[64..64 + 4096]);
        assert_eq!(peak, 128, "output frequency shifted");
        assert!(snr > 40.0, "snr {snr} dB");
    }

    #[test]
    fn long_run_output_count_matches_ratio() {
        for (ratio, total_in, ideal_out, block) in [
            (48_000.0 / 240_000.0, 1_200_000usize, 240_000i64, 7_777usize),
            (48_000.0 / 44_100.0, 441_000, 480_000, 9_999),
        ] {
            let mut r = FracResampler::new(ratio);
            let input = complex_tone(0.01, total_in);
            let mut out = Vec::new();
            let mut count = 0i64;
            for chunk in input.chunks(block) {
                r.process(chunk, &mut out);
                count += out.len() as i64;
            }
            assert!(
                (count - ideal_out).abs() <= 2,
                "ratio {ratio}: got {count}, ideal {ideal_out}"
            );
        }
    }

    #[test]
    fn a_kept_band_stays_clean_with_fewer_taps() {
        let dab = 2_048_000.0 / 2_400_000.0;
        let kept = FracResampler::keeping(dab, 1_536_000.0 / 2_048_000.0);
        assert!(kept.taps_per_phase * 2 < FracResampler::new(dab).taps_per_phase);
        let (ratio, keep) = (0.6, 0.75);
        let mut r = FracResampler::keeping(ratio, keep);
        let inside = complex_tone(1_228.0 / 4_096.0 * ratio, 8 * 4096);
        let mut out = Vec::new();
        r.process(&inside, &mut out);
        let (peak, snr) = tone_peak_and_snr(&out[256..256 + 4096]);
        assert_eq!(peak, 1_228, "output frequency shifted");
        assert!(snr > 40.0, "kept band snr {snr} dB");
        let mut r = FracResampler::keeping(ratio, keep);
        let folding = complex_tone(0.45, 8 * 4096);
        r.process(&folding, &mut out);
        let rms = rms_c(&out[256..]);
        assert!(
            rms < 3.16e-3,
            "a tone folding into the kept band leaks at rms {rms}"
        );
    }

    #[test]
    fn keeping_the_whole_band_is_the_plain_resampler() {
        let ratio = 0.853;
        assert_eq!(
            FracResampler::keeping(ratio, 0.95).taps_per_phase,
            FracResampler::new(ratio).taps_per_phase
        );
        for (ratio, keep) in [(1.2, 0.5), (0.8, 1.5), (0.8, -0.1), (0.8, f64::NAN)] {
            assert_eq!(
                FracResampler::keeping(ratio, keep).taps_per_phase,
                FracResampler::new(ratio).taps_per_phase,
                "ratio {ratio} keep {keep}"
            );
        }
    }

    #[test]
    fn ragged_blocks_match_one_shot() {
        let input = complex_tone(0.021, 30_000);
        let mut whole = FracResampler::new(48_000.0 / 44_100.0);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = FracResampler::new(48_000.0 / 44_100.0);
        let mut got = Vec::new();
        let mut block = Vec::new();
        let mut pos = 0;
        for len in [1usize, 7, 64, 3, 129, 1024, 17].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            ragged.process(&input[pos..end], &mut block);
            got.extend_from_slice(&block);
            pos = end;
        }
        assert_eq!(expected.len(), got.len());
        for (i, (a, b)) in expected.iter().zip(&got).enumerate() {
            assert_eq!(a, b, "sample {i}");
        }
    }

    #[test]
    fn reset_leaves_the_state_a_fresh_resampler_would_have() {
        let ratio = 48_000.0 / 8_000.0;
        let input = complex_tone(0.037, 4_000);

        let mut fresh = FracResampler::new(ratio);
        let mut expected = Vec::new();
        fresh.process(&input, &mut expected);

        let mut reused = FracResampler::new(ratio);
        let mut scratch = Vec::new();
        reused.process(&complex_tone(0.11, 2_500), &mut scratch);
        reused.reset();
        let mut got = Vec::new();
        reused.process(&input, &mut got);

        assert_eq!(expected.len(), got.len(), "reset shifted the output phase");
        for (i, (a, b)) in expected.iter().zip(&got).enumerate() {
            assert_eq!(a, b, "sample {i}: fresh {a}, reset {b}");
        }
    }
}
