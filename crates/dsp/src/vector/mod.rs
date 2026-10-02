mod body;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GemmShape {
    pub rows: usize,
    pub depth: usize,
    pub cols: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vector {
    wide: bool,
}

impl Vector {
    #[must_use]
    pub fn detect() -> Self {
        Self { wide: wide() }
    }

    #[must_use]
    pub fn portable() -> Self {
        Self { wide: false }
    }

    #[must_use]
    pub fn dot(self, a: &[f32], b: &[f32]) -> f32 {
        let len = a.len().min(b.len());
        let (a, b) = (&a[..len], &b[..len]);
        #[cfg(target_arch = "x86_64")]
        if self.wide {
            return unsafe { x86::dot(a, b) };
        }
        body::dot::<NATIVE_FMA>(a, b)
    }

    pub fn axpy(self, y: &mut [f32], alpha: f32, x: &[f32]) {
        #[cfg(target_arch = "x86_64")]
        if self.wide {
            return unsafe { x86::axpy(y, alpha, x) };
        }
        body::axpy::<NATIVE_FMA>(y, alpha, x);
    }

    pub fn tanh(self, values: &mut [f32]) {
        #[cfg(target_arch = "x86_64")]
        if self.wide {
            return unsafe { x86::tanh(values) };
        }
        body::tanh::<NATIVE_FMA>(values);
    }

    pub fn sigmoid(self, values: &mut [f32]) {
        #[cfg(target_arch = "x86_64")]
        if self.wide {
            return unsafe { x86::sigmoid(values) };
        }
        body::sigmoid::<NATIVE_FMA>(values);
    }

    pub fn gemm(self, shape: GemmShape, lhs: &[f32], rhs: &[f32], out: &mut [f32]) {
        let fits = lhs.len() >= shape.rows * shape.depth
            && rhs.len() >= shape.depth * shape.cols
            && out.len() >= shape.rows * shape.cols;
        if !fits {
            return;
        }
        #[cfg(target_arch = "x86_64")]
        if self.wide {
            return unsafe { x86::gemm(shape, lhs, rhs, out) };
        }
        body::gemm::<NATIVE_FMA>(shape, lhs, rhs, out);
    }
}

const NATIVE_FMA: bool = cfg!(any(target_arch = "aarch64", target_feature = "fma"));

#[cfg(target_arch = "x86_64")]
fn wide() -> bool {
    is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")
}

#[cfg(not(target_arch = "x86_64"))]
fn wide() -> bool {
    false
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::{GemmShape, body};

    #[target_feature(enable = "avx2,fma")]
    pub(super) fn dot(a: &[f32], b: &[f32]) -> f32 {
        body::dot::<true>(a, b)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) fn axpy(y: &mut [f32], alpha: f32, x: &[f32]) {
        body::axpy::<true>(y, alpha, x);
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) fn tanh(values: &mut [f32]) {
        body::tanh::<true>(values);
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) fn sigmoid(values: &mut [f32]) {
        body::sigmoid::<true>(values);
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) fn gemm(shape: GemmShape, lhs: &[f32], rhs: &[f32], out: &mut [f32]) {
        body::gemm::<true>(shape, lhs, rhs, out);
    }
}
