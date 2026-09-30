use std::arch::aarch64::{
    float32x4_t, float32x4x4_t, vadd_f32, vaddq_f32, vdupq_n_f32, vfmaq_f32, vfmaq_n_f32,
    vget_high_f32, vget_lane_f32, vget_low_f32, vld1q_f32, vld1q_f32_x2, vld1q_f32_x4, vrev64q_f32,
    vst1q_f32_x4, vzip1q_f32, vzip2q_f32,
};

use num_complex::Complex;

use super::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_tail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa;

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
        unsafe { real_block::<FOLD>(floats, plan, scale) }
    }

    pub(crate) fn complex_block<const FOLD: bool>(
        self,
        floats: &[f32],
        plan: &Plan<Complex<f32>>,
    ) -> [f32; BLOCK_FLOATS] {
        unsafe { complex_block::<FOLD>(floats, plan) }
    }

    pub(crate) fn interpolated_dot(
        self,
        samples: &[Complex<f32>],
        lower: &[f32],
        slope: &[f32],
        mu: f32,
    ) -> Complex<f32> {
        unsafe { interpolated_dot(samples, lower, slope, mu) }
    }
}

type Block = [float32x4_t; 4];

const SETS: usize = 4;

#[target_feature(enable = "neon")]
fn zero() -> Block {
    [vdupq_n_f32(0.0); 4]
}

#[target_feature(enable = "neon")]
fn load(values: &[f32; 4]) -> float32x4_t {
    unsafe { vld1q_f32(values.as_ptr()) }
}

#[target_feature(enable = "neon")]
fn load_block(floats: &[f32], offset: usize) -> Block {
    let lanes = &floats[offset..offset + BLOCK_FLOATS];
    let loaded = unsafe { vld1q_f32_x4(lanes.as_ptr()) };
    [loaded.0, loaded.1, loaded.2, loaded.3]
}

#[target_feature(enable = "neon")]
fn store(block: Block) -> [f32; BLOCK_FLOATS] {
    let mut lanes = [0.0; BLOCK_FLOATS];
    unsafe {
        vst1q_f32_x4(
            lanes.as_mut_ptr(),
            float32x4x4_t(block[0], block[1], block[2], block[3]),
        );
    }
    lanes
}

#[target_feature(enable = "neon")]
fn add(a: Block, b: Block) -> Block {
    [
        vaddq_f32(a[0], b[0]),
        vaddq_f32(a[1], b[1]),
        vaddq_f32(a[2], b[2]),
        vaddq_f32(a[3], b[3]),
    ]
}

#[target_feature(enable = "neon")]
fn add_product(sum: Block, samples: Block, tap: f32) -> Block {
    [
        vfmaq_n_f32(sum[0], samples[0], tap),
        vfmaq_n_f32(sum[1], samples[1], tap),
        vfmaq_n_f32(sum[2], samples[2], tap),
        vfmaq_n_f32(sum[3], samples[3], tap),
    ]
}

#[target_feature(enable = "neon")]
fn samples<const FOLD: bool, C>(floats: &[f32], tap: &Tap<C>, scale: usize) -> Block {
    let front = load_block(floats, tap.front * scale);
    if FOLD {
        add(front, load_block(floats, tap.back * scale))
    } else {
        front
    }
}

#[target_feature(enable = "neon")]
fn real_block<const FOLD: bool>(
    floats: &[f32],
    plan: &Plan<f32>,
    scale: usize,
) -> [f32; BLOCK_FLOATS] {
    let mut sets = [zero(); SETS];
    let (groups, rest) = plan.taps.as_chunks::<SETS>();
    for group in groups {
        for (set, tap) in sets.iter_mut().zip(group) {
            *set = add_product(*set, samples::<FOLD, f32>(floats, tap, scale), tap.value);
        }
    }
    for tap in rest {
        sets[0] = add_product(sets[0], samples::<FOLD, f32>(floats, tap, scale), tap.value);
    }
    store(add(add(sets[0], sets[1]), add(sets[2], sets[3])))
}

