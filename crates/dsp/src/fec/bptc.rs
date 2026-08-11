//! The DMR block product turbo codes (ETSI TS 102 361-1 B.2): a rectangle of bits with a
//! Hamming code along every row *and* every column, interleaved across the burst so a fade
//! that destroys a run of bits leaves at most one error in each codeword.
//!
//! Two shapes are in use and both live here because they differ only in their dimensions and
//! their row code:
//!
//! * [`Bptc196`] — 196 bits either side of a data burst's sync, carrying 96 bits of signalling
//!   (a link control, a CSBK, a data header). 13 rows of Hamming(15,11,3), 15 columns of
//!   Hamming(13,9,3).
//! * [`Bptc128`] — the 128 bits a voice superframe carries its embedded link control in, four
//!   bursts at a time, carrying 77. 8 rows of Hamming(16,11,4), 16 columns of even parity.
//!
//! Decoding alternates row and column passes: a repair in one direction changes the syndromes
//! in the other, which is what lets the product code correct patterns neither code could alone.
//!
//! Both blocks also decode soft input (`decode_soft`): Chase-2 over every component codeword —
//! try sign flips of the least-reliable positions, hard-decode each trial, keep the valid
//! codeword nearest in analog distance — iterated between rows and columns like the hard
//! passes, with the winner's signs imposed on the soft rectangle so each direction learns from
//! the other. Measured over 400 AWGN-corrupted BPSK [`Bptc196`] blocks per point (unit
//! symbols, noise deviation σ), blocks lost at σ = {0.70, 0.65, 0.60, 0.55, 0.50}:
//! hard {357, 298, 228, 144, 68}, soft {116, 51, 20, 5, 1}.
//!
//! Pyndiah's extrinsic update (blend the metric gap to the best competitor into the soft value
//! instead of just flipping signs; α = ½, β = 16) was measured against this and dropped: it
//! lost only {10, 1, 0, 0, 0} blocks on the same points — a real further gain — but it also
//! converged 16–43 % of pure-noise blocks into confident codewords where sign-flip converges
//! 0.1 % and the hard decoder 0.01 %. These blocks feed signalling whose only guard beyond
//! this point is a 16-bit check; rejection is not a property this decoder may trade away.

use super::{block::ParityCode, conv::Soft};

/// Passes over the rectangle before giving up. Each pass can only reduce the number of bad
/// codewords, so a fixed point is reached quickly; the cap bounds the work when the burst is
/// noise and no fixed point exists.
const MAX_PASSES: usize = 5;

/// Longest component codeword either block uses, so the Chase search needs no allocation.
const MAX_COMPONENT_BITS: usize = 16;

/// Sign-flip positions per Chase-2 search over a Hamming component. Canonical Chase-2 flips
/// ⌊d/2⌋ = 1–2 positions, which misses exactly the double-error words the product iteration
/// feeds back; 4 flips cover every pattern of up to four low-confidence errors plus one the
/// syndrome locates elsewhere — more than a transverse pass ever leaves in one codeword — at
/// 2⁴ syndrome decodes of at most 16 bits, a cost too small to meter.
const CHASE_FLIPS: u32 = 4;

/// Hard repairs the post-Chase settle may make before the block is refused. The settle exists
/// to mop up the single-bit leftovers a bounded Chase iteration strands, not to finish a
/// decode the soft passes never converged on: every repair it is allowed multiplies the rate
/// at which pure noise is accepted, for a shrinking correction return. Measured on [`Bptc196`]
/// as (noise blocks accepted per 10⁴, blocks lost of 400 at σ = 0.70): cap 0 → (0, 181),
/// 3 → (6, 133), 4 → (12, 116), 6 → (16, 115), 8 → (49, 100), unbounded → (554, 94).
/// The knee is at 4.
const MAX_SETTLE_REPAIRS: u32 = 4;

