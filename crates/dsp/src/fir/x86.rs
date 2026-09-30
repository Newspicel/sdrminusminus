use num_complex::Complex;

use super::kernel::{BLOCK_FLOATS, Plan};

mod avx2;
mod sse2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa {
    wide: bool,
}

fn wide_available() -> bool {
    is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")
}

impl Isa {
    pub(crate) fn detect() -> Self {
        Self {
            wide: wide_available(),
        }
    }

    #[cfg(test)]
    pub(crate) fn available() -> Vec<Self> {
        let mut available = vec![Self { wide: false }];
        if wide_available() {
            available.push(Self { wide: true });
        }
        available
    }

    pub(crate) fn real_block<const FOLD: bool>(
        self,
        floats: &[f32],
        plan: &Plan<f32>,
        scale: usize,
    ) -> [f32; BLOCK_FLOATS] {
        if self.wide {
            unsafe { avx2::real_block::<FOLD>(floats, plan, scale) }
        } else {
            unsafe { sse2::real_block::<FOLD>(floats, plan, scale) }
        }
    }

    pub(crate) fn complex_block<const FOLD: bool>(
        self,
        floats: &[f32],
        plan: &Plan<Complex<f32>>,
    ) -> [f32; BLOCK_FLOATS] {
        if self.wide {
            unsafe { avx2::complex_block::<FOLD>(floats, plan) }
        } else {
            unsafe { sse2::complex_block::<FOLD>(floats, plan) }
        }
    }

    pub(crate) fn interpolated_dot(
        self,
        samples: &[Complex<f32>],
        lower: &[f32],
        slope: &[f32],
        mu: f32,
    ) -> Complex<f32> {
        if self.wide {
            unsafe { avx2::interpolated_dot(samples, lower, slope, mu) }
        } else {
            unsafe { sse2::interpolated_dot(samples, lower, slope, mu) }
        }
    }
}
