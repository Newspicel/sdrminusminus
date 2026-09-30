use std::ops::Mul;

use super::{
    kernel::{Accumulate, Isa, Plan},
    line::DelayLine,
};

#[derive(Clone, Debug)]
pub(crate) struct StreamFir<T, C> {
    plan: Plan<C>,
    span: usize,
    isa: Isa,
    line: DelayLine<T>,
}

impl<T, C> StreamFir<T, C>
where
    T: Accumulate<C>,
    C: Copy + PartialEq + Mul<f32, Output = C>,
{
    pub(crate) fn new(taps: &[C], factor: usize) -> Self {
        Self::with_folding(taps, factor, true)
    }

    fn with_folding(taps: &[C], factor: usize, fold: bool) -> Self {
        assert!(!taps.is_empty(), "taps must not be empty");
        assert!(factor >= 1, "factor must be >= 1");
        assert!(factor <= taps.len(), "factor must not exceed the tap count");
        let line = DelayLine::new(taps.len() - 1, factor, T::LANES);
        Self {
            plan: Plan::new::<T>(taps, factor, line.stride(), fold),
            span: taps.len(),
            isa: Isa::detect(),
            line,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.line.reset();
    }

    pub(crate) fn process(&mut self, input: &[T], out: &mut Vec<T>) {
        out.clear();
        for chunk in input.chunks(self.line.room()) {
            self.line.push(chunk);
            self.emit(out);
        }
    }

    fn emit(&mut self, out: &mut Vec<T>) {
        let windows = self.line.windows(self.span);
        let mut done = 0;
        while done < windows {
            let count = (windows - done).min(T::LANES);
            let len = out.len();
            out.resize(len + count, T::zero());
            let rows = self.line.rows_from(done);
            if self.plan.folded() {
                T::block::<true>(self.isa, rows, &self.plan, &mut out[len..]);
            } else {
                T::block::<false>(self.isa, rows, &self.plan, &mut out[len..]);
            }
            done += count;
        }
        self.line.consume(windows);
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::fir::design_lowpass;

    fn signal(len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|index| {
                Complex::new(
                    ((index * 37) % 251) as f32 / 251.0 - 0.5,
                    ((index * 71) % 257) as f32 / 257.0 - 0.5,
                )
            })
            .collect()
    }

    fn reference<C: Copy>(
        input: &[Complex<f32>],
        taps: &[C],
        factor: usize,
        widen: impl Fn(C) -> Complex<f64>,
    ) -> Vec<Complex<f64>> {
        let mut history = vec![Complex::new(0.0, 0.0); taps.len() - 1];
        history.extend_from_slice(input);
        (0..=history.len() - taps.len())
            .step_by(factor)
            .map(|start| {
                history[start..start + taps.len()]
                    .iter()
                    .zip(taps.iter().rev())
                    .map(|(sample, &tap)| {
                        Complex::new(f64::from(sample.re), f64::from(sample.im)) * widen(tap)
                    })
                    .sum()
            })
            .collect()
    }

    fn ragged<T, C>(fir: &mut StreamFir<T, C>, input: &[T]) -> Vec<T>
    where
        T: Accumulate<C>,
        C: Copy + PartialEq + Mul<f32, Output = C>,
    {
        let mut got = Vec::new();
        let mut block = Vec::new();
        let mut pos = 0;
        for len in [1usize, 7, 64, 3, 129, 5000, 17].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            fir.process(&input[pos..end], &mut block);
            got.extend_from_slice(&block);
            pos = end;
        }
        got
    }

    fn assert_close(actual: &[Complex<f32>], expected: &[Complex<f64>], label: &str) {
        assert_eq!(actual.len(), expected.len(), "{label}");
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            let actual = Complex::new(f64::from(actual.re), f64::from(actual.im));
            assert!(
                (actual - expected).norm() < 2e-6,
                "{label} sample={index}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn symmetric_and_asymmetric_taps_match_double_precision_in_ragged_blocks() {
        let input = signal(20_011);
        for (taps, factor) in [
            (127, 1),
            (127, 4),
            (64, 3),
            (11, 11),
            (75, 13),
            (3, 1),
            (2, 2),
        ] {
            let symmetric = if taps >= 3 {
                design_lowpass(taps, 0.11)
            } else {
                vec![0.5; taps]
            };
            let mut skewed = symmetric.clone();
            skewed[0] += 0.01;
            for (label, taps) in [("symmetric", symmetric), ("asymmetric", skewed)] {
                let mut fir = StreamFir::<Complex<f32>, f32>::new(&taps, factor);
                assert_eq!(fir.plan.folded(), label == "symmetric", "{label}");
                let expected = reference(&input, &taps, factor, |tap| {
                    Complex::new(f64::from(tap), 0.0)
                });
                let label = format!("{label} {} taps by {factor}", taps.len());
                assert_close(&ragged(&mut fir, &input), &expected, &label);
                fir.reset();
                assert_close(&ragged(&mut fir, &input), &expected, &label);
            }
        }
    }

    #[test]
    fn complex_taps_fold_only_when_symmetric() {
        let input = signal(9_001);
        let lowpass = design_lowpass(33, 0.2);
        let symmetric: Vec<_> = lowpass.iter().map(|&tap| Complex::new(tap, -tap)).collect();
        let rotated: Vec<_> = lowpass
            .iter()
            .enumerate()
            .map(|(index, &tap)| Complex::from_polar(tap, index as f32 * 0.3))
            .collect();
        for (taps, folded) in [(symmetric, true), (rotated, false)] {
            let mut fir = StreamFir::<Complex<f32>, Complex<f32>>::new(&taps, 1);
            assert_eq!(fir.plan.folded(), folded);
            let expected = reference(&input, &taps, 1, |tap| {
                Complex::new(f64::from(tap.re), f64::from(tap.im))
            });
            assert_close(
                &ragged(&mut fir, &input),
                &expected,
                &format!("folded {folded}"),
            );
        }
    }

    #[test]
    fn symmetric_fold_matches_the_plain_filter() {
        let input: Vec<f32> = signal(6_007).iter().map(|sample| sample.re).collect();
        for (taps, factor) in [(47, 2), (48, 1), (75, 13)] {
            let taps = design_lowpass(taps, 0.2 / factor as f64);
            let mut folded = StreamFir::<f32, f32>::new(&taps, factor);
            let mut plain = StreamFir::<f32, f32>::with_folding(&taps, factor, false);
            assert!(folded.plan.folded() && !plain.plan.folded());
            let (a, b) = (ragged(&mut folded, &input), ragged(&mut plain, &input));
            assert_eq!(a.len(), b.len());
            for (index, (a, b)) in a.iter().zip(&b).enumerate() {
                assert!((a - b).abs() < 1e-6, "sample={index}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let input = signal(12_345);
        let taps = design_lowpass(49, 0.06);
        let mut whole = StreamFir::<Complex<f32>, f32>::new(&taps, 7);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);
        let mut split = StreamFir::<Complex<f32>, f32>::new(&taps, 7);
        assert_eq!(ragged(&mut split, &input), expected);
    }
}
