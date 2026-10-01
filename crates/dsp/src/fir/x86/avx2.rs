use std::arch::x86_64::{
    __m256, _mm_add_ps, _mm_cvtss_f32, _mm_movehdup_ps, _mm_movehl_ps, _mm256_add_ps,
    _mm256_addsub_ps, _mm256_castps256_ps128, _mm256_extractf128_ps, _mm256_fmadd_ps,
    _mm256_loadu_ps, _mm256_permute_ps, _mm256_set1_ps, _mm256_setzero_ps, _mm256_storeu_ps,
};

use num_complex::Complex;

use crate::fir::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_plane_tail, plane_tail};

type Block = [__m256; 2];

const SETS: usize = 4;

#[target_feature(enable = "avx2,fma")]
fn zero() -> Block {
    [_mm256_setzero_ps(); 2]
}

#[target_feature(enable = "avx2,fma")]
unsafe fn load_block(floats: *const f32, offset: usize) -> Block {
    unsafe {
        [
            _mm256_loadu_ps(floats.add(offset)),
            _mm256_loadu_ps(floats.add(offset + BLOCK_FLOATS / 2)),
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
unsafe fn samples<const FOLD: bool, C>(floats: *const f32, tap: &Tap<C>) -> Block {
    let front = unsafe { load_block(floats, tap.front) };
    if FOLD {
        add(front, unsafe { load_block(floats, tap.back) })
    } else {
        front
    }
}

#[target_feature(enable = "avx2,fma")]
pub(super) fn real_block<const FOLD: bool>(
    floats: &[f32],
    plan: &Plan<f32>,
) -> [f32; BLOCK_FLOATS] {
    let floats = floats[..plan.floats].as_ptr();
    let mut sets = [zero(); SETS];
    let (groups, rest) = plan.taps.as_chunks::<SETS>();
    for group in groups {
        for (set, tap) in sets.iter_mut().zip(group) {
            *set = add_product(
                *set,
                unsafe { samples::<FOLD, f32>(floats, tap) },
                tap.value,
            );
        }
    }
    for tap in rest {
        sets[0] = add_product(
            sets[0],
            unsafe { samples::<FOLD, f32>(floats, tap) },
            tap.value,
        );
    }
    store(add(add(sets[0], sets[1]), add(sets[2], sets[3])))
}

#[target_feature(enable = "avx2,fma")]
unsafe fn complex_step<const FOLD: bool>(
    sums: [Block; 2],
    floats: *const f32,
    tap: &Tap<Complex<f32>>,
) -> [Block; 2] {
    let samples = unsafe { samples::<FOLD, Complex<f32>>(floats, tap) };
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
    let floats = floats[..plan.floats].as_ptr();
    let mut sets = [[zero(); 2]; SETS / 2];
    let (groups, rest) = plan.taps.as_chunks::<{ SETS / 2 }>();
    for group in groups {
        for (set, tap) in sets.iter_mut().zip(group) {
            *set = unsafe { complex_step::<FOLD>(*set, floats, tap) };
        }
    }
    for tap in rest {
        sets[0] = unsafe { complex_step::<FOLD>(sets[0], floats, tap) };
    }
    let by_re = add(sets[0][0], sets[1][0]);
    let by_im = add(sets[0][1], sets[1][1]);
    store([rotated(by_re[0], by_im[0]), rotated(by_re[1], by_im[1])])
}

#[target_feature(enable = "avx2,fma")]
fn horizontal(sum: __m256) -> f32 {
    let half = _mm_add_ps(_mm256_castps256_ps128(sum), _mm256_extractf128_ps::<1>(sum));
    let pair = _mm_add_ps(half, _mm_movehl_ps(half, half));
    _mm_cvtss_f32(_mm_add_ps(pair, _mm_movehdup_ps(pair)))
}

#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn plane_dot<const N: usize>(
    re: [*const f32; N],
    im: [*const f32; N],
    taps: &[f32],
) -> [Complex<f32>; N] {
    let (blocks, _) = taps.as_chunks::<8>();
    let mut sums = [[_mm256_setzero_ps(); 2]; N];
    for (index, block) in blocks.iter().enumerate() {
        let taps = unsafe { _mm256_loadu_ps(block.as_ptr()) };
        let at = 8 * index;
        for ((sum, re), im) in sums.iter_mut().zip(re).zip(im) {
            unsafe {
                *sum = [
                    _mm256_fmadd_ps(_mm256_loadu_ps(re.add(at)), taps, sum[0]),
                    _mm256_fmadd_ps(_mm256_loadu_ps(im.add(at)), taps, sum[1]),
                ];
            }
        }
    }
    std::array::from_fn(|output| {
        let sum = Complex::new(horizontal(sums[output][0]), horizontal(sums[output][1]));
        unsafe { plane_tail(sum, re[output], im[output], taps, 8 * blocks.len()) }
    })
}

#[target_feature(enable = "avx2,fma")]
pub(super) fn plane_interpolated(
    re: &[f32],
    im: &[f32],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let scale = _mm256_set1_ps(mu);
    let (re_blocks, re) = re.as_chunks::<8>();
    let (im_blocks, im) = im.as_chunks::<8>();
    let (lower_blocks, lower) = lower.as_chunks::<8>();
    let (slope_blocks, slope) = slope.as_chunks::<8>();
    let mut sums = [_mm256_setzero_ps(); 2];
    let blocks = re_blocks
        .iter()
        .zip(im_blocks)
        .zip(lower_blocks.iter().zip(slope_blocks));
    for ((re, im), (lower, slope)) in blocks {
        unsafe {
            let taps = _mm256_fmadd_ps(
                _mm256_loadu_ps(slope.as_ptr()),
                scale,
                _mm256_loadu_ps(lower.as_ptr()),
            );
            sums = [
                _mm256_fmadd_ps(_mm256_loadu_ps(re.as_ptr()), taps, sums[0]),
                _mm256_fmadd_ps(_mm256_loadu_ps(im.as_ptr()), taps, sums[1]),
            ];
        }
    }
    let sum = Complex::new(horizontal(sums[0]), horizontal(sums[1]));
    interpolated_plane_tail(sum, re, im, lower, slope, mu)
}
