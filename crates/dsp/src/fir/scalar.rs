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
            let front = window[tap.front / T::FLOATS + lane];
            let pair = if FOLD {
                front + window[tap.back / T::FLOATS + lane]
            } else {
                front
            };
            sum.add_product(pair, tap.value)
        });
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod fallback {
    use num_complex::Complex;

    use crate::fir::kernel::{BLOCK_FLOATS, Plan, interpolated_plane_tail, plane_tail};

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
        ) -> [f32; BLOCK_FLOATS] {
            std::array::from_fn(|lane| {
                plan.taps.iter().fold(0.0, |sum, tap| {
                    let sample = pair::<FOLD>(floats, tap.front + lane, tap.back + lane);
                    sum + sample * tap.value
                })
            })
        }

        pub(crate) fn complex_real_block<const FOLD: bool>(
            self,
            floats: &[f32],
            plan: &Plan<f32>,
        ) -> [f32; BLOCK_FLOATS] {
            self.real_block::<FOLD>(floats, plan)
        }

        pub(crate) fn complex_block<const FOLD: bool>(
            self,
            floats: &[f32],
            plan: &Plan<Complex<f32>>,
        ) -> [f32; BLOCK_FLOATS] {
            let mut lanes = [0.0; BLOCK_FLOATS];
            for (lane, out) in lanes.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let sum = plan.taps.iter().fold(Complex::new(0.0, 0.0), |sum, tap| {
                    let (front, back) = (tap.front + 2 * lane, tap.back + 2 * lane);
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

        pub(crate) unsafe fn plane_dot<const N: usize>(
            self,
            re: [*const f32; N],
            im: [*const f32; N],
            taps: &[f32],
        ) -> [Complex<f32>; N] {
            std::array::from_fn(|output| unsafe {
                plane_tail(Complex::new(0.0, 0.0), re[output], im[output], taps, 0)
            })
        }

        pub(crate) fn plane_interpolated(
            self,
            re: &[f32],
            im: &[f32],
            lower: &[f32],
            slope: &[f32],
            mu: f32,
        ) -> Complex<f32> {
            interpolated_plane_tail(Complex::new(0.0, 0.0), re, im, lower, slope, mu)
        }
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) use fallback::Isa;
