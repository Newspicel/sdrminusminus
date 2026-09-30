use num_complex::Complex;

use super::{Table, phasor};

const LANES: usize = 4;

type Block = [Complex<f32>; LANES];

pub(super) fn mix_into(
    table: &Table,
    phase: u64,
    step: u64,
    input: &[Complex<f32>],
    out: &mut [Complex<f32>],
) -> u64 {
    let (sources, source_tail) = input.as_chunks::<LANES>();
    let (targets, target_tail) = out.as_chunks_mut::<LANES>();
    let mut lanes = Lanes::new(phase, step);
    for (source, target) in sources.iter().zip(targets) {
        lanes.mix(table, source, target);
    }
    let mut phase = lanes.phase;
    for (source, target) in source_tail.iter().zip(target_tail) {
        *target = *source * phasor(table, phase);
        phase = phase.wrapping_add(step);
    }
    phase
}

pub(super) fn mix(table: &Table, phase: u64, step: u64, samples: &mut [Complex<f32>]) -> u64 {
    let (blocks, tail) = samples.as_chunks_mut::<LANES>();
    let mut lanes = Lanes::new(phase, step);
    for block in blocks {
        let source = *block;
        lanes.mix(table, &source, block);
    }
    let mut phase = lanes.phase;
    for sample in tail {
        *sample *= phasor(table, phase);
        phase = phase.wrapping_add(step);
    }
    phase
}

