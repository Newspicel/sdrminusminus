#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(super) fn step(
    metrics: &[i32; 64],
    next: &mut [i32; 64],
    signs: &[[i32; 32]; 2],
    values: [i32; 2],
) -> u64 {
    use std::arch::aarch64::*;
    unsafe {
        let first = vdupq_n_s32(values[0]);
        let second = vdupq_n_s32(values[1]);
        let weights = vld1q_u32([1, 2, 4, 8].as_ptr());
        let mut decisions = 0u64;
        for group in 0..8 {
            let offset = group * 4;
            let previous = vld2q_s32(metrics.as_ptr().add(offset * 2));
            let mask0 = vld1q_s32(signs[0].as_ptr().add(offset));
            let mask1 = vld1q_s32(signs[1].as_ptr().add(offset));
            let branch = vaddq_s32(
                vsubq_s32(veorq_s32(first, mask0), mask0),
                vsubq_s32(veorq_s32(second, mask1), mask1),
            );
            let a = vaddq_s32(previous.0, branch);
            let b = vsubq_s32(previous.1, branch);
            let c = vsubq_s32(previous.0, branch);
            let d = vaddq_s32(previous.1, branch);
            vst1q_s32(next.as_mut_ptr().add(offset), vmaxq_s32(a, b));
            vst1q_s32(next.as_mut_ptr().add(offset + 32), vmaxq_s32(c, d));
            decisions |= u64::from(vaddvq_u32(vandq_u32(vcgtq_s32(b, a), weights))) << offset;
            decisions |=
                u64::from(vaddvq_u32(vandq_u32(vcgtq_s32(d, c), weights))) << (offset + 32);
        }
        decisions
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(super) fn step(
    metrics: &[i32; 64],
    next: &mut [i32; 64],
    signs: &[[i32; 32]; 2],
    values: [i32; 2],
) -> u64 {
    use std::arch::x86_64::*;
    unsafe {
        let first = _mm_set1_epi32(values[0]);
        let second = _mm_set1_epi32(values[1]);
        let mut decisions = 0u64;
        for group in 0..8 {
            let offset = group * 4;
            let p = _mm_castsi128_ps(_mm_loadu_si128(metrics.as_ptr().add(offset * 2).cast()));
            let q = _mm_castsi128_ps(_mm_loadu_si128(metrics.as_ptr().add(offset * 2 + 4).cast()));
            let even = _mm_castps_si128(_mm_shuffle_ps::<0x88>(p, q));
            let odd = _mm_castps_si128(_mm_shuffle_ps::<0xdd>(p, q));
            let mask0 = _mm_loadu_si128(signs[0].as_ptr().add(offset).cast());
            let mask1 = _mm_loadu_si128(signs[1].as_ptr().add(offset).cast());
            let branch = _mm_add_epi32(
                _mm_sub_epi32(_mm_xor_si128(first, mask0), mask0),
                _mm_sub_epi32(_mm_xor_si128(second, mask1), mask1),
            );
            let a = _mm_add_epi32(even, branch);
            let b = _mm_sub_epi32(odd, branch);
            let c = _mm_sub_epi32(even, branch);
            let d = _mm_add_epi32(odd, branch);
            let low = _mm_cmpgt_epi32(b, a);
            let high = _mm_cmpgt_epi32(d, c);
            let best0 = _mm_or_si128(_mm_and_si128(low, b), _mm_andnot_si128(low, a));
            let best1 = _mm_or_si128(_mm_and_si128(high, d), _mm_andnot_si128(high, c));
            _mm_storeu_si128(next.as_mut_ptr().add(offset).cast(), best0);
            _mm_storeu_si128(next.as_mut_ptr().add(offset + 32).cast(), best1);
            decisions |= (_mm_movemask_ps(_mm_castsi128_ps(low)) as u64) << offset;
            decisions |= (_mm_movemask_ps(_mm_castsi128_ps(high)) as u64) << (offset + 32);
        }
        decisions
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline(always)]
pub(super) fn step(
    metrics: &[i32; 64],
    next: &mut [i32; 64],
    signs: &[[i32; 32]; 2],
    values: [i32; 2],
) -> u64 {
    let mut decisions = 0u64;
    for i in 0..32 {
        let branch =
            ((values[0] ^ signs[0][i]) - signs[0][i]) + ((values[1] ^ signs[1][i]) - signs[1][i]);
        let a = metrics[2 * i] + branch;
        let b = metrics[2 * i + 1] - branch;
        let c = metrics[2 * i] - branch;
        let d = metrics[2 * i + 1] + branch;
        next[i] = a.max(b);
        next[i + 32] = c.max(d);
        decisions |= u64::from(b > a) << i;
        decisions |= u64::from(d > c) << (i + 32);
    }
    decisions
}
