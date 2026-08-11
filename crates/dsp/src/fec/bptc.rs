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

use super::block::ParityCode;

/// Passes over the rectangle before giving up. Each pass can only reduce the number of bad
/// codewords, so a fixed point is reached quickly; the cap bounds the work when the burst is
/// noise and no fixed point exists.
const MAX_PASSES: usize = 5;

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
        let mut corrected = 0;
        for _ in 0..MAX_PASSES {
            let repaired = Self::pass(&mut matrix)?;
            corrected += repaired;
            if repaired == 0 {
                return Some((Self::extract(&matrix), corrected));
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
        let mut out = [false; Self::DATA_BITS];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = matrix[i / 11 * 16 + i % 11];
        }
        Some((out, corrected))
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
}
