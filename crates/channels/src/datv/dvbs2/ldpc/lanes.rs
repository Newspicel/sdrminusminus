pub(super) const STRIDE: usize = 368;
const LIMIT: i16 = 16_383;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kernel {
    #[cfg(test)]
    Scalar,
    Vector,
    #[cfg(target_arch = "x86_64")]
    Wide,
}

impl Kernel {
    pub(super) fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2") {
            return Self::Wide;
        }
        Self::Vector
    }

    pub(super) fn update(
        self,
        gathered: &mut [i16],
        extrinsic: &mut [i16],
        messages: &mut [i16],
    ) -> u32 {
        let rows = messages.len() - messages.len() % STRIDE;
        let messages = &mut messages[..rows];
        let gathered = &mut gathered[..rows];
        let extrinsic = &mut extrinsic[..rows];
        match self {
            #[cfg(test)]
            Self::Scalar => scalar(gathered, extrinsic, messages),
            Self::Vector => vector(gathered, extrinsic, messages),
            #[cfg(target_arch = "x86_64")]
            Self::Wide => unsafe { wide(gathered, extrinsic, messages) },
        }
    }
}

#[cfg(any(test, not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
const fn scale(magnitude: i16) -> i16 {
    let scaled = magnitude - (magnitude >> 3);
    if scaled < LIMIT { scaled } else { LIMIT }
}

#[cfg(any(test, not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
fn scalar(gathered: &mut [i16], extrinsic: &mut [i16], messages: &mut [i16]) -> u32 {
    let mut unsatisfied = 0;
    for lane in 0..STRIDE {
        let mut first = i16::MAX;
        let mut second = i16::MAX;
        let mut sign = 0i16;
        let mut parity = 0i16;
        for at in (lane..messages.len()).step_by(STRIDE) {
            let total = gathered[at];
            let value = total.saturating_sub(messages[at]);
            extrinsic[at] = value;
            sign ^= value;
            parity ^= total;
            let magnitude = value.saturating_abs();
            second = second.min(first.max(magnitude));
            first = first.min(magnitude);
        }
        unsatisfied += u32::from(parity < 0);
        let near = scale(first);
        let far = scale(second);
        for at in (lane..messages.len()).step_by(STRIDE) {
            let value = extrinsic[at];
            let magnitude = if value.saturating_abs() == first {
                far
            } else {
                near
            };
            let flip = (sign ^ value) >> 15;
            let message = (magnitude ^ flip) - flip;
            gathered[at] = message.saturating_sub(messages[at]);
            messages[at] = message;
        }
    }
    unsatisfied
}

#[cfg(target_arch = "aarch64")]
fn vector(gathered: &mut [i16], extrinsic: &mut [i16], messages: &mut [i16]) -> u32 {
    use std::arch::aarch64::*;
    let rows = messages.len();
    let gathered = gathered.as_mut_ptr();
    let extrinsic = extrinsic.as_mut_ptr();
    let messages = messages.as_mut_ptr();
    unsafe {
        let limit = vdupq_n_s16(LIMIT);
        let mut count = vdupq_n_s16(0);
        for lane in (0..STRIDE).step_by(8) {
            let mut first = vdupq_n_s16(i16::MAX);
            let mut second = first;
            let mut sign = vdupq_n_s16(0);
            let mut parity = sign;
            for at in (lane..rows).step_by(STRIDE) {
                let total = vld1q_s16(gathered.add(at));
                let value = vqsubq_s16(total, vld1q_s16(messages.add(at)));
                vst1q_s16(extrinsic.add(at), value);
                sign = veorq_s16(sign, value);
                parity = veorq_s16(parity, total);
                let magnitude = vqabsq_s16(value);
                second = vminq_s16(second, vmaxq_s16(first, magnitude));
                first = vminq_s16(first, magnitude);
            }
            count = vsubq_s16(count, vshrq_n_s16::<15>(parity));
            let near = vminq_s16(vsubq_s16(first, vshrq_n_s16::<3>(first)), limit);
            let far = vminq_s16(vsubq_s16(second, vshrq_n_s16::<3>(second)), limit);
            for at in (lane..rows).step_by(STRIDE) {
                let value = vld1q_s16(extrinsic.add(at));
                let magnitude = vbslq_s16(vceqq_s16(vqabsq_s16(value), first), far, near);
                let flip = vshrq_n_s16::<15>(veorq_s16(sign, value));
                let message = vsubq_s16(veorq_s16(magnitude, flip), flip);
                let previous = vld1q_s16(messages.add(at));
                vst1q_s16(gathered.add(at), vqsubq_s16(message, previous));
                vst1q_s16(messages.add(at), message);
            }
        }
        u32::from(vaddvq_s16(count).unsigned_abs())
    }
}

