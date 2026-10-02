use super::rs256::ReedSolomon;

pub const CCSDS_BLOCK: usize = 255;
pub const CCSDS_PARITY: usize = 32;
pub const CCSDS_DATA: usize = CCSDS_BLOCK - CCSDS_PARITY;

const FIELD_POLY: u16 = 0x187;
const ORDER: usize = 255;
const ROOT_STEP: usize = 11;
const ROOT_STEP_INVERSE: usize = 116;
const FIRST_ROOT: u8 = 112;
const DUAL_BASIS_ROWS: [u8; 8] = [0x8d, 0xef, 0xec, 0x86, 0xfa, 0x99, 0xaf, 0x7b];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterleavedOutcome {
    pub corrected: u32,
    pub failed: usize,
}

#[derive(Clone, Debug)]
pub struct CcsdsReedSolomon {
    code: ReedSolomon,
    to_code: [u8; 256],
    from_code: [u8; 256],
}

struct Field {
    exp: [u8; 2 * ORDER],
    log: [u8; 256],
}

impl Field {
    fn new(poly: u16) -> Self {
        let mut exp = [0u8; 2 * ORDER];
        let mut log = [0u8; 256];
        let mut value = 1u16;
        for index in 0..ORDER {
            exp[index] = value as u8;
            exp[index + ORDER] = value as u8;
            log[usize::from(value)] = index as u8;
            value <<= 1;
            if value & 0x100 != 0 {
                value ^= poly;
            }
        }
        Self { exp, log }
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])]
    }

    fn minimal_polynomial(&self, exponent: usize) -> u16 {
        let mut product = [0u8; 9];
        product[0] = 1;
        let mut degree = 0;
        let mut conjugate = exponent % ORDER;
        loop {
            let root = self.exp[conjugate];
            for index in (0..=degree).rev() {
                product[index + 1] ^= product[index];
                product[index] = self.mul(product[index], root);
            }
            degree += 1;
            conjugate = conjugate * 2 % ORDER;
            if conjugate == exponent % ORDER || degree == 8 {
                break;
            }
        }
        product[..=degree]
            .iter()
            .enumerate()
            .fold(0u16, |poly, (power, &bit)| {
                poly | u16::from(bit & 1) << power
            })
    }
}

fn dual_to_conventional() -> [u8; 256] {
    let mut table = [0u8; 256];
    for conventional in 0..=255u8 {
        let dual = (0..8)
            .filter(|bit| conventional >> bit & 1 == 1)
            .fold(0u8, |acc, bit| acc ^ DUAL_BASIS_ROWS[7 - bit]);
        table[usize::from(dual)] = conventional;
    }
    table
}

impl CcsdsReedSolomon {
    #[must_use]
    pub fn new() -> Self {
        let conventional = Field::new(FIELD_POLY);
        let root_poly = conventional.minimal_polynomial(ROOT_STEP);
        let rooted = Field::new(root_poly);
        let dual = dual_to_conventional();
        let mut to_code = [0u8; 256];
        let mut from_code = [0u8; 256];
        for (symbol, slot) in to_code.iter_mut().enumerate() {
            let value = dual[symbol];
            *slot = if value == 0 {
                0
            } else {
                let power = usize::from(conventional.log[usize::from(value)]);
                rooted.exp[power * ROOT_STEP_INVERSE % ORDER]
            };
            from_code[usize::from(*slot)] = symbol as u8;
        }
        Self {
            code: ReedSolomon::new(root_poly, FIRST_ROOT, CCSDS_PARITY),
            to_code,
            from_code,
        }
    }

    pub fn encode(&self, data: &[u8; CCSDS_DATA], parity: &mut [u8; CCSDS_PARITY]) {
        let mut mapped = [0u8; CCSDS_DATA];
        for (slot, &symbol) in mapped.iter_mut().zip(data) {
            *slot = self.to_code[usize::from(symbol)];
        }
        let mut codeword = Vec::with_capacity(CCSDS_BLOCK);
        self.code.encode(&mapped, &mut codeword);
        for (slot, &symbol) in parity.iter_mut().zip(&codeword[CCSDS_DATA..]) {
            *slot = self.from_code[usize::from(symbol)];
        }
    }

    pub fn decode(&self, codeword: &mut [u8; CCSDS_BLOCK]) -> Option<u32> {
        for symbol in codeword.iter_mut() {
            *symbol = self.to_code[usize::from(*symbol)];
        }
        let outcome = self.code.decode(codeword);
        for symbol in codeword.iter_mut() {
            *symbol = self.from_code[usize::from(*symbol)];
        }
        outcome
    }

