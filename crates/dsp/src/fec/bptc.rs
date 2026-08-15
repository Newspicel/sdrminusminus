use super::{block::ParityCode, conv::Soft};

const MAX_PASSES: usize = 5;

const MAX_COMPONENT_BITS: usize = 16;

const CHASE_FLIPS: u32 = 4;

const MAX_SETTLE_REPAIRS: u32 = 4;

fn chase(soft: &mut [Soft], flips: u32, decode: impl Fn(&mut [bool]) -> Option<u32>) -> bool {
    let n = soft.len();
    debug_assert!(n <= MAX_COMPONENT_BITS && (flips as usize) <= n);
    let mut base = [false; MAX_COMPONENT_BITS];
    for (slot, &s) in base.iter_mut().zip(soft.iter()) {
        *slot = s > 0;
    }
    let mut order: [usize; MAX_COMPONENT_BITS] = std::array::from_fn(|i| i);
    order[..n].sort_unstable_by_key(|&i| (soft[i].unsigned_abs(), i));

    let mut best: Option<u32> = None;
    let mut winner = base;
    for pattern in 0..1u32 << flips {
        let mut cand = base;
        for (j, &i) in order.iter().enumerate().take(flips as usize) {
            if pattern >> j & 1 == 1 {
                cand[i] = !cand[i];
            }
        }
        if decode(&mut cand[..n]).is_none() {
            continue;
        }
        let cost = soft
            .iter()
            .zip(&cand)
            .filter(|&(&s, &c)| (s > 0) != c)
            .map(|(&s, _)| u32::from(s.unsigned_abs()))
            .sum();
        if best.is_none_or(|b| cost < b) {
            best = Some(cost);
            winner = cand;
        }
    }
    if best.is_none() {
        return false;
    }
    let mut changed = false;
    for (s, &w) in soft.iter_mut().zip(&winner) {
        if (*s > 0) != w || *s == 0 {
            let mag = (*s).unsigned_abs().clamp(1, i16::MAX as u16) as i16;
            *s = if w { mag } else { -mag };
            changed = true;
        }
    }
    changed
}

fn even_parity_decode(word: &mut [bool]) -> Option<u32> {
    let odd = word.iter().fold(false, |acc, &b| acc ^ b);
    if odd { None } else { Some(0) }
}

pub struct Bptc196;

impl Bptc196 {
    pub const CODED_BITS: usize = 196;
    pub const DATA_BITS: usize = 96;

    fn deinterleave(coded: &[bool; Self::CODED_BITS]) -> [bool; Self::CODED_BITS] {
        let mut matrix = [false; Self::CODED_BITS];
        for (a, slot) in matrix.iter_mut().enumerate() {
            *slot = coded[a * 181 % Self::CODED_BITS];
        }
        matrix
    }

