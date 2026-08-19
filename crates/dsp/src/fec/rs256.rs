const SIZE: usize = 256;
const ORDER: usize = SIZE - 1;

pub const DVB_PRIMITIVE: u16 = 0x11D;

#[derive(Clone, Debug)]
pub struct ReedSolomon {
    exp: [u8; 2 * ORDER],
    log: [u8; SIZE],
    generator: Vec<u8>,
    parity: usize,
    first_root: u8,
}

impl ReedSolomon {
    #[must_use]
    pub fn new(primitive: u16, first_root: u8, parity: usize) -> Self {
        let mut exp = [0u8; 2 * ORDER];
        let mut log = [0u8; SIZE];
        let mut value = 1u16;
        for index in 0..ORDER {
            exp[index] = value as u8;
            exp[index + ORDER] = value as u8;
            log[value as usize] = index as u8;
            value <<= 1;
            if value & SIZE as u16 != 0 {
                value ^= primitive;
            }
        }
        let mut code = Self {
            exp,
            log,
            generator: Vec::new(),
            parity,
            first_root,
        };
        code.generator = code.build_generator();
        code
    }

    fn build_generator(&self) -> Vec<u8> {
        let mut ascending = vec![1u8];
        for index in 0..self.parity {
            let root = self.power(u32::from(self.first_root) + index as u32);
            let mut next = vec![0u8; ascending.len() + 1];
            for (position, &coefficient) in ascending.iter().enumerate() {
                next[position] ^= self.mul(coefficient, root);
                next[position + 1] ^= coefficient;
            }
            ascending = next;
        }
        ascending.reverse();
        ascending
    }

