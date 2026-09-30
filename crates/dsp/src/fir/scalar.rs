use num_complex::Complex;

use super::kernel::interpolated_tail;
#[cfg(test)]
use super::kernel::{Accumulate, Plan};

#[cfg(test)]
pub(super) fn block<T, C, const FOLD: bool>(window: &[T], plan: &Plan<C>, out: &mut [T])
where
    T: Accumulate<C>,
    C: Copy,
{
    for (lane, sample) in out.iter_mut().enumerate() {
        *sample = plan.taps.iter().fold(T::zero(), |sum, tap| {
            let front = window[tap.front + lane];
            let pair = if FOLD {
                front + window[tap.back + lane]
            } else {
                front
            };
            sum.add_product(pair, tap.value)
        });
    }
}

pub(super) fn interpolated_dot(
    samples: &[Complex<f32>],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    interpolated_tail(Complex::new(0.0, 0.0), samples, lower, slope, mu)
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod fallback {
    use num_complex::Complex;

    use super::interpolated_dot;
    use crate::fir::kernel::{BLOCK_FLOATS, Plan};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Isa;

    fn pair<const FOLD: bool>(floats: &[f32], front: usize, back: usize) -> f32 {
        if FOLD {
            floats[front] + floats[back]
        } else {
            floats[front]
        }
    }

    impl Isa {
        pub(crate) fn detect() -> Self {
            Self
        }

        #[cfg(test)]
        pub(crate) fn available() -> Vec<Self> {
            vec![Self]
        }

        pub(crate) fn real_block<const FOLD: bool>(
            self,
            floats: &[f32],
            plan: &Plan<f32>,
            scale: usize,
        ) -> [f32; BLOCK_FLOATS] {
            std::array::from_fn(|lane| {
                plan.taps.iter().fold(0.0, |sum, tap| {
                    let sample =
                        pair::<FOLD>(floats, tap.front * scale + lane, tap.back * scale + lane);
                    sum + sample * tap.value
                })
            })
        }

        pub(crate) fn complex_block<const FOLD: bool>(
            self,
            floats: &[f32],
            plan: &Plan<Complex<f32>>,
        ) -> [f32; BLOCK_FLOATS] {
            let mut lanes = [0.0; BLOCK_FLOATS];
            for (lane, out) in lanes.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let sum = plan.taps.iter().fold(Complex::new(0.0, 0.0), |sum, tap| {
                    let (front, back) = (2 * (tap.front + lane), 2 * (tap.back + lane));
                    let sample = Complex::new(
                        pair::<FOLD>(floats, front, back),
                        pair::<FOLD>(floats, front + 1, back + 1),
                    );
                    sum + sample * tap.value
                });
                *out = [sum.re, sum.im];
            }
            lanes
        }

        pub(crate) fn interpolated_dot(
            self,
            samples: &[Complex<f32>],
            lower: &[f32],
            slope: &[f32],
            mu: f32,
        ) -> Complex<f32> {
            interpolated_dot(samples, lower, slope, mu)
        }
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) use fallback::Isa;
