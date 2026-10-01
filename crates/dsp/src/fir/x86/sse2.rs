use std::arch::x86_64::{
    __m128, _mm_add_ps, _mm_cvtss_f32, _mm_loadu_ps, _mm_movehl_ps, _mm_mul_ps, _mm_set1_ps,
    _mm_setr_ps, _mm_setzero_ps, _mm_shuffle_ps, _mm_storeu_ps,
};

use num_complex::Complex;

use crate::fir::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_plane_tail, plane_tail};

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
unsafe fn load_block(floats: *const f32, offset: usize) -> Block {
    unsafe {
        [
            _mm_loadu_ps(floats.add(offset)),
            _mm_loadu_ps(floats.add(offset + 4)),
            _mm_loadu_ps(floats.add(offset + 8)),
            _mm_loadu_ps(floats.add(offset + 12)),
        ]
    }
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
unsafe fn samples<const FOLD: bool, C>(floats: *const f32, tap: &Tap<C>) -> Block {
    let front = unsafe { load_block(floats, tap.front) };
    if FOLD {
        add(front, unsafe { load_block(floats, tap.back) })
    } else {
        front
    }
}

#[target_feature(enable = "sse2")]
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
    let floats = floats[..plan.floats].as_ptr();
    let (mut by_re, mut by_im) = (zero(), zero());
    for tap in &plan.taps {
        let samples = unsafe { samples::<FOLD, Complex<f32>>(floats, tap) };
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
fn horizontal(sum: __m128) -> f32 {
    let pair = _mm_add_ps(sum, _mm_movehl_ps(sum, sum));
    _mm_cvtss_f32(_mm_add_ps(pair, _mm_shuffle_ps::<0x55>(pair, pair)))
}

#[target_feature(enable = "sse2")]
pub(super) unsafe fn plane_dot<const N: usize>(
    re: [*const f32; N],
    im: [*const f32; N],
    taps: &[f32],
) -> [Complex<f32>; N] {
    let (blocks, _) = taps.as_chunks::<8>();
    let mut sums = [[[_mm_setzero_ps(); 2]; 2]; N];
    for (index, block) in blocks.iter().enumerate() {
        let (low, high) = unsafe {
            (
                _mm_loadu_ps(block.as_ptr()),
                _mm_loadu_ps(block.as_ptr().add(4)),
            )
        };
        let at = 8 * index;
        for ((sum, re), im) in sums.iter_mut().zip(re).zip(im) {
            unsafe {
                let (re, im) = (re.add(at), im.add(at));
                sum[0] = [
                    _mm_add_ps(sum[0][0], _mm_mul_ps(_mm_loadu_ps(re), low)),
                    _mm_add_ps(sum[0][1], _mm_mul_ps(_mm_loadu_ps(im), low)),
                ];
                sum[1] = [
                    _mm_add_ps(sum[1][0], _mm_mul_ps(_mm_loadu_ps(re.add(4)), high)),
                    _mm_add_ps(sum[1][1], _mm_mul_ps(_mm_loadu_ps(im.add(4)), high)),
                ];
            }
        }
    }
    std::array::from_fn(|output| {
        let [low, high] = sums[output];
        let sum = Complex::new(
            horizontal(_mm_add_ps(low[0], high[0])),
            horizontal(_mm_add_ps(low[1], high[1])),
        );
        unsafe { plane_tail(sum, re[output], im[output], taps, 8 * blocks.len()) }
    })
}

#[target_feature(enable = "sse2")]
pub(super) fn plane_interpolated(
    re: &[f32],
    im: &[f32],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let scale = _mm_set1_ps(mu);
    let (re_quads, re) = re.as_chunks::<4>();
    let (im_quads, im) = im.as_chunks::<4>();
    let (lower_quads, lower) = lower.as_chunks::<4>();
    let (slope_quads, slope) = slope.as_chunks::<4>();
    let mut sums = [_mm_setzero_ps(); 2];
    let quads = re_quads
        .iter()
        .zip(im_quads)
        .zip(lower_quads.iter().zip(slope_quads));
    for ((re, im), (lower, slope)) in quads {
        let taps = _mm_add_ps(load(lower), _mm_mul_ps(load(slope), scale));
        sums = [
            _mm_add_ps(sums[0], _mm_mul_ps(load(re), taps)),
            _mm_add_ps(sums[1], _mm_mul_ps(load(im), taps)),
        ];
    }
    let sum = Complex::new(horizontal(sums[0]), horizontal(sums[1]));
    interpolated_plane_tail(sum, re, im, lower, slope, mu)
}