    fn power(&self, exponent: u32) -> u8 {
        self.exp[(exponent % ORDER as u32) as usize]
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])]
    }

    fn inv(&self, a: u8) -> u8 {
        self.exp[ORDER - usize::from(self.log[usize::from(a)])]
    }

    fn evaluate(&self, ascending: &[u8], at: u8) -> u8 {
        let mut sum = 0u8;
        let mut term = 1u8;
        for &coefficient in ascending {
            sum ^= self.mul(coefficient, term);
            term = self.mul(term, at);
        }
        sum
    }

    #[must_use]
    pub const fn parity(&self) -> usize {
        self.parity
    }

    #[must_use]
    pub fn correctable(&self) -> usize {
        self.parity / 2
    }

    pub fn encode(&self, data: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(data);
        out.resize(start + data.len() + self.parity, 0);
        for index in 0..data.len() {
            let coefficient = out[start + index];
            if coefficient == 0 {
                continue;
            }
            for (offset, &factor) in self.generator.iter().enumerate().skip(1) {
                out[start + index + offset] ^= self.mul(factor, coefficient);
            }
        }
        out[start..start + data.len()].copy_from_slice(data);
    }

    fn syndromes(&self, codeword: &[u8]) -> Vec<u8> {
        (0..self.parity)
            .map(|index| {
                let root = self.power(u32::from(self.first_root) + index as u32);
                codeword.iter().fold(0u8, |accumulator, &symbol| {
                    self.mul(accumulator, root) ^ symbol
                })
            })
            .collect()
    }

    fn berlekamp_massey(&self, syndromes: &[u8]) -> (Vec<u8>, usize) {
        let mut locator = vec![1u8];
        let mut previous = vec![1u8];
        let mut discrepancy_at_update = 1u8;
        let mut shift = 1usize;
        let mut errors = 0usize;
        for step in 0..self.parity {
            let mut discrepancy = syndromes[step];
            for index in 1..=errors.min(step) {
                if index < locator.len() {
                    discrepancy ^= self.mul(locator[index], syndromes[step - index]);
                }
            }
            if discrepancy == 0 {
                shift += 1;
                continue;
            }
            let saved = locator.clone();
            let scale = self.mul(discrepancy, self.inv(discrepancy_at_update));
            if locator.len() < previous.len() + shift {
                locator.resize(previous.len() + shift, 0);
            }
            for (index, &coefficient) in previous.iter().enumerate() {
                locator[index + shift] ^= self.mul(scale, coefficient);
            }
            if 2 * errors <= step {
                errors = step + 1 - errors;
                previous = saved;
                discrepancy_at_update = discrepancy;
                shift = 1;
            } else {
                shift += 1;
            }
        }
        locator.truncate(errors + 1);
        (locator, errors)
    }

    fn chien(&self, locator: &[u8], length: usize, errors: usize) -> Option<Vec<usize>> {
        let mut positions = Vec::with_capacity(errors);
        for position in 0..length {
            if self.evaluate(locator, self.power((ORDER - position % ORDER) as u32)) == 0 {
                positions.push(position);
            }
        }
        (positions.len() == errors).then_some(positions)
    }

    fn evaluator(&self, syndromes: &[u8], locator: &[u8]) -> Vec<u8> {
        let mut product = vec![0u8; self.parity];
        for (index, &coefficient) in locator.iter().enumerate() {
            for (offset, &syndrome) in syndromes.iter().enumerate() {
                if index + offset < self.parity {
                    product[index + offset] ^= self.mul(coefficient, syndrome);
                }
            }
        }
        product
    }

    fn magnitude(&self, evaluator: &[u8], derivative: &[u8], position: usize) -> Option<u8> {
        let inverse = self.power((ORDER - position % ORDER) as u32);
        let denominator = self.evaluate(derivative, inverse);
        if denominator == 0 {
            return None;
        }
        let ratio = self.mul(self.evaluate(evaluator, inverse), self.inv(denominator));
        let exponent = (ORDER as u32 * SIZE as u32
            - u32::from(self.first_root) * position as u32 % ORDER as u32)
            % ORDER as u32;
        Some(self.mul(ratio, self.power(exponent)))
    }

    pub fn decode(&self, codeword: &mut [u8]) -> Option<u32> {
        let syndromes = self.syndromes(codeword);
        if syndromes.iter().all(|&value| value == 0) {
            return Some(0);
        }
        let (locator, errors) = self.berlekamp_massey(&syndromes);
        if errors == 0 || errors > self.correctable() {
            return None;
        }
        let positions = self.chien(&locator, codeword.len(), errors)?;
        let evaluator = self.evaluator(&syndromes, &locator);
        let derivative: Vec<u8> = locator
            .iter()
            .enumerate()
            .map(|(index, &coefficient)| if index % 2 == 1 { coefficient } else { 0 })
            .collect();
        for &position in &positions {
            let magnitude = self.magnitude(&evaluator, &derivative, position)?;
            let index = codeword.len().checked_sub(1 + position)?;
            codeword[index] ^= magnitude;
        }
        self.syndromes(codeword)
            .iter()
            .all(|&value| value == 0)
            .then_some(errors as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dvb() -> ReedSolomon {
        ReedSolomon::new(DVB_PRIMITIVE, 0, 16)
    }

    fn dab_plus() -> ReedSolomon {
        ReedSolomon::new(DVB_PRIMITIVE, 0, 10)
    }

    fn payload(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    #[test]
    fn the_field_matches_the_dvb_primitive_polynomial() {
        let code = dvb();
        assert_eq!(code.power(8), 0x1D);
        assert_eq!(code.power(0), 1);
        assert_eq!(code.mul(code.power(200), code.inv(code.power(200))), 1);
    }

    #[test]
    fn the_generator_is_monic_and_vanishes_at_every_root() {
        for code in [dvb(), dab_plus(), ReedSolomon::new(DVB_PRIMITIVE, 1, 8)] {
            assert_eq!(code.generator[0], 1);
            assert_eq!(code.generator.len(), code.parity() + 1);
            let ascending: Vec<u8> = code.generator.iter().rev().copied().collect();
            for index in 0..code.parity() {
                let root = code.power(u32::from(code.first_root) + index as u32);
                assert_eq!(code.evaluate(&ascending, root), 0, "root {index}");
            }
        }
    }

    #[test]
    fn a_clean_dvb_codeword_reports_no_errors() {
        let code = dvb();
        let mut codeword = Vec::new();
        code.encode(&payload(188, 3), &mut codeword);
        assert_eq!(codeword.len(), 204);
        assert_eq!(code.decode(&mut codeword), Some(0));
    }

    #[test]
    fn eight_symbol_errors_are_repaired() {
        let code = dvb();
        let mut codeword = Vec::new();
        code.encode(&payload(188, 5), &mut codeword);
        let clean = codeword.clone();
        for (offset, position) in [0usize, 7, 40, 91, 130, 150, 187, 203]
            .into_iter()
            .enumerate()
        {
            codeword[position] ^= 0x5A ^ offset as u8;
        }
        assert_eq!(code.decode(&mut codeword), Some(8));
        assert_eq!(codeword, clean);
    }

    #[test]
    fn nine_symbol_errors_are_refused_rather_than_mangled() {
        let code = dvb();
        let mut codeword = Vec::new();
        code.encode(&payload(188, 9), &mut codeword);
        for position in [1usize, 5, 9, 44, 70, 99, 111, 160, 190] {
            codeword[position] ^= 0xC3;
        }
        assert_eq!(code.decode(&mut codeword), None);
    }

    #[test]
    fn the_shortened_dab_plus_codeword_repairs_five_errors() {
        let code = dab_plus();
        let mut codeword = Vec::new();
        code.encode(&payload(110, 17), &mut codeword);
        assert_eq!(codeword.len(), 120);
        let clean = codeword.clone();
        for position in [3usize, 30, 55, 100, 117] {
            codeword[position] ^= 0x9E;
        }
        assert_eq!(code.decode(&mut codeword), Some(5));
        assert_eq!(codeword, clean);
    }

    #[test]
    fn a_single_error_at_either_end_is_repaired() {
        let code = dab_plus();
        let mut codeword = Vec::new();
        code.encode(&payload(110, 23), &mut codeword);
        let clean = codeword.clone();
        for position in [0, clean.len() - 1] {
            let mut damaged = clean.clone();
            damaged[position] ^= 0x01;
            assert_eq!(code.decode(&mut damaged), Some(1));
            assert_eq!(damaged, clean);
        }
    }

    #[test]
    fn a_code_with_a_non_zero_first_root_still_corrects() {
        let code = ReedSolomon::new(DVB_PRIMITIVE, 1, 8);
        let mut codeword = Vec::new();
        code.encode(&payload(100, 41), &mut codeword);
        let clean = codeword.clone();
        for position in [2usize, 44, 77, 101] {
            codeword[position] ^= 0x7B;
        }
        assert_eq!(code.decode(&mut codeword), Some(4));
        assert_eq!(codeword, clean);
    }
}
