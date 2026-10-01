use std::ops::{Add, Mul};

use num_complex::Complex;

#[cfg(target_arch = "aarch64")]
pub(crate) use super::neon::Isa;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) use super::scalar::Isa;
#[cfg(target_arch = "x86_64")]
pub(crate) use super::x86::Isa;

pub(super) const BLOCK_FLOATS: usize = 16;

pub(crate) trait Sample: Copy + Add<Output = Self> {
    const LANES: usize;
    const FLOATS: usize;
    fn zero() -> Self;
}

impl Sample for f32 {
    const LANES: usize = BLOCK_FLOATS;
    const FLOATS: usize = 1;

    fn zero() -> Self {
        0.0
    }
}

impl Sample for Complex<f32> {
    const LANES: usize = BLOCK_FLOATS / 2;
    const FLOATS: usize = 2;

    fn zero() -> Self {
        Complex::new(0.0, 0.0)
    }
}

pub(crate) trait Accumulate<C>: Sample {
    fn add_product(self, sample: Self, coefficient: C) -> Self;
    fn block<const FOLD: bool>(isa: Isa, window: &[Self], plan: &Plan<C>, out: &mut [Self]);
}

fn add_product(sum: f32, sample: f32, coefficient: f32) -> f32 {
    if cfg!(any(target_arch = "aarch64", target_feature = "fma")) {
        sample.mul_add(coefficient, sum)
    } else {
        sum + sample * coefficient
    }
}

impl Accumulate<f32> for f32 {
    fn add_product(self, sample: Self, coefficient: f32) -> Self {
        add_product(self, sample, coefficient)
    }

    fn block<const FOLD: bool>(isa: Isa, window: &[Self], plan: &Plan<f32>, out: &mut [Self]) {
        let lanes = isa.real_block::<FOLD>(&window[..plan.reach], plan);
        out.copy_from_slice(&lanes[..out.len()]);
    }
}

impl Accumulate<f32> for Complex<f32> {
    fn add_product(self, sample: Self, coefficient: f32) -> Self {
        Self::new(
            add_product(self.re, sample.re, coefficient),
            add_product(self.im, sample.im, coefficient),
        )
    }

    fn block<const FOLD: bool>(isa: Isa, window: &[Self], plan: &Plan<f32>, out: &mut [Self]) {
        let lanes = isa.complex_real_block::<FOLD>(floats(&window[..plan.reach]), plan);
        unpack(&lanes, out);
    }
}

impl Accumulate<Complex<f32>> for Complex<f32> {
    fn add_product(self, sample: Self, coefficient: Self) -> Self {
        Self::new(
            add_product(
                add_product(self.re, sample.re, coefficient.re),
                -sample.im,
                coefficient.im,
            ),
            add_product(
                add_product(self.im, sample.re, coefficient.im),
                sample.im,
                coefficient.re,
            ),
        )
    }

    fn block<const FOLD: bool>(
        isa: Isa,
        window: &[Self],
        plan: &Plan<Complex<f32>>,
        out: &mut [Self],
    ) {
        let lanes = isa.complex_block::<FOLD>(floats(&window[..plan.reach]), plan);
        unpack(&lanes, out);
    }
}

pub(super) fn floats(samples: &[Complex<f32>]) -> &[f32] {
    unsafe { std::slice::from_raw_parts(samples.as_ptr().cast(), 2 * samples.len()) }
}