fn lane_offsets(step: u64) -> [u64; LANES] {
    std::array::from_fn(|lane| step.wrapping_mul(lane as u64))
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[inline(always)]
fn entry(table: &Table, phase: u64) -> &Complex<f32> {
    &table[(phase >> (super::LOOKUP_SHIFT + super::INDEX_SHIFT)) as usize]
}

#[cfg(target_arch = "aarch64")]
struct Lanes {
    phase: u64,
    stride: u64,
    offsets: [u64; LANES],
    low: std::arch::aarch64::uint64x2_t,
    high: std::arch::aarch64::uint64x2_t,
    advance: std::arch::aarch64::uint64x2_t,
}

#[cfg(target_arch = "aarch64")]
impl Lanes {
    fn new(phase: u64, step: u64) -> Self {
        use std::arch::aarch64::*;
        let offsets = lane_offsets(step);
        let stride = step.wrapping_mul(LANES as u64);
        unsafe {
            let base = vdupq_n_u64(phase);
            Self {
                phase,
                stride,
                offsets,
                low: vaddq_u64(base, vld1q_u64(offsets.as_ptr())),
                high: vaddq_u64(base, vld1q_u64(offsets.as_ptr().add(2))),
                advance: vdupq_n_u64(stride),
            }
        }
    }

    #[inline(always)]
    fn mix(&mut self, table: &Table, source: &Block, target: &mut Block) {
        use std::arch::aarch64::*;

        use super::{DELTA_PER_STEP, FRACTION_MASK, LOOKUP_SHIFT, SIXTH};
        let load = |lane: usize| {
            let slot = entry(table, self.phase.wrapping_add(self.offsets[lane]));
            unsafe { vld1_f32(std::ptr::from_ref(slot).cast()) }
        };
        unsafe {
            let front = vcombine_f32(load(0), load(1));
            let back = vcombine_f32(load(2), load(3));
            let tops = vcombine_u32(
                vshrn_n_u64::<{ LOOKUP_SHIFT as i32 }>(self.low),
                vshrn_n_u64::<{ LOOKUP_SHIFT as i32 }>(self.high),
            );
            let fraction = vcvtq_f32_u32(vandq_u32(tops, vdupq_n_u32(FRACTION_MASK)));
            let delta = vmulq_n_f32(fraction, DELTA_PER_STEP);
            let first_re = vuzp1q_f32(front, back);
            let first_im = vuzp2q_f32(front, back);
            let one = vdupq_n_f32(1.0);
            let square = vmulq_f32(delta, delta);
            let sin = vmulq_f32(delta, vsubq_f32(one, vmulq_n_f32(square, SIXTH)));
            let cos = vsubq_f32(one, vmulq_n_f32(square, 0.5));
            let wave_re = vsubq_f32(vmulq_f32(first_re, cos), vmulq_f32(first_im, sin));
            let wave_im = vaddq_f32(vmulq_f32(first_im, cos), vmulq_f32(first_re, sin));
            let x = vld2q_f32(source.as_ptr().cast());
            let re = vsubq_f32(vmulq_f32(x.0, wave_re), vmulq_f32(x.1, wave_im));
            let im = vaddq_f32(vmulq_f32(x.0, wave_im), vmulq_f32(x.1, wave_re));
            vst2q_f32(target.as_mut_ptr().cast(), float32x4x2_t(re, im));
            self.low = vaddq_u64(self.low, self.advance);
            self.high = vaddq_u64(self.high, self.advance);
        }
        self.phase = self.phase.wrapping_add(self.stride);
    }
}

#[cfg(target_arch = "x86_64")]
struct Lanes {
    phase: u64,
    stride: u64,
    offsets: [u64; LANES],
    low: std::arch::x86_64::__m128i,
    high: std::arch::x86_64::__m128i,
    advance: std::arch::x86_64::__m128i,
}

#[cfg(target_arch = "x86_64")]
impl Lanes {
    fn new(phase: u64, step: u64) -> Self {
        use std::arch::x86_64::*;
        let offsets = lane_offsets(step);
        let stride = step.wrapping_mul(LANES as u64);
        unsafe {
            let base = _mm_set1_epi64x(phase as i64);
            Self {
                phase,
                stride,
                offsets,
                low: _mm_add_epi64(base, _mm_loadu_si128(offsets.as_ptr().cast())),
                high: _mm_add_epi64(base, _mm_loadu_si128(offsets.as_ptr().add(2).cast())),
                advance: _mm_set1_epi64x(stride as i64),
            }
        }
    }

    #[inline(always)]
    fn mix(&mut self, table: &Table, source: &Block, target: &mut Block) {
        use std::arch::x86_64::*;

        use super::{DELTA_PER_STEP, FRACTION_MASK, SIXTH};
        let load = |lane: usize| {
            let slot = entry(table, self.phase.wrapping_add(self.offsets[lane]));
            unsafe { _mm_loadl_epi64(std::ptr::from_ref(slot).cast()) }
        };
        unsafe {
            let front = _mm_castsi128_ps(_mm_unpacklo_epi64(load(0), load(1)));
            let back = _mm_castsi128_ps(_mm_unpacklo_epi64(load(2), load(3)));
            let tops = _mm_castps_si128(_mm_shuffle_ps::<0xDD>(
                _mm_castsi128_ps(self.low),
                _mm_castsi128_ps(self.high),
            ));
            let fraction =
                _mm_cvtepi32_ps(_mm_and_si128(tops, _mm_set1_epi32(FRACTION_MASK as i32)));
            let delta = _mm_mul_ps(fraction, _mm_set1_ps(DELTA_PER_STEP));
            let first_re = _mm_shuffle_ps::<0x88>(front, back);
            let first_im = _mm_shuffle_ps::<0xDD>(front, back);
            let one = _mm_set1_ps(1.0);
            let square = _mm_mul_ps(delta, delta);
            let sin = _mm_mul_ps(
                delta,
                _mm_sub_ps(one, _mm_mul_ps(square, _mm_set1_ps(SIXTH))),
            );
            let cos = _mm_sub_ps(one, _mm_mul_ps(square, _mm_set1_ps(0.5)));
            let wave_re = _mm_sub_ps(_mm_mul_ps(first_re, cos), _mm_mul_ps(first_im, sin));
            let wave_im = _mm_add_ps(_mm_mul_ps(first_im, cos), _mm_mul_ps(first_re, sin));
            let pair_front = _mm_loadu_ps(source.as_ptr().cast());
            let pair_back = _mm_loadu_ps(source.as_ptr().add(2).cast());
            let x_re = _mm_shuffle_ps::<0x88>(pair_front, pair_back);
            let x_im = _mm_shuffle_ps::<0xDD>(pair_front, pair_back);
            let re = _mm_sub_ps(_mm_mul_ps(x_re, wave_re), _mm_mul_ps(x_im, wave_im));
            let im = _mm_add_ps(_mm_mul_ps(x_re, wave_im), _mm_mul_ps(x_im, wave_re));
            _mm_storeu_ps(target.as_mut_ptr().cast(), _mm_unpacklo_ps(re, im));
            _mm_storeu_ps(target.as_mut_ptr().add(2).cast(), _mm_unpackhi_ps(re, im));
            self.low = _mm_add_epi64(self.low, self.advance);
            self.high = _mm_add_epi64(self.high, self.advance);
        }
        self.phase = self.phase.wrapping_add(self.stride);
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
struct Lanes {
    phase: u64,
    stride: u64,
    offsets: [u64; LANES],
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
impl Lanes {
    fn new(phase: u64, step: u64) -> Self {
        Self {
            phase,
            stride: step.wrapping_mul(LANES as u64),
            offsets: lane_offsets(step),
        }
    }

    #[inline(always)]
    fn mix(&mut self, table: &Table, source: &Block, target: &mut Block) {
        for ((target, source), offset) in target.iter_mut().zip(source).zip(&self.offsets) {
            *target = *source * phasor(table, self.phase.wrapping_add(*offset));
        }
        self.phase = self.phase.wrapping_add(self.stride);
    }
}
