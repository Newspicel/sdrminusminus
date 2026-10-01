use std::arch::aarch64::{
    float32x4_t, float32x4x4_t, vaddq_f32, vaddvq_f32, vdupq_n_f32, vfmaq_f32, vfmaq_n_f32,
    vld1q_f32, vld1q_f32_x4, vrev64q_f32, vst1q_f32_x4,
};

use num_complex::Complex;

use super::kernel::{BLOCK_FLOATS, Plan, Tap, interpolated_plane_tail, plane_tail};

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
    ) -> [f32; BLOCK_FLOATS] {
        unsafe { real_block::<FOLD>(floats, plan) }
    }

    pub(crate) fn complex_real_block<const FOLD: bool>(
        self,
        floats: &[f32],
        plan: &Plan<f32>,
    ) -> [f32; BLOCK_FLOATS] {
        unsafe { sliding_block(floats, plan) }
    }

    pub(crate) fn complex_block<const FOLD: bool>(
        self,
        floats: &[f32],
        plan: &Plan<Complex<f32>>,
    ) -> [f32; BLOCK_FLOATS] {
        unsafe { complex_block::<FOLD>(floats, plan) }
    }

    pub(crate) unsafe fn plane_dot<const N: usize>(
        self,
        re: [*const f32; N],
        im: [*const f32; N],
        taps: &[f32],
    ) -> [Complex<f32>; N] {
        unsafe { plane_dot(re, im, taps) }
    }

    pub(crate) fn plane_interpolated(
        self,
        re: &[f32],
        im: &[f32],
        lower: &[f32],
        slope: &[f32],
        mu: f32,
    ) -> Complex<f32> {
        unsafe { plane_interpolated(re, im, lower, slope, mu) }
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
unsafe fn load_block(floats: *const f32, offset: usize) -> Block {
    let loaded = unsafe { vld1q_f32_x4(floats.add(offset)) };
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
unsafe fn samples<const FOLD: bool, C>(floats: *const f32, tap: &Tap<C>) -> Block {
    let front = unsafe { load_block(floats, tap.front) };
    if FOLD {
        add(front, unsafe { load_block(floats, tap.back) })
    } else {
        front
    }
}

#[target_feature(enable = "neon")]
fn real_block<const FOLD: bool>(floats: &[f32], plan: &Plan<f32>) -> [f32; BLOCK_FLOATS] {
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

#[target_feature(enable = "neon")]
unsafe fn slid(window: Block, floats: *const f32, offset: usize) -> Block {
    [window[1], window[2], window[3], unsafe {
        vld1q_f32(floats.add(offset))
    }]
}

#[target_feature(enable = "neon")]
fn sliding_block(floats: &[f32], plan: &Plan<f32>) -> [f32; BLOCK_FLOATS] {
    let floats = floats[..plan.floats].as_ptr();
    let mut sums = [zero(); SETS];
    for row in &plan.rows {
        let taps = &plan.row_taps[row.taps.clone()];
        unsafe { sliding_row(&mut sums, floats.add(row.offset), taps) };
    }
    store(add(add(sums[0], sums[1]), add(sums[2], sums[3])))
}

#[target_feature(enable = "neon")]
#[inline]
unsafe fn sliding_row(sums: &mut [Block; SETS], base: *const f32, taps: &[f32]) {
    let mut even = unsafe { load_block(base, 0) };
    let mut odd = if taps.len() > 1 {
        unsafe { load_block(base, 2) }
    } else {
        zero()
    };
    let (quads, rest) = taps.as_chunks::<4>();
    for (index, quad) in quads.iter().enumerate() {
        let at = 8 * index;
        if index > 0 {
            even = unsafe { slid(even, base, at + 12) };
            odd = unsafe { slid(odd, base, at + 14) };
        }
        sums[0] = add_product(sums[0], even, quad[0]);
        sums[1] = add_product(sums[1], odd, quad[1]);
        even = unsafe { slid(even, base, at + 16) };
        odd = unsafe { slid(odd, base, at + 18) };
        sums[2] = add_product(sums[2], even, quad[2]);
        sums[3] = add_product(sums[3], odd, quad[3]);
    }
    let at = 8 * quads.len();
    let moved = !quads.is_empty();
    if let Some(&tap) = rest.first() {
        if moved {
            even = unsafe { slid(even, base, at + 12) };
        }
        sums[0] = add_product(sums[0], even, tap);
    }
    if let Some(&tap) = rest.get(1) {
        if moved {
            odd = unsafe { slid(odd, base, at + 14) };
        }
        sums[1] = add_product(sums[1], odd, tap);
    }
    if let Some(&tap) = rest.get(2) {
        even = unsafe { slid(even, base, at + 16) };
        sums[2] = add_product(sums[2], even, tap);
    }
}

#[target_feature(enable = "neon")]
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
    store([
        rotated(by_re[0], by_im[0]),
        rotated(by_re[1], by_im[1]),
        rotated(by_re[2], by_im[2]),
        rotated(by_re[3], by_im[3]),
    ])
}

#[target_feature(enable = "neon")]
unsafe fn plane_dot<const N: usize>(
    re: [*const f32; N],
    im: [*const f32; N],
    taps: &[f32],
) -> [Complex<f32>; N] {
    let (blocks, _) = taps.as_chunks::<8>();
    let mut sums = [[[vdupq_n_f32(0.0); 2]; 2]; N];
    for (index, block) in blocks.iter().enumerate() {
        let (low, high) = unsafe { (vld1q_f32(block.as_ptr()), vld1q_f32(block.as_ptr().add(4))) };
        let at = 8 * index;
        for ((sum, re), im) in sums.iter_mut().zip(re).zip(im) {
            unsafe {
                let (re, im) = (re.add(at), im.add(at));
                sum[0] = [
                    vfmaq_f32(sum[0][0], vld1q_f32(re), low),
                    vfmaq_f32(sum[0][1], vld1q_f32(im), low),
                ];
                sum[1] = [
                    vfmaq_f32(sum[1][0], vld1q_f32(re.add(4)), high),
                    vfmaq_f32(sum[1][1], vld1q_f32(im.add(4)), high),
                ];
            }
        }
    }
    std::array::from_fn(|output| {
        let [low, high] = sums[output];
        let sum = Complex::new(
            vaddvq_f32(vaddq_f32(low[0], high[0])),
            vaddvq_f32(vaddq_f32(low[1], high[1])),
        );
        unsafe { plane_tail(sum, re[output], im[output], taps, 8 * blocks.len()) }
    })
}

#[target_feature(enable = "neon")]
fn interpolated_quad(
    sum: [float32x4_t; 2],
    re: &[f32; 4],
    im: &[f32; 4],
    lower: &[f32; 4],
    slope: &[f32; 4],
    mu: f32,
) -> [float32x4_t; 2] {
    let taps = vfmaq_n_f32(load(lower), load(slope), mu);
    [
        vfmaq_f32(sum[0], load(re), taps),
        vfmaq_f32(sum[1], load(im), taps),
    ]
}

#[target_feature(enable = "neon")]
fn plane_interpolated(
    re: &[f32],
    im: &[f32],
    lower: &[f32],
    slope: &[f32],
    mu: f32,
) -> Complex<f32> {
    let mut sums = [[vdupq_n_f32(0.0); 2]; 2];
    let (re_quads, re) = re.as_chunks::<4>();
    let (im_quads, im) = im.as_chunks::<4>();
    let (lower_quads, lower) = lower.as_chunks::<4>();
    let (slope_quads, slope) = slope.as_chunks::<4>();
    let quads = re_quads
        .iter()
        .zip(im_quads)
        .zip(lower_quads.iter().zip(slope_quads));
    for (index, ((re, im), (lower, slope))) in quads.enumerate() {
        if index % 2 == 0 {
            sums[0] = interpolated_quad(sums[0], re, im, lower, slope, mu);
        } else {
            sums[1] = interpolated_quad(sums[1], re, im, lower, slope, mu);
        }
    }
    let sum = Complex::new(
        vaddvq_f32(vaddq_f32(sums[0][0], sums[1][0])),
        vaddvq_f32(vaddq_f32(sums[0][1], sums[1][1])),
    );
    interpolated_plane_tail(sum, re, im, lower, slope, mu)
}