fn unpack(lanes: &[f32; BLOCK_FLOATS], out: &mut [Complex<f32>]) {
    for (sample, pair) in out.iter_mut().zip(lanes.as_chunks::<2>().0) {
        *sample = Complex::new(pair[0], pair[1]);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tap<C> {
    pub(super) front: usize,
    pub(super) back: usize,
    pub(super) value: C,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(super) offset: usize,
    pub(super) taps: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Plan<C> {
    pub(super) taps: Vec<Tap<C>>,
    #[cfg(target_arch = "aarch64")]
    pub(super) rows: Vec<Row>,
    #[cfg(target_arch = "aarch64")]
    pub(super) row_taps: Vec<C>,
    pub(super) reach: usize,
    pub(super) floats: usize,
    pub(super) folded: bool,
}

impl<C> Plan<C>
where
    C: Copy + PartialEq + Mul<f32, Output = C>,
{
    pub(crate) fn new<T: Sample>(taps: &[C], phases: usize, stride: usize, fold: bool) -> Self {
        let offset = |index: usize| ((index % phases) * stride + index / phases) * T::FLOATS;
        let reversed: Vec<C> = taps.iter().rev().copied().collect();
        let folded = fold && reversed.iter().eq(taps.iter());
        let last = taps.len() - 1;
        let taps: Vec<_> = if folded {
            (0..taps.len().div_ceil(2))
                .map(|index| Tap {
                    front: offset(index),
                    back: offset(last - index),
                    value: if index == last - index {
                        reversed[index] * 0.5
                    } else {
                        reversed[index]
                    },
                })
                .collect()
        } else {
            reversed
                .iter()
                .enumerate()
                .map(|(index, &value)| Tap {
                    front: offset(index),
                    back: offset(index),
                    value,
                })
                .collect()
        };
        let farthest = taps.iter().map(|tap| tap.front.max(tap.back)).max();
        let reach = farthest.unwrap_or(0) / T::FLOATS + T::LANES;
        #[cfg(target_arch = "aarch64")]
        let (rows, row_taps) = rows::<T, C>(&reversed, phases, stride);
        Self {
            reach,
            floats: reach * T::FLOATS,
            taps,
            #[cfg(target_arch = "aarch64")]
            rows,
            #[cfg(target_arch = "aarch64")]
            row_taps,
            folded,
        }
    }

    pub(crate) fn folded(&self) -> bool {
        self.folded
    }
}

#[cfg(target_arch = "aarch64")]
fn rows<T: Sample, C: Copy>(reversed: &[C], phases: usize, stride: usize) -> (Vec<Row>, Vec<C>) {
    let mut rows = Vec::with_capacity(phases);
    let mut row_taps = Vec::with_capacity(reversed.len());
    for phase in 0..phases.min(reversed.len()) {
        let start = row_taps.len();
        row_taps.extend(reversed[phase..].iter().step_by(phases));
        rows.push(Row {
            offset: phase * stride * T::FLOATS,
            taps: start..row_taps.len(),
        });
    }
    (rows, row_taps)
}

pub(crate) fn plane_dot<const N: usize>(
    isa: Isa,
    re: &[f32],
    im: &[f32],
    starts: [usize; N],
    taps: &[f32],
) -> [Complex<f32>; N] {
    let len = taps.len();
    let re = starts.map(|start| re[start..start + len].as_ptr());
    let im = starts.map(|start| im[start..start + len].as_ptr());
    unsafe { isa.plane_dot(re, im, taps) }
}

pub(crate) fn plane_interpolated(
    isa: Isa,
    re: &[f32],
    im: &[f32],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let len = lower.len().min(slope.len()).min(re.len()).min(im.len());
    isa.plane_interpolated(&re[..len], &im[..len], &lower[..len], &slope[..len], mu)
}

pub(super) unsafe fn plane_tail(
    sum: Complex<f32>,
    re: *const f32,
    im: *const f32,
    taps: &[f32],
    from: usize,
) -> Complex<f32> {
    taps.iter()
        .enumerate()
        .skip(from)
        .fold(sum, |sum, (index, &tap)| {
            let sample = unsafe { Complex::new(*re.add(index), *im.add(index)) };
            sum.add_product(sample, tap)
        })
}

pub(super) fn interpolated_plane_tail(
    sum: Complex<f32>,
    re: &[f32],
    im: &[f32],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    re.iter().zip(im).zip(lower.iter().zip(slope)).fold(
        sum,
        |sum, ((&re, &im), (&lower, &slope))| {
            sum.add_product(Complex::new(re, im), add_product(lower, slope, mu))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fir::scalar;

    fn complex_ramp(len: usize, a: usize, b: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|index| {
                Complex::new(
                    (index % a) as f32 / a as f32 - 0.5,
                    (index % b) as f32 / b as f32 - 0.5,
                )
            })
            .collect()
    }

    fn widen(value: Complex<f32>) -> Complex<f64> {
        Complex::new(f64::from(value.re), f64::from(value.im))
    }

    trait Widen: Copy {
        fn widened(self) -> Complex<f64>;
    }

    impl Widen for f32 {
        fn widened(self) -> Complex<f64> {
            Complex::new(f64::from(self), 0.0)
        }
    }

    impl Widen for Complex<f32> {
        fn widened(self) -> Complex<f64> {
            widen(self)
        }
    }

    fn lengths() -> impl Iterator<Item = usize> {
        (1..=17).chain([31, 32, 33, 63, 64, 65, 127, 128, 129, 257])
    }

    fn one_output<T, C>(isa: Isa, window: &[T], taps: &[C], fold: bool) -> T
    where
        T: Accumulate<C>,
        C: Copy + PartialEq + Mul<f32, Output = C>,
    {
        let plan = Plan::new::<T>(taps, 1, window.len() + T::LANES, fold);
        let mut padded = window.to_vec();
        padded.resize(plan.reach, T::zero());
        let mut out = [T::zero()];
        if plan.folded() {
            T::block::<true>(isa, &padded, &plan, &mut out);
        } else {
            T::block::<false>(isa, &padded, &plan, &mut out);
        }
        out[0]
    }

    #[test]
    fn batched_products_match_double_precision_for_every_tail_length() {
        for isa in Isa::available() {
            for len in 1..=257 {
                let samples = complex_ramp(len, 7, 11);
                let taps = complex_ramp(len, 13, 17);
                let reference: Complex<f64> = samples
                    .iter()
                    .zip(taps.iter().rev())
                    .map(|(&sample, &tap)| widen(sample) * widen(tap))
                    .sum();
                let actual = widen(one_output(isa, &samples, &taps, false));
                assert!(
                    (actual - reference).norm() < len as f64 * 1e-7,
                    "{isa:?} length={len}"
                );
                let real_samples: Vec<_> = samples.iter().map(|sample| sample.re).collect();
                let real_taps: Vec<_> = taps.iter().map(|tap| tap.re).collect();
                let reference: Complex<f64> = samples
                    .iter()
                    .zip(real_taps.iter().rev())
                    .map(|(&sample, &tap)| widen(sample) * f64::from(tap))
                    .sum();
                let actual = widen(one_output(isa, &samples, &real_taps, false));
                assert!(
                    (actual - reference).norm() < len as f64 * 1e-7,
                    "{isa:?} real taps, length={len}"
                );
                let reference: f64 = real_samples
                    .iter()
                    .zip(real_taps.iter().rev())
                    .map(|(&sample, &tap)| f64::from(sample) * f64::from(tap))
                    .sum();
                let actual = f64::from(one_output(isa, &real_samples, &real_taps, false));
                assert!(
                    (actual - reference).abs() < len as f64 * 1e-7,
                    "{isa:?} real, length={len}"
                );
            }
        }
    }

    fn check_block<T, C>(isa: Isa, window: &[T], taps: &[C], phases: usize, label: &str)
    where
        T: Accumulate<C> + Widen + std::fmt::Debug,
        C: Copy + PartialEq + Mul<f32, Output = C>,
    {
        let stride = window.len().div_ceil(phases);
        for fold in [false, true] {
            let plan = Plan::new::<T>(taps, phases, stride, fold);
            let mut padded = window.to_vec();
            padded.resize(plan.reach.max(window.len()), T::zero());
            let mut simd = vec![T::zero(); T::LANES];
            let mut reference = simd.clone();
            if plan.folded() {
                T::block::<true>(isa, &padded, &plan, &mut simd);
                scalar::block::<T, C, true>(&padded[..plan.reach], &plan, &mut reference);
            } else {
                T::block::<false>(isa, &padded, &plan, &mut simd);
                scalar::block::<T, C, false>(&padded[..plan.reach], &plan, &mut reference);
            }
            for (lane, (&a, &b)) in simd.iter().zip(&reference).enumerate() {
                let error = (a.widened() - b.widened()).norm();
                assert!(
                    error <= 1e-6 * (taps.len() as f64 + 1.0),
                    "{isa:?} {label} taps={} phases={phases} fold={fold} lane={lane}: {a:?} vs {b:?}",
                    taps.len()
                );
            }
        }
    }

    #[test]
    fn every_block_kernel_matches_the_scalar_reference_at_every_length() {
        for isa in Isa::available() {
            for len in lengths() {
                for phases in [1, 2, 3, 7] {
                    let window = complex_ramp(4 * (len + 64), 7, 11);
                    let real_window: Vec<f32> = window.iter().map(|sample| sample.im).collect();
                    let complex_taps = complex_ramp(len, 13, 17);
                    let real_taps: Vec<f32> = complex_taps.iter().map(|tap| tap.re).collect();
                    let symmetric: Vec<f32> = (0..len)
                        .map(|index| real_taps[index.min(len - 1 - index)])
                        .collect();
                    let symmetric_complex: Vec<_> = (0..len)
                        .map(|index| complex_taps[index.min(len - 1 - index)])
                        .collect();
                    check_block(isa, &real_window, &real_taps, phases, "real");
                    check_block(isa, &real_window, &symmetric, phases, "real symmetric");
                    check_block(isa, &window, &real_taps, phases, "complex real");
                    check_block(isa, &window, &symmetric, phases, "complex symmetric");
                    check_block(isa, &window, &complex_taps, phases, "complex");
                    check_block(
                        isa,
                        &window,
                        &symmetric_complex,
                        phases,
                        "complex taps symmetric",
                    );
                }
            }
        }
    }

    #[test]
    fn folding_needs_mirrored_taps() {
        let symmetric = [0.25f32, 0.5, 1.0, 0.5, 0.25];
        assert!(Plan::new::<f32>(&symmetric, 1, 64, true).folded());
        assert!(!Plan::new::<f32>(&symmetric, 1, 64, false).folded());
        assert!(!Plan::new::<f32>(&[0.25, 0.5, 1.0, 0.5, 0.2], 1, 64, true).folded());
        assert_eq!(Plan::new::<f32>(&symmetric, 1, 64, true).taps.len(), 3);
        assert_eq!(Plan::new::<f32>(&[0.5, 0.5], 1, 64, true).taps.len(), 1);
    }

    fn planes(len: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let samples = complex_ramp(len, 7, 11);
        let taps = complex_ramp(len, 13, 17);
        (
            samples.iter().map(|sample| sample.re).collect(),
            samples.iter().map(|sample| sample.im).collect(),
            taps.iter().map(|tap| tap.re).collect(),
            taps.iter().map(|tap| tap.im * 0.1).collect(),
        )
    }

    fn planar_reference(re: &[f32], im: &[f32], taps: impl Iterator<Item = f64>) -> Complex<f64> {
        re.iter()
            .zip(im)
            .zip(taps)
            .map(|((&re, &im), tap)| Complex::new(f64::from(re), f64::from(im)) * tap)
            .sum()
    }

    #[test]
    fn planar_dot_matches_double_precision_and_is_the_same_for_any_batch() {
        for isa in Isa::available() {
            for len in (0..=17).chain(lengths()) {
                let (re, im, taps, _) = planes(len + 9);
                let taps = &taps[..len];
                let starts = [0, 3, 9, 1];
                let batch = plane_dot(isa, &re, &im, starts, taps);
                for (&start, &actual) in starts.iter().zip(&batch) {
                    let window = start..start + len;
                    let reference = planar_reference(
                        &re[window.clone()],
                        &im[window],
                        taps.iter().map(|&tap| f64::from(tap)),
                    );
                    assert!(
                        (widen(actual) - reference).norm() <= 1e-6 * (len as f64 + 1.0),
                        "{isa:?} length={len} start={start}"
                    );
                    assert_eq!(
                        plane_dot(isa, &re, &im, [start], taps)[0],
                        actual,
                        "{isa:?} length={len} start={start}"
                    );
                }
            }
        }
    }

    #[test]
    fn planar_interpolation_matches_double_precision_at_every_length() {
        for isa in Isa::available() {
            for len in (0..=17).chain(lengths()) {
                let (re, im, lower, slope) = planes(len);
                let mu = 0.625;
                let reference = planar_reference(
                    &re,
                    &im,
                    lower.iter().zip(&slope).map(|(&lower, &slope)| {
                        f64::from(lower) + f64::from(slope) * f64::from(mu)
                    }),
                );
                let actual = widen(plane_interpolated(isa, &re, &im, &lower, &slope, mu));
                assert!(
                    (actual - reference).norm() <= 1e-6 * (len as f64 + 1.0),
                    "{isa:?} length={len}: {actual} vs {reference}"
                );
            }
        }
    }

    #[test]
    fn complex_taps_accumulate_the_full_product() {
        let sum = Complex::new(0.25f32, -0.5);
        let sample = Complex::new(0.75f32, -1.25);
        let tap = Complex::new(-0.5f32, 2.0);
        assert_eq!(sum.add_product(sample, tap), sum + sample * tap);
    }
}
