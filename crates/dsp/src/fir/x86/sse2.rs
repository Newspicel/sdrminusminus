use std::arch::x86_64::{
    __m128, _mm_add_ps, _mm_cvtss_f32, _mm_loadu_ps, _mm_movehl_ps, _mm_mul_ps, _mm_set1_ps,
    _mm_setr_ps, _mm_setzero_ps, _mm_shuffle_ps, _mm_storeu_ps, _mm_unpackhi_ps, _mm_unpacklo_ps,
};

use num_complex::Complex;

use crate::fir::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_tail};

type Block = [__m128; 4];

const SETS: usize = 2;

#[target_feature(enable = "sse2")]
fn zero() -> Block {
    [_mm_setzero_ps(); 4]
}

#[target_feature(enable = "sse2")]
fn load(values: &[f32; 4]) -> __m128 {
    unsafe { _mm_loadu_ps(values.as_ptr()) }
}

#[target_feature(enable = "sse2")]
fn load_block(floats: &[f32], offset: usize) -> Block {
    let (quads, _) = floats[offset..offset + BLOCK_FLOATS].as_chunks::<4>();
    [
        load(&quads[0]),
        load(&quads[1]),
        load(&quads[2]),
        load(&quads[3]),
    ]
}

#[target_feature(enable = "sse2")]
fn store(block: Block) -> [f32; BLOCK_FLOATS] {
    let mut lanes = [0.0; BLOCK_FLOATS];
    for (quad, value) in lanes.as_chunks_mut::<4>().0.iter_mut().zip(block) {
        unsafe { _mm_storeu_ps(quad.as_mut_ptr(), value) };
    }
    lanes
}

#[target_feature(enable = "sse2")]
fn add(a: Block, b: Block) -> Block {
    [
        _mm_add_ps(a[0], b[0]),
        _mm_add_ps(a[1], b[1]),
        _mm_add_ps(a[2], b[2]),
        _mm_add_ps(a[3], b[3]),
    ]
}

#[target_feature(enable = "sse2")]
fn add_product(sum: Block, samples: Block, tap: f32) -> Block {
    let tap = _mm_set1_ps(tap);
    [
        _mm_add_ps(sum[0], _mm_mul_ps(samples[0], tap)),
        _mm_add_ps(sum[1], _mm_mul_ps(samples[1], tap)),
        _mm_add_ps(sum[2], _mm_mul_ps(samples[2], tap)),
        _mm_add_ps(sum[3], _mm_mul_ps(samples[3], tap)),
    ]
}

#[target_feature(enable = "sse2")]
fn samples<const FOLD: bool, C>(floats: &[f32], tap: &Tap<C>, scale: usize) -> Block {
    let front = load_block(floats, tap.front * scale);
    if FOLD {
        add(front, load_block(floats, tap.back * scale))
    } else {
        front
    }
}

#[target_feature(enable = "sse2")]
pub(super) fn real_block<const FOLD: bool>(
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
    store(add(sets[0], sets[1]))
}

#[target_feature(enable = "sse2")]
fn rotated(by_re: __m128, by_im: __m128) -> __m128 {
    let sign = _mm_setr_ps(-1.0, 1.0, -1.0, 1.0);
    _mm_add_ps(
        by_re,
        _mm_mul_ps(_mm_shuffle_ps::<0xB1>(by_im, by_im), sign),
    )
}

#[target_feature(enable = "sse2")]
pub(super) fn complex_block<const FOLD: bool>(
    floats: &[f32],
    plan: &Plan<Complex<f32>>,
) -> [f32; BLOCK_FLOATS] {
    let (mut by_re, mut by_im) = (zero(), zero());
    for tap in &plan.taps {
        let samples = samples::<FOLD, Complex<f32>>(floats, tap, 2);
        by_re = add_product(by_re, samples, tap.value.re);
        by_im = add_product(by_im, samples, tap.value.im);
    }
    store([
        rotated(by_re[0], by_im[0]),
        rotated(by_re[1], by_im[1]),
        rotated(by_re[2], by_im[2]),
        rotated(by_re[3], by_im[3]),
    ])
}

#[target_feature(enable = "sse2")]
fn interpolated_step(
    sums: [__m128; 2],
    samples: &[Complex<f32>; 4],
    lower: &[f32; 4],
    slope: &[f32; 4],
    mu: __m128,
) -> [__m128; 2] {
    let taps = _mm_add_ps(load(lower), _mm_mul_ps(load(slope), mu));
    let (pairs, _) = samples.as_chunks::<2>();
    let first = unsafe { _mm_loadu_ps(pairs[0].as_ptr().cast()) };
    let second = unsafe { _mm_loadu_ps(pairs[1].as_ptr().cast()) };
    [
        _mm_add_ps(sums[0], _mm_mul_ps(first, _mm_unpacklo_ps(taps, taps))),
        _mm_add_ps(sums[1], _mm_mul_ps(second, _mm_unpackhi_ps(taps, taps))),
    ]
}

#[target_feature(enable = "sse2")]
pub(super) fn interpolated_dot(
    samples: &[Complex<f32>],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let scale = _mm_set1_ps(mu);
    let (sample_blocks, samples) = samples.as_chunks::<8>();
    let (lower_blocks, lower) = lower.as_chunks::<8>();
    let (slope_blocks, slope) = slope.as_chunks::<8>();
    let mut sums = [[_mm_setzero_ps(); 2]; 2];
    for ((samples, lower), slope) in sample_blocks.iter().zip(lower_blocks).zip(slope_blocks) {
        let quads = samples
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lower.as_chunks::<4>().0);
        for ((sum, (samples, lower)), slope) in
            sums.iter_mut().zip(quads).zip(slope.as_chunks::<4>().0)
        {
            *sum = interpolated_step(*sum, samples, lower, slope, scale);
        }
    }
    let (sample_quads, samples) = samples.as_chunks::<4>();
    let (lower_quads, lower) = lower.as_chunks::<4>();
    let (slope_quads, slope) = slope.as_chunks::<4>();
    for ((samples, lower), slope) in sample_quads.iter().zip(lower_quads).zip(slope_quads) {
        sums[0] = interpolated_step(sums[0], samples, lower, slope, scale);
    }
    let sum = _mm_add_ps(
        _mm_add_ps(sums[0][0], sums[0][1]),
        _mm_add_ps(sums[1][0], sums[1][1]),
    );
    let pair = _mm_add_ps(sum, _mm_movehl_ps(sum, sum));
    let sum = Complex::new(
        _mm_cvtss_f32(pair),
        _mm_cvtss_f32(_mm_shuffle_ps::<0x55>(pair, pair)),
    );
    interpolated_tail(sum, samples, lower, slope, mu)
}
