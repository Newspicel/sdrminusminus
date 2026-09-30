use std::arch::x86_64::{
    __m128, __m256, _mm_add_ps, _mm_cvtss_f32, _mm_fmadd_ps, _mm_loadu_ps, _mm_movehdup_ps,
    _mm_movehl_ps, _mm_set1_ps, _mm256_add_ps, _mm256_addsub_ps, _mm256_castps128_ps256,
    _mm256_castps256_ps128, _mm256_extractf128_ps, _mm256_fmadd_ps, _mm256_loadu_ps,
    _mm256_permute_ps, _mm256_permutevar8x32_ps, _mm256_set1_ps, _mm256_setr_epi32,
    _mm256_setzero_ps, _mm256_storeu_ps,
};

use num_complex::Complex;

use crate::fir::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_tail};

type Block = [__m256; 2];

const SETS: usize = 4;

#[target_feature(enable = "avx2,fma")]
fn zero() -> Block {
    [_mm256_setzero_ps(); 2]
}

#[target_feature(enable = "avx2,fma")]
fn load_block(floats: &[f32], offset: usize) -> Block {
    let (low, high) = floats[offset..offset + BLOCK_FLOATS].split_at(BLOCK_FLOATS / 2);
    unsafe {
        [
            _mm256_loadu_ps(low.as_ptr()),
            _mm256_loadu_ps(high.as_ptr()),
        ]
    }
}

#[target_feature(enable = "avx2,fma")]
fn store(block: Block) -> [f32; BLOCK_FLOATS] {
    let mut lanes = [0.0; BLOCK_FLOATS];
    let (low, high) = lanes.split_at_mut(BLOCK_FLOATS / 2);
    unsafe {
        _mm256_storeu_ps(low.as_mut_ptr(), block[0]);
        _mm256_storeu_ps(high.as_mut_ptr(), block[1]);
    }
    lanes
}

#[target_feature(enable = "avx2,fma")]
fn add(a: Block, b: Block) -> Block {
    [_mm256_add_ps(a[0], b[0]), _mm256_add_ps(a[1], b[1])]
}

#[target_feature(enable = "avx2,fma")]
fn add_product(sum: Block, samples: Block, tap: f32) -> Block {
    let tap = _mm256_set1_ps(tap);
    [
        _mm256_fmadd_ps(samples[0], tap, sum[0]),
        _mm256_fmadd_ps(samples[1], tap, sum[1]),
    ]
}

#[target_feature(enable = "avx2,fma")]
fn samples<const FOLD: bool, C>(floats: &[f32], tap: &Tap<C>, scale: usize) -> Block {
    let front = load_block(floats, tap.front * scale);
    if FOLD {
        add(front, load_block(floats, tap.back * scale))
    } else {
        front
    }
}

#[target_feature(enable = "avx2,fma")]
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
    store(add(add(sets[0], sets[1]), add(sets[2], sets[3])))
}

#[target_feature(enable = "avx2,fma")]
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

#[target_feature(enable = "avx2,fma")]
fn rotated(by_re: __m256, by_im: __m256) -> __m256 {
    _mm256_addsub_ps(by_re, _mm256_permute_ps::<0xB1>(by_im))
}

#[target_feature(enable = "avx2,fma")]
pub(super) fn complex_block<const FOLD: bool>(
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
    store([rotated(by_re[0], by_im[0]), rotated(by_re[1], by_im[1])])
}

#[target_feature(enable = "avx2,fma")]
fn load_quad(values: &[f32; 4]) -> __m128 {
    unsafe { _mm_loadu_ps(values.as_ptr()) }
}

#[target_feature(enable = "avx2,fma")]
fn interpolated_step(
    sum: __m256,
    samples: &[Complex<f32>; 4],
    lower: &[f32; 4],
    slope: &[f32; 4],
    mu: __m128,
) -> __m256 {
    let samples = unsafe { _mm256_loadu_ps(samples.as_ptr().cast()) };
    let taps = _mm_fmadd_ps(load_quad(slope), mu, load_quad(lower));
    let paired = _mm256_permutevar8x32_ps(
        _mm256_castps128_ps256(taps),
        _mm256_setr_epi32(0, 0, 1, 1, 2, 2, 3, 3),
    );
    _mm256_fmadd_ps(samples, paired, sum)
}

#[target_feature(enable = "avx2,fma")]
pub(super) fn interpolated_dot(
    samples: &[Complex<f32>],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let scale = _mm_set1_ps(mu);
    let (sample_blocks, samples) = samples.as_chunks::<16>();
    let (lower_blocks, lower) = lower.as_chunks::<16>();
    let (slope_blocks, slope) = slope.as_chunks::<16>();
    let mut sums = [_mm256_setzero_ps(); 4];
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
    let sum = _mm256_add_ps(
        _mm256_add_ps(sums[0], sums[1]),
        _mm256_add_ps(sums[2], sums[3]),
    );
    let half = _mm_add_ps(_mm256_castps256_ps128(sum), _mm256_extractf128_ps::<1>(sum));
    let pair = _mm_add_ps(half, _mm_movehl_ps(half, half));
    let sum = Complex::new(_mm_cvtss_f32(pair), _mm_cvtss_f32(_mm_movehdup_ps(pair)));
    interpolated_tail(sum, samples, lower, slope, mu)
}
