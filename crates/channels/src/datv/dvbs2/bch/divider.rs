const LIMBS: usize = 3;
pub(super) const MAX_PARITY: usize = 64 * LIMBS;

pub(super) type Remainder = [u64; LIMBS];

pub(super) struct Divider {
    parity: usize,
    mask: Remainder,
    table: Vec<Remainder>,
}

impl Divider {
    pub(super) fn new(generator: &[bool]) -> Self {
        let parity = generator.len().saturating_sub(1).min(MAX_PARITY);
        let mask = low_bits(parity);
        let mut reduced = [0u64; LIMBS];
        for (index, _) in generator[..parity]
            .iter()
            .enumerate()
            .filter(|(_, bit)| **bit)
        {
            reduced[index / 64] |= 1 << (index % 64);
        }
        let base = reduced;
        let mut powers = [[0u64; LIMBS]; 8];
        for power in &mut powers {
            *power = reduced;
            let carry = bit(&reduced, parity - 1);
            reduced = and(shl(&reduced, 1), &mask);
            if carry {
                reduced = xor(reduced, &base);
            }
        }
        let table = (0..256usize)
            .map(|byte| {
                (0..8)
                    .filter(|shift| byte >> shift & 1 == 1)
                    .fold([0; LIMBS], |sum, shift| xor(sum, &powers[shift]))
            })
            .collect();
        Self {
            parity,
            mask,
            table,
        }
    }

    pub(super) fn remainder(&self, word: &[bool]) -> Remainder {
        let (lead, rest) = word.split_at(word.len() % 8);
        let mut state = [0u64; LIMBS];
        state[0] = u64::from(pack(lead));
        for chunk in rest.as_chunks::<8>().0 {
            let top = top_byte(&state, self.parity);
            state = and(shl(&state, 8), &self.mask);
            state[0] |= u64::from(pack(chunk));
            state = xor(state, &self.table[usize::from(top)]);
        }
        state
    }
}

pub(super) fn set_bits(remainder: &Remainder) -> impl Iterator<Item = usize> + '_ {
    remainder.iter().enumerate().flat_map(|(limb, &word)| {
        let mut rest = word;
        std::iter::from_fn(move || {
            (rest != 0).then(|| {
                let at = rest.trailing_zeros() as usize;
                rest &= rest - 1;
                limb * 64 + at
            })
        })
    })
}

fn pack(bits: &[bool]) -> u8 {
    bits.iter().fold(0, |byte, &bit| byte << 1 | u8::from(bit))
}

fn bit(value: &Remainder, index: usize) -> bool {
    value[index / 64] >> (index % 64) & 1 == 1
}

fn top_byte(value: &Remainder, parity: usize) -> u8 {
    let low = parity - 8;
    let limb = low / 64;
    let offset = low % 64;
    let mut byte = value[limb] >> offset;
    if offset > 56 && limb + 1 < LIMBS {
        byte |= value[limb + 1] << (64 - offset);
    }
    byte as u8
}

fn shl(value: &Remainder, by: u32) -> Remainder {
    let mut out = [0u64; LIMBS];
    for limb in (0..LIMBS).rev() {
        out[limb] = value[limb] << by;
        if limb > 0 {
            out[limb] |= value[limb - 1] >> (64 - by);
        }
    }
    out
}

fn and(value: Remainder, mask: &Remainder) -> Remainder {
    std::array::from_fn(|limb| value[limb] & mask[limb])
}

fn xor(value: Remainder, other: &Remainder) -> Remainder {
    std::array::from_fn(|limb| value[limb] ^ other[limb])
}

fn low_bits(count: usize) -> Remainder {
    std::array::from_fn(|limb| match count.saturating_sub(limb * 64) {
        0 => 0,
        bits if bits >= 64 => u64::MAX,
        bits => (1 << bits) - 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_remainder(word: &[bool], generator: &[bool]) -> Vec<bool> {
        let parity = generator.len() - 1;
        let mut state = vec![false; parity];
        for &input in word {
            let carry = state[parity - 1];
            state.rotate_right(1);
            state[0] = input;
            if carry {
                for (slot, &tap) in state.iter_mut().zip(generator) {
                    *slot ^= tap;
                }
            }
        }
        state
    }

    #[test]
    fn the_byte_table_matches_bitwise_division_for_every_parity_width() {
        let mut seed = 0x9E37_79B9u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed & 1 == 1
        };
        for parity in [9usize, 63, 64, 65, 127, 128, 168, 180, 192] {
            let mut generator: Vec<bool> = (0..=parity).map(|_| next()).collect();
            generator[0] = true;
            generator[parity] = true;
            let divider = Divider::new(&generator);
            for len in [0usize, 5, 8, 301, 1_003] {
                let word: Vec<bool> = (0..len).map(|_| next()).collect();
                let fast = divider.remainder(&word);
                let slow = slow_remainder(&word, &generator);
                let unpacked: Vec<bool> = (0..parity).map(|index| bit(&fast, index)).collect();
                assert_eq!(unpacked, slow, "parity {parity}, length {len}");
            }
        }
    }

    #[test]
    fn set_bits_walks_every_limb() {
        let bits: Vec<usize> = set_bits(&[0b101, 0, 1 << 63]).collect();
        assert_eq!(bits, vec![0, 2, 191]);
    }
}