    #[must_use]
    pub fn decode(coded: &[bool; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        let mut matrix = Self::deinterleave(coded);
        let corrected = Self::settle(&mut matrix)?;
        Some((Self::extract(&matrix), corrected))
    }

    #[must_use]
    pub fn decode_soft(coded: &[Soft; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        if coded.iter().all(|&s| s == 0) {
            return None;
        }
        let mut matrix = [0 as Soft; Self::CODED_BITS];
        for (a, slot) in matrix.iter_mut().enumerate() {
            *slot = coded[a * 181 % Self::CODED_BITS];
        }
        let received: [bool; Self::CODED_BITS] = std::array::from_fn(|i| matrix[i] > 0);
        for _ in 0..MAX_PASSES {
            let mut changed = false;
            for c in 0..15 {
                let mut column: [Soft; 13] = std::array::from_fn(|r| matrix[r * 15 + c + 1]);
                changed |= chase(&mut column, CHASE_FLIPS, |w| {
                    ParityCode::HAMMING_13_9.decode(w)
                });
                for (r, s) in column.into_iter().enumerate() {
                    matrix[r * 15 + c + 1] = s;
                }
            }
            for r in 0..13 {
                let start = r * 15 + 1;
                changed |= chase(&mut matrix[start..start + 15], CHASE_FLIPS, |w| {
                    ParityCode::HAMMING_15_11.decode(w)
                });
            }
            if !changed {
                break;
            }
        }
        let mut hard: [bool; Self::CODED_BITS] = std::array::from_fn(|i| matrix[i] > 0);
        if Self::settle(&mut hard).is_none_or(|repairs| repairs > MAX_SETTLE_REPAIRS) {
            return None;
        }
        let corrected = hard.iter().zip(&received).filter(|(a, b)| a != b).count() as u32;
        Some((Self::extract(&hard), corrected))
    }

    fn settle(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for _ in 0..MAX_PASSES {
            let repaired = Self::pass(matrix)?;
            corrected += repaired;
            if repaired == 0 {
                return Some(corrected);
            }
        }
        None
    }

    fn pass(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for c in 0..15 {
            let mut column: [bool; 13] = std::array::from_fn(|r| matrix[r * 15 + c + 1]);
            corrected += ParityCode::HAMMING_13_9.decode(&mut column)?;
            for (r, bit) in column.into_iter().enumerate() {
                matrix[r * 15 + c + 1] = bit;
            }
        }
        for r in 0..9 {
            let start = r * 15 + 1;
            corrected += ParityCode::HAMMING_15_11.decode(&mut matrix[start..start + 15])?;
        }
        Some(corrected)
    }

    fn data_positions() -> impl Iterator<Item = usize> {
        (4..=11).chain((1..9).flat_map(|row| row * 15 + 1..=row * 15 + 11))
    }

    fn extract(matrix: &[bool; Self::CODED_BITS]) -> [bool; Self::DATA_BITS] {
        let mut out = [false; Self::DATA_BITS];
        for (slot, a) in out.iter_mut().zip(Self::data_positions()) {
            *slot = matrix[a];
        }
        out
    }

    #[must_use]
    pub fn encode(data: &[bool; Self::DATA_BITS]) -> [bool; Self::CODED_BITS] {
        let mut matrix = [false; Self::CODED_BITS];
        for (a, &bit) in Self::data_positions().zip(data.iter()) {
            matrix[a] = bit;
        }
        for r in 0..9 {
            let start = r * 15 + 1;
            ParityCode::HAMMING_15_11.encode(&mut matrix[start..start + 15]);
        }
        for c in 0..15 {
            let mut column: [bool; 13] = std::array::from_fn(|r| matrix[r * 15 + c + 1]);
            ParityCode::HAMMING_13_9.encode(&mut column);
            for (r, bit) in column.into_iter().enumerate() {
                matrix[r * 15 + c + 1] = bit;
            }
        }
        let mut coded = [false; Self::CODED_BITS];
        for (a, bit) in matrix.into_iter().enumerate() {
            coded[a * 181 % Self::CODED_BITS] = bit;
        }
        coded
    }
}

pub struct Bptc128;

impl Bptc128 {
    pub const CODED_BITS: usize = 128;
    pub const DATA_BITS: usize = 77;

    fn transpose(index: usize) -> usize {
        if index == 127 { 127 } else { index * 16 % 127 }
    }

    #[must_use]
    pub fn decode(coded: &[bool; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        let mut matrix = [false; Self::CODED_BITS];
        for (a, &bit) in coded.iter().enumerate() {
            matrix[Self::transpose(a)] = bit;
        }
        let corrected = Self::settle(&mut matrix)?;
        Some((Self::extract(&matrix), corrected))
    }

    #[must_use]
    pub fn decode_soft(coded: &[Soft; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        if coded.iter().all(|&s| s == 0) {
            return None;
        }
        let mut matrix = [0 as Soft; Self::CODED_BITS];
        for (a, &s) in coded.iter().enumerate() {
            matrix[Self::transpose(a)] = s;
        }
        let received: [bool; Self::CODED_BITS] = std::array::from_fn(|i| matrix[i] > 0);
        for _ in 0..MAX_PASSES {
            let mut changed = false;
            for r in 0..8 {
                changed |= chase(&mut matrix[r * 16..r * 16 + 16], CHASE_FLIPS, |w| {
                    ParityCode::HAMMING_16_11.decode(w)
                });
            }
            for c in 0..16 {
                let mut column: [Soft; 8] = std::array::from_fn(|r| matrix[r * 16 + c]);
                changed |= chase(&mut column, 1, even_parity_decode);
                for (r, s) in column.into_iter().enumerate() {
                    matrix[r * 16 + c] = s;
                }
            }
            if !changed {
                break;
            }
        }
        let mut hard: [bool; Self::CODED_BITS] = std::array::from_fn(|i| matrix[i] > 0);
        if Self::settle(&mut hard).is_none_or(|repairs| repairs > MAX_SETTLE_REPAIRS) {
            return None;
        }
        let corrected = hard.iter().zip(&received).filter(|(a, b)| a != b).count() as u32;
        Some((Self::extract(&hard), corrected))
    }

    fn settle(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for r in 0..7 {
            corrected += ParityCode::HAMMING_16_11.decode(&mut matrix[r * 16..r * 16 + 16])?;
        }
        for c in 0..16 {
            if (0..8).fold(false, |acc, r| acc ^ matrix[r * 16 + c]) {
                return None;
            }
        }
        Some(corrected)
    }

    fn extract(matrix: &[bool; Self::CODED_BITS]) -> [bool; Self::DATA_BITS] {
        let mut out = [false; Self::DATA_BITS];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = matrix[i / 11 * 16 + i % 11];
        }
        out
    }

    #[must_use]
    pub fn encode(data: &[bool; Self::DATA_BITS]) -> [bool; Self::CODED_BITS] {
        let mut matrix = [false; Self::CODED_BITS];
        for (i, &bit) in data.iter().enumerate() {
            matrix[i / 11 * 16 + i % 11] = bit;
        }
        for r in 0..7 {
            ParityCode::HAMMING_16_11.encode(&mut matrix[r * 16..r * 16 + 16]);
        }
        for c in 0..16 {
            matrix[7 * 16 + c] = (0..7).fold(false, |acc, r| acc ^ matrix[r * 16 + c]);
        }
        let mut coded = [false; Self::CODED_BITS];
        for (a, slot) in coded.iter_mut().enumerate() {
            *slot = matrix[Self::transpose(a)];
        }
        coded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern<const N: usize>(seed: u32) -> [bool; N] {
        let mut state = seed | 1;
        std::array::from_fn(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state & 1 == 1
        })
    }

    #[test]
    fn bptc196_round_trips() {
        let data: [bool; 96] = pattern(7);
        let coded = Bptc196::encode(&data);
        assert_eq!(Bptc196::decode(&coded), Some((data, 0)));
    }

    #[test]
    fn bptc196_repairs_scattered_errors() {
        let data: [bool; 96] = pattern(11);
        let mut coded = Bptc196::encode(&data);
        for bit in [3usize, 44, 91, 150, 195] {
            coded[bit] = !coded[bit];
        }
        let (decoded, corrected) = Bptc196::decode(&coded).expect("repairable");
        assert_eq!(decoded, data);
        assert!(corrected >= 5, "corrected {corrected} of 5 planted errors");
    }

    #[test]
    fn bptc128_round_trips_and_repairs_one_bit_per_row() {
        let data: [bool; 77] = pattern(3);
        let mut coded = Bptc128::encode(&data);
        assert_eq!(Bptc128::decode(&coded), Some((data, 0)));
        coded[5] = !coded[5];
        coded[70] = !coded[70];
        let (decoded, corrected) = Bptc128::decode(&coded).expect("repairable");
        assert_eq!(decoded, data);
        assert_eq!(corrected, 2);
    }

    #[test]
    fn bptc_refuses_a_block_it_cannot_reconcile() {
        let noise: [bool; 196] = pattern(99);
        let mut heavy = Bptc196::encode(&pattern::<96>(5));
        for (i, bit) in heavy.iter_mut().enumerate() {
            if i % 3 == 0 {
                *bit = noise[i];
            }
        }
        assert!(Bptc196::decode(&heavy).is_none_or(|(_, c)| c > 0));
    }

    use crate::fec::conv::{CONFIDENT, soft};

    fn soften<const N: usize>(bits: &[bool; N]) -> [Soft; N] {
        std::array::from_fn(|i| soft(bits[i]))
    }

    struct TestRng(u64);

    impl TestRng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn uniform(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        fn gauss(&mut self) -> f64 {
            let u1 = 1.0 - self.uniform();
            let u2 = self.uniform();
            (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
        }

        fn noise_block<const N: usize>(&mut self) -> [Soft; N] {
            std::array::from_fn(|_| {
                let r = self.next();
                let mag = (r >> 8 & 63) as i16 + 1;
                if r & 1 == 1 { mag } else { -mag }
            })
        }
    }

    fn awgn_trial(seed: u32, sigma: f64) -> ([bool; 96], [Soft; 196]) {
        let data: [bool; 96] = pattern(seed);
        let coded = Bptc196::encode(&data);
        let mut rng = TestRng(u64::from(seed) * 0x9E37_79B9 + 1);
        let soft_block = std::array::from_fn(|i| {
            let x = if coded[i] { 1.0 } else { -1.0 };
            let y = x + sigma * rng.gauss();
            (y * 32.0)
                .round()
                .clamp(-f64::from(CONFIDENT), f64::from(CONFIDENT)) as Soft
        });
        (data, soft_block)
    }

    #[test]
    fn bptc196_soft_round_trips() {
        let data: [bool; 96] = pattern(21);
        let coded = soften(&Bptc196::encode(&data));
        assert_eq!(Bptc196::decode_soft(&coded), Some((data, 0)));
    }

    #[test]
    fn bptc128_soft_round_trips() {
        let data: [bool; 77] = pattern(23);
        let coded = soften(&Bptc128::encode(&data));
        assert_eq!(Bptc128::decode_soft(&coded), Some((data, 0)));
    }

    #[test]
    fn bptc196_soft_recovers_a_rectangle_hard_decode_cannot() {
        let data: [bool; 96] = pattern(31);
        let mut coded = soften(&Bptc196::encode(&data));
        for m in [34usize, 38, 79, 83] {
            let a = m * 181 % 196;
            coded[a] = if coded[a] > 0 { -8 } else { 8 };
        }
        let hard: [bool; 196] = std::array::from_fn(|i| coded[i] > 0);
        assert!(
            Bptc196::decode(&hard).is_none_or(|(d, _)| d != data),
            "the rectangle must defeat hard decoding for this test to mean anything"
        );
        assert_eq!(Bptc196::decode_soft(&coded), Some((data, 4)));
    }

    #[test]
    fn bptc128_soft_recovers_a_rectangle_hard_decode_cannot() {
        let data: [bool; 77] = pattern(37);
        let mut coded = soften(&Bptc128::encode(&data));
        for m in [18usize, 25, 66, 73] {
            let a = (0..128)
                .find(|&a| Bptc128::transpose(a) == m)
                .unwrap_or_default();
            coded[a] = if coded[a] > 0 { -8 } else { 8 };
        }
        let hard: [bool; 128] = std::array::from_fn(|i| coded[i] > 0);
        assert_eq!(Bptc128::decode(&hard), None);
        assert_eq!(Bptc128::decode_soft(&coded), Some((data, 4)));
    }

    #[test]
    fn bptc196_soft_reports_the_hamming_distance_to_the_input() {
        let data: [bool; 96] = pattern(41);
        let mut coded = soften(&Bptc196::encode(&data));
        for a in [3usize, 44, 91, 150, 195] {
            coded[a] = if coded[a] > 0 { -8 } else { 8 };
        }
        assert_eq!(Bptc196::decode_soft(&coded), Some((data, 5)));
    }

    #[test]
    fn bptc_soft_refuses_an_all_erasure_block() {
        assert_eq!(Bptc196::decode_soft(&[0; 196]), None);
        assert_eq!(Bptc128::decode_soft(&[0; 128]), None);
    }

    #[test]
    fn bptc_soft_decoding_is_deterministic() {
        let mut rng = TestRng(0xDEC0DE);
        for _ in 0..20 {
            let block196: [Soft; 196] = rng.noise_block();
            assert_eq!(
                Bptc196::decode_soft(&block196),
                Bptc196::decode_soft(&block196)
            );
            let block128: [Soft; 128] = rng.noise_block();
            assert_eq!(
                Bptc128::decode_soft(&block128),
                Bptc128::decode_soft(&block128)
            );
        }
    }

    #[test]
    fn bptc196_soft_acceptance_of_noise_stays_at_its_measured_rate() {
        let mut rng = TestRng(0x196_196);
        let accepted = (0..10_000)
            .filter(|_| Bptc196::decode_soft(&rng.noise_block::<196>()).is_some())
            .count();
        assert!(accepted <= 40, "accepted {accepted} of 10000 noise blocks");
    }

    #[test]
    fn bptc128_soft_acceptance_of_noise_stays_at_its_measured_rate() {
        let mut rng = TestRng(0x128_128);
        let accepted = (0..10_000)
            .filter(|_| Bptc128::decode_soft(&rng.noise_block::<128>()).is_some())
            .count();
        assert!(accepted <= 160, "accepted {accepted} of 10000 noise blocks");
    }

    #[test]
    fn bptc196_soft_decoding_beats_hard_under_awgn() {
        let mut hard_failures = 0u32;
        let mut soft_failures = 0u32;
        for seed in 0..400 {
            let (data, soft_block) = awgn_trial(seed, 0.55);
            let hard: [bool; 196] = std::array::from_fn(|i| soft_block[i] > 0);
            if Bptc196::decode(&hard).is_none_or(|(d, _)| d != data) {
                hard_failures += 1;
            }
            if Bptc196::decode_soft(&soft_block).is_none_or(|(d, _)| d != data) {
                soft_failures += 1;
            }
        }
        assert!(
            hard_failures >= 100,
            "only {hard_failures} hard failures: noise level no longer stresses the decoder"
        );
        assert!(
            soft_failures <= 20 && soft_failures * 4 <= hard_failures,
            "soft lost {soft_failures} blocks to hard's {hard_failures}"
        );
    }
}