#[cfg(target_arch = "x86_64")]
fn vector(gathered: &mut [i16], extrinsic: &mut [i16], messages: &mut [i16]) -> u32 {
    use std::arch::x86_64::*;
    let rows = messages.len();
    let gathered = gathered.as_mut_ptr();
    let extrinsic = extrinsic.as_mut_ptr();
    let messages = messages.as_mut_ptr();
    unsafe {
        let zero = _mm_setzero_si128();
        let limit = _mm_set1_epi16(LIMIT);
        let mut count = zero;
        for lane in (0..STRIDE).step_by(8) {
            let mut first = _mm_set1_epi16(i16::MAX);
            let mut second = first;
            let mut sign = zero;
            let mut parity = zero;
            for at in (lane..rows).step_by(STRIDE) {
                let total = _mm_loadu_si128(gathered.add(at).cast());
                let value = _mm_subs_epi16(total, _mm_loadu_si128(messages.add(at).cast()));
                _mm_storeu_si128(extrinsic.add(at).cast(), value);
                sign = _mm_xor_si128(sign, value);
                parity = _mm_xor_si128(parity, total);
                let magnitude = _mm_max_epi16(value, _mm_subs_epi16(zero, value));
                second = _mm_min_epi16(second, _mm_max_epi16(first, magnitude));
                first = _mm_min_epi16(first, magnitude);
            }
            count = _mm_sub_epi16(count, _mm_srai_epi16::<15>(parity));
            let near = _mm_min_epi16(_mm_sub_epi16(first, _mm_srai_epi16::<3>(first)), limit);
            let far = _mm_min_epi16(_mm_sub_epi16(second, _mm_srai_epi16::<3>(second)), limit);
            for at in (lane..rows).step_by(STRIDE) {
                let value = _mm_loadu_si128(extrinsic.add(at).cast());
                let magnitude = _mm_max_epi16(value, _mm_subs_epi16(zero, value));
                let tied = _mm_cmpeq_epi16(magnitude, first);
                let chosen = _mm_or_si128(_mm_and_si128(tied, far), _mm_andnot_si128(tied, near));
                let flip = _mm_srai_epi16::<15>(_mm_xor_si128(sign, value));
                let message = _mm_sub_epi16(_mm_xor_si128(chosen, flip), flip);
                let previous = _mm_loadu_si128(messages.add(at).cast());
                _mm_storeu_si128(gathered.add(at).cast(), _mm_subs_epi16(message, previous));
                _mm_storeu_si128(messages.add(at).cast(), message);
            }
        }
        let mut lanes = [0i16; 8];
        _mm_storeu_si128(lanes.as_mut_ptr().cast(), count);
        lanes
            .iter()
            .map(|&lane| u32::from(lane.unsigned_abs()))
            .sum()
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn wide(gathered: &mut [i16], extrinsic: &mut [i16], messages: &mut [i16]) -> u32 {
    use std::arch::x86_64::*;
    let rows = messages.len();
    let gathered = gathered.as_mut_ptr();
    let extrinsic = extrinsic.as_mut_ptr();
    let messages = messages.as_mut_ptr();
    unsafe {
        let zero = _mm256_setzero_si256();
        let limit = _mm256_set1_epi16(LIMIT);
        let mut count = zero;
        for lane in (0..STRIDE).step_by(16) {
            let mut first = _mm256_set1_epi16(i16::MAX);
            let mut second = first;
            let mut sign = zero;
            let mut parity = zero;
            for at in (lane..rows).step_by(STRIDE) {
                let total = _mm256_loadu_si256(gathered.add(at).cast());
                let value = _mm256_subs_epi16(total, _mm256_loadu_si256(messages.add(at).cast()));
                _mm256_storeu_si256(extrinsic.add(at).cast(), value);
                sign = _mm256_xor_si256(sign, value);
                parity = _mm256_xor_si256(parity, total);
                let magnitude = _mm256_max_epi16(value, _mm256_subs_epi16(zero, value));
                second = _mm256_min_epi16(second, _mm256_max_epi16(first, magnitude));
                first = _mm256_min_epi16(first, magnitude);
            }
            count = _mm256_sub_epi16(count, _mm256_srai_epi16::<15>(parity));
            let near = _mm256_min_epi16(
                _mm256_sub_epi16(first, _mm256_srai_epi16::<3>(first)),
                limit,
            );
            let far = _mm256_min_epi16(
                _mm256_sub_epi16(second, _mm256_srai_epi16::<3>(second)),
                limit,
            );
            for at in (lane..rows).step_by(STRIDE) {
                let value = _mm256_loadu_si256(extrinsic.add(at).cast());
                let magnitude = _mm256_max_epi16(value, _mm256_subs_epi16(zero, value));
                let tied = _mm256_cmpeq_epi16(magnitude, first);
                let chosen = _mm256_blendv_epi8(near, far, tied);
                let flip = _mm256_srai_epi16::<15>(_mm256_xor_si256(sign, value));
                let message = _mm256_sub_epi16(_mm256_xor_si256(chosen, flip), flip);
                let previous = _mm256_loadu_si256(messages.add(at).cast());
                _mm256_storeu_si256(
                    gathered.add(at).cast(),
                    _mm256_subs_epi16(message, previous),
                );
                _mm256_storeu_si256(messages.add(at).cast(), message);
            }
        }
        let mut lanes = [0i16; 16];
        _mm256_storeu_si256(lanes.as_mut_ptr().cast(), count);
        lanes
            .iter()
            .map(|&lane| u32::from(lane.unsigned_abs()))
            .sum()
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn vector(gathered: &mut [i16], extrinsic: &mut [i16], messages: &mut [i16]) -> u32 {
    scalar(gathered, extrinsic, messages)
}