    pub fn encode_interleaved(&self, data: &[u8], depth: usize, out: &mut Vec<u8>) {
        let start = out.len();
        out.resize(start + depth * CCSDS_BLOCK, 0);
        for lane in 0..depth {
            let mut lane_data = [0u8; CCSDS_DATA];
            for (index, slot) in lane_data.iter_mut().enumerate() {
                *slot = data.get(index * depth + lane).copied().unwrap_or(0);
            }
            let mut parity = [0u8; CCSDS_PARITY];
            self.encode(&lane_data, &mut parity);
            for (index, &symbol) in lane_data.iter().chain(&parity).enumerate() {
                out[start + index * depth + lane] = symbol;
            }
        }
    }

    pub fn decode_interleaved(&self, block: &mut [u8], depth: usize) -> InterleavedOutcome {
        let mut outcome = InterleavedOutcome::default();
        if block.len() < depth * CCSDS_BLOCK {
            outcome.failed = depth;
            return outcome;
        }
        for lane in 0..depth {
            let mut codeword = [0u8; CCSDS_BLOCK];
            for (index, slot) in codeword.iter_mut().enumerate() {
                *slot = block[index * depth + lane];
            }
            match self.decode(&mut codeword) {
                Some(corrected) => {
                    outcome.corrected += corrected;
                    for (index, &symbol) in codeword.iter().enumerate() {
                        block[index * depth + lane] = symbol;
                    }
                }
                None => outcome.failed += 1,
            }
        }
        outcome
    }
}

impl Default for CcsdsReedSolomon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(seed: u32) -> [u8; CCSDS_DATA] {
        let mut state = seed | 1;
        std::array::from_fn(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
    }

    fn codeword(code: &CcsdsReedSolomon, data: &[u8; CCSDS_DATA]) -> [u8; CCSDS_BLOCK] {
        let mut parity = [0u8; CCSDS_PARITY];
        code.encode(data, &mut parity);
        let mut word = [0u8; CCSDS_BLOCK];
        word[..CCSDS_DATA].copy_from_slice(data);
        word[CCSDS_DATA..].copy_from_slice(&parity);
        word
    }

    #[test]
    fn the_symbol_maps_are_bijections() {
        let code = CcsdsReedSolomon::new();
        for symbol in 0..=255u8 {
            let mapped = code.to_code[usize::from(symbol)];
            assert_eq!(code.from_code[usize::from(mapped)], symbol);
        }
        assert_eq!(code.to_code[0], 0);
    }

    #[test]
    fn the_generator_is_palindromic_in_the_code_basis() {
        let code = CcsdsReedSolomon::new();
        let mut unit = [0u8; CCSDS_DATA];
        unit[CCSDS_DATA - 1] = code.from_code[1];
        let mut parity = [0u8; CCSDS_PARITY];
        code.encode(&unit, &mut parity);
        let coefficients: Vec<u8> = parity
            .iter()
            .map(|&symbol| code.to_code[usize::from(symbol)])
            .collect();
        assert_eq!(coefficients[CCSDS_PARITY - 1], 1);
        for index in 0..CCSDS_PARITY - 1 {
            assert_eq!(
                coefficients[index],
                coefficients[CCSDS_PARITY - 2 - index],
                "coefficient {index}"
            );
        }
    }

    #[test]
    fn a_clean_codeword_decodes_without_corrections() {
        let code = CcsdsReedSolomon::new();
        let mut word = codeword(&code, &message(7));
        assert_eq!(code.decode(&mut word), Some(0));
    }

    #[test]
    fn sixteen_symbol_errors_are_corrected() {
        let code = CcsdsReedSolomon::new();
        let data = message(11);
        let clean = codeword(&code, &data);
        let mut word = clean;
        for index in 0..16 {
            word[index * 15 + 3] ^= 0x5A ^ index as u8;
        }
        assert_eq!(code.decode(&mut word), Some(16));
        assert_eq!(word, clean);
    }

    #[test]
    fn too_many_errors_are_reported() {
        let code = CcsdsReedSolomon::new();
        let mut word = codeword(&code, &message(13));
        for index in 0..40 {
            word[index * 6] ^= 0xA5;
        }
        assert_eq!(code.decode(&mut word), None);
    }

    #[test]
    fn interleaving_spreads_a_burst_over_the_lanes() {
        let code = CcsdsReedSolomon::new();
        let data: Vec<u8> = (0..4 * CCSDS_DATA).map(|index| (index * 7) as u8).collect();
        let mut block = Vec::new();
        code.encode_interleaved(&data, 4, &mut block);
        assert_eq!(block.len(), 4 * CCSDS_BLOCK);
        assert_eq!(&block[..8], &data[..8]);
        let clean = block.clone();
        for byte in &mut block[100..160] {
            *byte ^= 0xFF;
        }
        let outcome = code.decode_interleaved(&mut block, 4);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.corrected, 60);
        assert_eq!(block, clean);
        for byte in &mut block[0..200] {
            *byte ^= 0x33;
        }
        assert_eq!(code.decode_interleaved(&mut block, 4).failed, 4);
    }
}
