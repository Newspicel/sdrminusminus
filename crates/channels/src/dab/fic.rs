use sdrmm_dsp::{ConvCode, DAB_DISPERSAL, Prbs, Soft, ViterbiK7, crc16_msb, pack_msb};

use super::protection::Protection;

pub const GENERATORS: [u16; 4] = [0o133, 0o171, 0o145, 0o133];
pub const FIB_BYTES: usize = 32;
pub const FIB_BITS: usize = FIB_BYTES * 8;
pub const FIBS_PER_BLOCK: usize = 3;
pub const BLOCK_BITS: usize = 2_304;
const GROUP_BITS: usize = FIB_BITS * FIBS_PER_BLOCK;

#[must_use]
pub fn fib_crc_ok(fib: &[u8]) -> bool {
    fib.len() == FIB_BYTES
        && !crc16_msb(0x1021, 0xFFFF, &fib[..FIB_BYTES - 2])
            == u16::from_be_bytes([fib[FIB_BYTES - 2], fib[FIB_BYTES - 1]])
}

#[cfg(any(test, feature = "test-signals"))]
pub fn append_fib_crc(fib: &mut Vec<u8>) {
    let crc = !crc16_msb(0x1021, 0xFFFF, fib);
    fib.extend_from_slice(&crc.to_be_bytes());
}

pub struct FicDecoder {
    protection: Protection,
    viterbi: ViterbiK7,
    mother: Vec<Soft>,
    bits: Vec<bool>,
    pub blocks_ok: u32,
    pub blocks_bad: u32,
}

impl FicDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            protection: Protection::fic(),
            viterbi: ViterbiK7::new(ConvCode::new(&GENERATORS)),
            mother: Vec::new(),
            bits: Vec::new(),
            blocks_ok: 0,
            blocks_bad: 0,
        }
    }

    pub fn reset(&mut self) {
        self.blocks_ok = 0;
        self.blocks_bad = 0;
    }

    #[must_use]
    pub fn quality(&self) -> f32 {
        let seen = self.blocks_ok + self.blocks_bad;
        if seen == 0 {
            0.0
        } else {
            self.blocks_ok as f32 / seen as f32
        }
    }

    pub fn block(&mut self, received: &[Soft], fibs: &mut Vec<[u8; FIB_BYTES]>) {
        if received.len() != BLOCK_BITS {
            return;
        }
        self.mother.clear();
        self.protection.depuncture(received, &mut self.mother);
        self.bits.clear();
        self.viterbi.decode_tailed(&self.mother, &mut self.bits);
        self.bits.truncate(GROUP_BITS);
        let mut prbs = Prbs::new(DAB_DISPERSAL);
        prbs.apply_bits(&mut self.bits);
        for chunk in self.bits.as_chunks::<FIB_BITS>().0 {
            let bytes = pack_msb(chunk);
            let mut fib = [0u8; FIB_BYTES];
            fib.copy_from_slice(&bytes);
            if fib_crc_ok(&fib) {
                self.blocks_ok += 1;
                fibs.push(fib);
            } else {
                self.blocks_bad += 1;
            }
        }
    }
}

impl Default for FicDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub struct FicEncoder {
    protection: Protection,
    code: ConvCode,
    bits: Vec<bool>,
    coded: Vec<bool>,
}

#[cfg(any(test, feature = "test-signals"))]
impl FicEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            protection: Protection::fic(),
            code: ConvCode::new(&GENERATORS),
            bits: Vec::new(),
            coded: Vec::new(),
        }
    }

    pub fn block(&mut self, fibs: &[[u8; FIB_BYTES]; FIBS_PER_BLOCK], out: &mut Vec<bool>) {
        self.bits.clear();
        for fib in fibs {
            for &byte in fib {
                for shift in (0..8).rev() {
                    self.bits.push(byte >> shift & 1 == 1);
                }
            }
        }
        Prbs::new(DAB_DISPERSAL).apply_bits(&mut self.bits);
        self.bits.extend([false; 6]);
        self.coded.clear();
        self.code.encode(&self.bits, &mut self.coded);
        self.protection.puncture(&self.coded, out);
    }
}

#[cfg(any(test, feature = "test-signals"))]
impl Default for FicEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn softs(bits: &[bool]) -> Vec<Soft> {
        bits.iter().copied().map(sdrmm_dsp::soft).collect()
    }

    fn fib(seed: u8) -> [u8; FIB_BYTES] {
        let mut body: Vec<u8> = (0..30u8)
            .map(|index| index.wrapping_mul(7).wrapping_add(seed))
            .collect();
        append_fib_crc(&mut body);
        let mut fib = [0u8; FIB_BYTES];
        fib.copy_from_slice(&body);
        fib
    }

    #[test]
    fn a_fib_carries_a_crc_that_checks_out() {
        let fib = fib(3);
        assert!(fib_crc_ok(&fib));
        let mut damaged = fib;
        damaged[5] ^= 0x01;
        assert!(!fib_crc_ok(&damaged));
    }

    #[test]
    fn the_fib_crc_is_the_complemented_ccitt_false_register() {
        assert_eq!(crc16_msb(0x1021, 0xFFFF, b"123456789"), 0x29B1);
        let mut body = b"123456789".to_vec();
        append_fib_crc(&mut body);
        assert_eq!(u16::from_be_bytes([body[9], body[10]]), !0x29B1);
    }

    #[test]
    fn a_fic_block_round_trips_three_fibs() {
        let fibs = [fib(1), fib(2), fib(3)];
        let mut sent = Vec::new();
        FicEncoder::new().block(&fibs, &mut sent);
        assert_eq!(sent.len(), BLOCK_BITS);
        let mut decoder = FicDecoder::new();
        let mut out = Vec::new();
        decoder.block(&softs(&sent), &mut out);
        assert_eq!(out, fibs);
        assert_eq!(decoder.blocks_ok, 3);
        assert_eq!(decoder.blocks_bad, 0);
    }

    #[test]
    fn scattered_channel_errors_are_repaired_by_the_convolutional_code() {
        let fibs = [fib(9), fib(11), fib(13)];
        let mut sent = Vec::new();
        FicEncoder::new().block(&fibs, &mut sent);
        let mut received = softs(&sent);
        for position in (0..received.len()).step_by(23) {
            received[position] = -received[position];
        }
        let mut decoder = FicDecoder::new();
        let mut out = Vec::new();
        decoder.block(&received, &mut out);
        assert_eq!(out, fibs);
    }

    #[test]
    fn a_block_of_noise_yields_no_valid_fib() {
        let mut state = 0x9e37_79b9u32;
        let received: Vec<Soft> = (0..BLOCK_BITS)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state % 129) as Soft - 64
            })
            .collect();
        let mut decoder = FicDecoder::new();
        let mut out = Vec::new();
        decoder.block(&received, &mut out);
        assert!(out.is_empty());
        assert_eq!(decoder.blocks_bad, 3);
    }
}