/// Chase-2 over one component codeword held as soft values (positive = logical 1): try every
/// sign-flip pattern over the `flips` least-reliable positions, hard-decode each trial with
/// `decode`, and keep the valid codeword closest to the received word in analog distance — the
/// sum of |soft| over the positions where they disagree.
///
/// The winner is imposed on `soft` by flipping signs while keeping magnitudes, so the
/// transverse pass still sees which bits were confident. An erasure (0) gets the smallest
/// nonzero magnitude, because a soft value of 0 slices to logical 0 and would smuggle a
/// decision nobody made. Returns whether anything changed — what the caller's fixed-point
/// loop watches. A word where no trial decodes (possible for the shortened and the extended
/// Hamming members) is left untouched for the other direction to repair.
fn chase(soft: &mut [Soft], flips: u32, decode: impl Fn(&mut [bool]) -> Option<u32>) -> bool {
    let n = soft.len();
    debug_assert!(n <= MAX_COMPONENT_BITS && (flips as usize) <= n);
    let mut base = [false; MAX_COMPONENT_BITS];
    for (slot, &s) in base.iter_mut().zip(soft.iter()) {
        *slot = s > 0;
    }
    let mut order: [usize; MAX_COMPONENT_BITS] = std::array::from_fn(|i| i);
    // Ascending reliability; the index tie-break keeps the search order deterministic.
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

/// Even parity across the whole word — the (8,7) column code of [`Bptc128`]. It can detect but
/// never locate, so it corrects nothing here; under [`chase`] with one flip position the search
/// becomes Wagner decoding (flip the least reliable bit iff parity fails), which is the exact
/// maximum-likelihood decision for a single parity check.
fn even_parity_decode(word: &mut [bool]) -> Option<u32> {
    let odd = word.iter().fold(false, |acc, &b| acc ^ b);
    if odd { None } else { Some(0) }
}

/// The interleaved 196-bit block a DMR data burst carries.
pub struct Bptc196;

impl Bptc196 {
    /// Payload bits either side of the sync, before deinterleaving.
    pub const CODED_BITS: usize = 196;
    /// Signalling bits recovered from one block.
    pub const DATA_BITS: usize = 96;

    /// The interleaver: coded bit `a` of the burst is matrix bit `a·181 mod 196`
    /// (ETSI TS 102 361-1 B.2.1). 181 is coprime with 196, so this is a permutation.
    fn deinterleave(coded: &[bool; Self::CODED_BITS]) -> [bool; Self::CODED_BITS] {
        let mut matrix = [false; Self::CODED_BITS];
        for (a, slot) in matrix.iter_mut().enumerate() {
            *slot = coded[a * 181 % Self::CODED_BITS];
        }
        matrix
    }

    /// Recover the 96 signalling bits, returning them with the number of bit errors repaired,
    /// or `None` when a row or column codeword is still inconsistent after the last pass.
    #[must_use]
    pub fn decode(coded: &[bool; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        let mut matrix = Self::deinterleave(coded);
        let corrected = Self::settle(&mut matrix)?;
        Some((Self::extract(&matrix), corrected))
    }

    /// Recover the 96 signalling bits from soft values, with the second element defined as the
    /// Hamming distance between the returned codeword and the hard slice of the input
    /// (`soft > 0` reads as 1) — the count of received signs the decoder overruled, whatever
    /// their confidence. The distance is the same in wire and matrix order, the interleaver
    /// being a permutation.
    ///
    /// `None` when the passes never come within [`MAX_SETTLE_REPAIRS`] hard repairs of a
    /// consistent rectangle, and for an all-erasure input: the all-zero word is a valid
    /// codeword of any linear code, but zero evidence must not decode to *any* data.
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
        // Unlike the hard passes, every one of the 13 rows and 15 columns is searched: the
        // parity-on-parity rows and columns are XORs of information rows/columns and therefore
        // valid component codewords themselves, and their soft values carry evidence too.
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
        // The hard passes verify the fixed point is a codeword and mop up the few leftovers
        // the Chase iterations were still short of — but no more than that (MAX_SETTLE_REPAIRS).
        let mut hard: [bool; Self::CODED_BITS] = std::array::from_fn(|i| matrix[i] > 0);
        if Self::settle(&mut hard).is_none_or(|repairs| repairs > MAX_SETTLE_REPAIRS) {
            return None;
        }
        let corrected = hard.iter().zip(&received).filter(|(a, b)| a != b).count() as u32;
        Some((Self::extract(&hard), corrected))
    }

    /// Run hard passes to a fixed point, returning the bits repaired.
    fn settle(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for _ in 0..MAX_PASSES {
            let repaired = Self::pass(matrix)?;
            corrected += repaired;
            if repaired == 0 {
                return Some(corrected);
            }
        }
        // The passes never settled: the block is still being changed on the last one, so
        // nothing here has been shown to be a codeword.
        None
    }

    /// One column pass followed by one row pass, returning the bits repaired.
    fn pass(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for c in 0..15 {
            let mut column: [bool; 13] = std::array::from_fn(|r| matrix[r * 15 + c + 1]);
            corrected += ParityCode::HAMMING_13_9.decode(&mut column)?;
            for (r, bit) in column.into_iter().enumerate() {
                matrix[r * 15 + c + 1] = bit;
            }
        }
        // Only the first nine rows carry information; the last four are the column parity and
        // are checked by the column pass that just ran.
        for r in 0..9 {
            let start = r * 15 + 1;
            corrected += ParityCode::HAMMING_15_11.decode(&mut matrix[start..start + 15])?;
        }
        Some(corrected)
    }

    /// Matrix positions of the 96 information bits, in order. Row 0 gives its first three
    /// columns to the reserved R(0..2) bits the burst still transmits, so it carries eight
    /// information bits where every other row carries eleven.
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

    /// Build the 196 coded bits for `data` — the reference modulator's half of [`decode`].
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

/// The 128-bit block a DMR voice superframe carries its embedded link control in, spread over
/// the embedded signalling fields of bursts B to E.
pub struct Bptc128;

impl Bptc128 {
    pub const CODED_BITS: usize = 128;
    /// Seven rows of eleven information bits. The mode above splits them into the 72-bit link
    /// control and the five-bit checksum interleaved with it — see [`Bptc128::decode`].
    pub const DATA_BITS: usize = 77;

    /// The wire order is the transpose of the rectangle: the four bursts each carry 32 bits,
    /// read down the columns. Position 127 is its own image, as it is for every transpose of
    /// this shape.
    fn transpose(index: usize) -> usize {
        if index == 127 { 127 } else { index * 16 % 127 }
    }

    /// Recover the 77 information bits row by row (row `r`, columns 0 to 10, at output index
    /// `r · 11 + c`), with the number of bits repaired.
    ///
    /// The five-bit checksum the embedded link control carries is *inside* those bits, at
    /// column 10 of rows 2 to 6; this code has no opinion about it.
    #[must_use]
    pub fn decode(coded: &[bool; Self::CODED_BITS]) -> Option<([bool; Self::DATA_BITS], u32)> {
        let mut matrix = [false; Self::CODED_BITS];
        for (a, &bit) in coded.iter().enumerate() {
            matrix[Self::transpose(a)] = bit;
        }
        let corrected = Self::settle(&mut matrix)?;
        Some((Self::extract(&matrix), corrected))
    }

    /// Recover the 77 information bits from soft values, with the second element defined as
    /// the Hamming distance between the returned codeword and the hard slice of the input
    /// (`soft > 0` reads as 1) — the count of received signs the decoder overruled, whatever
    /// their confidence. The distance is the same in wire and matrix order, the transpose
    /// being a permutation.
    ///
    /// `None` when the passes never come within [`MAX_SETTLE_REPAIRS`] hard repairs of a
    /// consistent rectangle, and for an all-erasure input — zero evidence must not decode to
    /// *any* data.
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
        // All eight rows are searched: the parity row is the XOR of the seven information
        // rows and therefore a valid Hamming(16,11) codeword itself.
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

    /// Hard-decode the rectangle in place, returning the bits repaired.
    fn settle(matrix: &mut [bool; Self::CODED_BITS]) -> Option<u32> {
        let mut corrected = 0;
        for r in 0..7 {
            corrected += ParityCode::HAMMING_16_11.decode(&mut matrix[r * 16..r * 16 + 16])?;
        }
        // The eighth row is even parity down each column. It cannot locate an error, so a
        // failure here means a row code accepted a pattern it should not have.
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

    /// Build the 128 coded bits for `data`.
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

    /// What the product code is for: errors spread across the burst by the interleaver land in
    /// different rows and columns, and each one is a single-bit repair.
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

    /// A burst of noise is not a block. Whatever the passes converge on, an inconsistent
    /// rectangle must be refused rather than handed up as signalling.
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

    /// xorshift64* plus Box-Muller: seeded noise with no OS randomness anywhere.
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

    /// One BPSK-over-AWGN transmission of a [`Bptc196`] block: unit symbols, noise deviation
    /// `sigma`, soft values scaled so a clean symbol sits at half confidence.
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

    /// The pattern soft decoding exists for: four flips on the corners of a rectangle give
    /// every touched row and column a double error, which hard decoding cannot reconcile —
    /// but the flipped positions arrive with low confidence, so Chase finds them.
    ///
    /// Also pins the `errors_corrected` definition: the returned codeword is the transmitted
    /// one, whose Hamming distance to the hard slice of the input is exactly the four flips.
    #[test]
    fn bptc196_soft_recovers_a_rectangle_hard_decode_cannot() {
        let data: [bool; 96] = pattern(31);
        let mut coded = soften(&Bptc196::encode(&data));
        // Matrix corners (rows 2 and 5, columns 3 and 7) mapped back through the interleaver.
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
        // Matrix corners (rows 1 and 4, columns 2 and 9): each row carries a double error,
        // which the distance-4 row code detects and refuses, so hard decode dies immediately.
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

    /// `errors_corrected` counts overruled input signs, not decoder work: five scattered
    /// low-confidence flips come back as exactly five.
    #[test]
    fn bptc196_soft_reports_the_hamming_distance_to_the_input() {
        let data: [bool; 96] = pattern(41);
        let mut coded = soften(&Bptc196::encode(&data));
        for a in [3usize, 44, 91, 150, 195] {
            coded[a] = if coded[a] > 0 { -8 } else { 8 };
        }
        assert_eq!(Bptc196::decode_soft(&coded), Some((data, 5)));
    }

    /// An all-erasure block carries no evidence. The all-zero word is a valid codeword of any
    /// linear code, and returning it would be fabrication.
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

    /// Chase searches harder than syndrome decoding, so it also finds codewords in pure noise
    /// more often; that rate is a property of the decoder, and this pins it. Measured: 12 of
    /// 10⁴ noise blocks accepted (the hard decoder accepts 1); the bound leaves headroom for
    /// seed sensitivity, not for regression. Whatever slips through still faces the CRC/RS
    /// checks every user of these blocks runs next.
    #[test]
    fn bptc196_soft_acceptance_of_noise_stays_at_its_measured_rate() {
        let mut rng = TestRng(0x196_196);
        let accepted = (0..10_000)
            .filter(|_| Bptc196::decode_soft(&rng.noise_block::<196>()).is_some())
            .count();
        assert!(accepted <= 40, "accepted {accepted} of 10000 noise blocks");
    }

    /// Measured: 81 of 10⁴ — an order worse than [`Bptc196`], as expected for half the block
    /// with a locate-nothing parity column code in place of a second Hamming code.
    #[test]
    fn bptc128_soft_acceptance_of_noise_stays_at_its_measured_rate() {
        let mut rng = TestRng(0x128_128);
        let accepted = (0..10_000)
            .filter(|_| Bptc128::decode_soft(&rng.noise_block::<128>()).is_some())
            .count();
        assert!(accepted <= 160, "accepted {accepted} of 10000 noise blocks");
    }

    /// The reason decode_soft exists, measured: at σ = 0.55 hard decoding loses 144 of 400
    /// blocks, soft decoding 5. The asserts are looser than the measurement only to tolerate
    /// libm rounding differences across platforms, not decoder regression.
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