#[target_feature(enable = "neon")]
fn complex_step<const FOLD: bool>(
    sums: [Block; 2],
    floats: &[f32],
    tap: &Tap<Complex<f32>>,
) -> [Block; 2] {
    let samples = samples::<FOLD, Complex<f32>>(floats, tap, 2);
    [
        add_product(sums[0], samples, tap.value.re),
        add_product(sums[1], samples, tap.value.im),
    ]
}

#[target_feature(enable = "neon")]
fn rotated(by_re: float32x4_t, by_im: float32x4_t) -> float32x4_t {
    let sign = load(&[-1.0, 1.0, -1.0, 1.0]);
    vfmaq_f32(by_re, vrev64q_f32(by_im), sign)
}

#[target_feature(enable = "neon")]
fn complex_block<const FOLD: bool>(
    floats: &[f32],
    plan: &Plan<Complex<f32>>,
) -> [f32; BLOCK_FLOATS] {
    let mut sets = [[zero(); 2]; SETS / 2];
    let (groups, rest) = plan.taps.as_chunks::<{ SETS / 2 }>();
    for group in groups {
        for (set, tap) in sets.iter_mut().zip(group) {
            *set = complex_step::<FOLD>(*set, floats, tap);
        }
    }
    for tap in rest {
        sets[0] = complex_step::<FOLD>(sets[0], floats, tap);
    }
    let by_re = add(sets[0][0], sets[1][0]);
    let by_im = add(sets[0][1], sets[1][1]);
    store([
        rotated(by_re[0], by_im[0]),
        rotated(by_re[1], by_im[1]),
        rotated(by_re[2], by_im[2]),
        rotated(by_re[3], by_im[3]),
    ])
}

#[target_feature(enable = "neon")]
fn interpolated_step(
    sums: [float32x4_t; 2],
    samples: &[Complex<f32>; 4],
    lower: &[f32; 4],
    slope: &[f32; 4],
    mu: f32,
) -> [float32x4_t; 2] {
    let samples = unsafe { vld1q_f32_x2(samples.as_ptr().cast()) };
    let taps = vfmaq_n_f32(load(lower), load(slope), mu);
    [
        vfmaq_f32(sums[0], samples.0, vzip1q_f32(taps, taps)),
        vfmaq_f32(sums[1], samples.1, vzip2q_f32(taps, taps)),
    ]
}

#[target_feature(enable = "neon")]
fn interpolated_dot(
    samples: &[Complex<f32>],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let (sample_blocks, samples) = samples.as_chunks::<8>();
    let (lower_blocks, lower) = lower.as_chunks::<8>();
    let (slope_blocks, slope) = slope.as_chunks::<8>();
    let mut sums = [[vdupq_n_f32(0.0); 2]; 2];
    for ((samples, lower), slope) in sample_blocks.iter().zip(lower_blocks).zip(slope_blocks) {
        let quads = samples
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lower.as_chunks::<4>().0);
        for ((sum, (samples, lower)), slope) in
            sums.iter_mut().zip(quads).zip(slope.as_chunks::<4>().0)
        {
            *sum = interpolated_step(*sum, samples, lower, slope, mu);
        }
    }
    let (sample_quads, samples) = samples.as_chunks::<4>();
    let (lower_quads, lower) = lower.as_chunks::<4>();
    let (slope_quads, slope) = slope.as_chunks::<4>();
    for ((samples, lower), slope) in sample_quads.iter().zip(lower_quads).zip(slope_quads) {
        sums[0] = interpolated_step(sums[0], samples, lower, slope, mu);
    }
    let sum = vaddq_f32(
        vaddq_f32(sums[0][0], sums[0][1]),
        vaddq_f32(sums[1][0], sums[1][1]),
    );
    let pair = vadd_f32(vget_low_f32(sum), vget_high_f32(sum));
    let sum = Complex::new(vget_lane_f32::<0>(pair), vget_lane_f32::<1>(pair));
    interpolated_tail(sum, samples, lower, slope, mu)
}
