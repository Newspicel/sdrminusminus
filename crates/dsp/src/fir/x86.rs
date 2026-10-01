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
    ) -> [f32; BLOCK_FLOATS] {
        if self.wide {
            unsafe { avx2::real_block::<FOLD>(floats, plan) }
        } else {
            unsafe { sse2::real_block::<FOLD>(floats, plan) }
        }
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
        if self.wide {
            unsafe { avx2::complex_block::<FOLD>(floats, plan) }
        } else {
            unsafe { sse2::complex_block::<FOLD>(floats, plan) }
        }
    }

    pub(crate) unsafe fn plane_dot<const N: usize>(
        self,
        re: [*const f32; N],
        im: [*const f32; N],
        taps: &[f32],
    ) -> [Complex<f32>; N] {
        if self.wide {
            unsafe { avx2::plane_dot(re, im, taps) }
        } else {
            unsafe { sse2::plane_dot(re, im, taps) }
        }
    }

    pub(crate) fn plane_interpolated(
        self,
        re: &[f32],
        im: &[f32],
        lower: &[f32],
        slope: &[f32],
        mu: f32,
    ) -> Complex<f32> {
        if self.wide {
            unsafe { avx2::plane_interpolated(re, im, lower, slope, mu) }
        } else {
            unsafe { sse2::plane_interpolated(re, im, lower, slope, mu) }
        }
    }
}
