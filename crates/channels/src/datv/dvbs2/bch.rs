const NORMAL_POLY: u32 = 0x1_002D;
const SHORT_POLY: u32 = 0x402B;

pub struct Bch {
    exp: Vec<u16>,
    log: Vec<u16>,
    order: usize,
    generator: Vec<bool>,
    correct: usize,
    message: usize,
}

fn minimal_polynomial(exp: &[u16], log: &[u16], order: usize, power: usize) -> Vec<u16> {
    let mut roots = Vec::new();
    let mut current = power % order;
    loop {
        if roots.contains(&current) {
            break;
        }
        roots.push(current);
        current = current * 2 % order;
    }
    let mut polynomial = vec![1u16];
    for root in roots {
        let value = exp[root];
        let mut next = vec![0u16; polynomial.len() + 1];
        for (index, &coefficient) in polynomial.iter().enumerate() {
            next[index + 1] ^= coefficient;
            if coefficient != 0 && value != 0 {
                next[index] ^= exp[(usize::from(log[usize::from(coefficient)])
                    + usize::from(log[usize::from(value)]))
                    % order];
            }
        }
        polynomial = next;
    }
    polynomial
}

impl Bch {
    #[must_use]
    pub fn new(short: bool, correct: usize, message: usize) -> Self {
        let (primitive, bits) = if short {
            (SHORT_POLY, 14u32)
        } else {
            (NORMAL_POLY, 16u32)
        };
        let size = 1usize << bits;
        let order = size - 1;
        let mut exp = vec![0u16; order];
        let mut log = vec![0u16; size];
        let mut value = 1u32;
        for (index, slot) in exp.iter_mut().enumerate() {
            *slot = value as u16;
            log[value as usize] = index as u16;
            value <<= 1;
            if value & size as u32 != 0 {
                value ^= primitive;
            }
        }
        let mut generator = vec![true];
        for step in 0..correct {
            let factor = minimal_polynomial(&exp, &log, order, 2 * step + 1);
            if !divides(&generator, &factor) {
                generator = multiply(&generator, &factor);
            }
        }
        Self {
            exp,
            log,
            order,
            generator,
            correct,
            message,
        }
    }

    #[must_use]
    pub const fn parity(&self) -> usize {
        self.generator.len() - 1
    }

    #[must_use]
    pub const fn message(&self) -> usize {
        self.message
    }

    fn power(&self, exponent: usize) -> u16 {
        self.exp[exponent % self.order]
    }

    fn mul(&self, a: u16, b: u16) -> u16 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[(usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)]))
            % self.order]
    }

    fn inv(&self, a: u16) -> u16 {
        self.power(self.order - usize::from(self.log[usize::from(a)]))
    }

    pub fn encode(&self, message: &[bool], out: &mut Vec<bool>) {
        let parity_len = self.parity();
        let mut remainder = vec![false; parity_len];
        for &bit in message {
            let feedback = bit ^ remainder[0];
            remainder.copy_within(1.., 0);
            remainder[parity_len - 1] = false;
            if feedback {
                for (index, slot) in remainder.iter_mut().enumerate() {
                    *slot ^= self.generator[parity_len - 1 - index];
                }
            }
        }
        out.extend_from_slice(message);
        out.extend_from_slice(&remainder);
    }

    fn syndromes(&self, word: &[bool]) -> Vec<u16> {
        let last = word.len() - 1;
        (1..=2 * self.correct)
            .map(|index| {
                word.iter()
                    .enumerate()
                    .filter(|&(_, &bit)| bit)
                    .fold(0u16, |sum, (position, _)| {
                        sum ^ self.power(index * (last - position))
                    })
            })
            .collect()
    }

    fn locator(&self, syndromes: &[u16]) -> (Vec<u16>, usize) {
        let mut locator = vec![1u16];
        let mut previous = vec![1u16];
        let mut discrepancy_at_update = 1u16;
        let mut shift = 1usize;
        let mut errors = 0usize;
        for step in 0..2 * self.correct {
            let mut discrepancy = syndromes[step];
            for index in 1..locator.len().min(step + 1) {
                discrepancy ^= self.mul(locator[index], syndromes[step - index]);
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

    pub fn decode(&self, word: &mut [bool]) -> Option<usize> {
        let syndromes = self.syndromes(word);
        if syndromes.iter().all(|&value| value == 0) {
            return Some(0);
        }
        let (locator, errors) = self.locator(&syndromes);
        if errors == 0 || errors > self.correct {
            return None;
        }
        let last = word.len() - 1;
        let mut found = 0usize;
        for (position, slot) in word.iter_mut().enumerate() {
            let inverse = self.power(self.order - (last - position) % self.order);
            let mut sum = 0u16;
            let mut term = 1u16;
            for &coefficient in &locator {
                sum ^= self.mul(coefficient, term);
                term = self.mul(term, inverse);
            }
            if sum == 0 {
                *slot ^= true;
                found += 1;
            }
        }
        if found != errors {
            return None;
        }
        self.syndromes(word)
            .iter()
            .all(|&value| value == 0)
            .then_some(errors)
    }
}

fn multiply(left: &[bool], right: &[u16]) -> Vec<bool> {
    let mut out = vec![false; left.len() + right.len() - 1];
    for (index, &a) in left.iter().enumerate() {
        if !a {
            continue;
        }
        for (offset, &b) in right.iter().enumerate() {
            if b != 0 {
                out[index + offset] ^= true;
            }
        }
    }
    out
}

fn divides(product: &[bool], factor: &[u16]) -> bool {
    let factor: Vec<bool> = factor.iter().map(|&value| value != 0).collect();
    if factor.len() > product.len() {
        return false;
    }
    let mut remainder = product.to_vec();
    for index in (factor.len() - 1..remainder.len()).rev() {
        if !remainder[index] {
            continue;
        }
        let base = index + 1 - factor.len();
        for (offset, &coefficient) in factor.iter().enumerate() {
            if coefficient {
                remainder[base + offset] ^= true;
            }
        }
    }
    remainder.iter().all(|&bit| !bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(len: usize, seed: u32) -> Vec<bool> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state & 1 == 1
            })
            .collect()
    }

    #[test]
    fn the_field_matches_the_primitive_polynomial_the_standard_names() {
        let normal = Bch::new(false, 8, 57_472);
        assert_eq!(normal.power(16), 0x2D);
        assert_eq!(normal.power(0), 1);
        let short = Bch::new(true, 12, 7_032);
        assert_eq!(short.power(14), 0x2B);
    }

    #[test]
    fn every_documented_parity_length_comes_out_of_the_generator() {
        for (short, correct, parity) in [
            (false, 12usize, 192usize),
            (false, 10, 160),
            (false, 8, 128),
            (true, 12, 168),
        ] {
            let code = Bch::new(short, correct, 1_000);
            assert_eq!(
                code.parity(),
                parity,
                "t={correct} short={short} gave {} parity bits",
                code.parity()
            );
        }
    }

    #[test]
    fn the_generator_vanishes_at_every_design_root() {
        let code = Bch::new(false, 12, 32_208);
        for root in 1..=2 * 12usize {
            let mut sum = 0u16;
            let mut term = 1u16;
            let value = code.power(root);
            for &coefficient in &code.generator {
                if coefficient {
                    sum ^= term;
                }
                term = code.mul(term, value);
            }
            assert_eq!(sum, 0, "the generator does not vanish at root {root}");
        }
    }

    #[test]
    fn a_single_error_is_located() {
        let code = Bch::new(false, 12, 32_208);
        let mut word = Vec::new();
        code.encode(&message(code.message(), 21), &mut word);
        let clean = word.clone();
        word[1_234] ^= true;
        assert_eq!(code.decode(&mut word), Some(1));
        assert_eq!(word, clean);
    }

    #[test]
    fn a_clean_codeword_reports_no_errors() {
        let code = Bch::new(false, 12, 32_208);
        let mut word = Vec::new();
        code.encode(&message(code.message(), 3), &mut word);
        assert_eq!(word.len(), 32_400);
        assert_eq!(code.decode(&mut word), Some(0));
    }

    #[test]
    fn errors_up_to_the_design_distance_are_repaired() {
        for (short, correct, message_bits) in [
            (false, 12usize, 32_208usize),
            (false, 8, 57_472),
            (true, 12, 7_032),
        ] {
            let code = Bch::new(short, correct, message_bits);
            let information = message(message_bits, 5);
            let mut word = Vec::new();
            code.encode(&information, &mut word);
            let clean = word.clone();
            for step in 0..correct {
                word[step * 97 + 3] ^= true;
            }
            assert_eq!(code.decode(&mut word), Some(correct), "t={correct}");
            assert_eq!(word, clean);
        }
    }

    #[test]
    fn one_error_beyond_the_design_distance_is_refused() {
        let code = Bch::new(false, 8, 57_472);
        let mut word = Vec::new();
        code.encode(&message(code.message(), 9), &mut word);
        for step in 0..9 {
            word[step * 601 + 11] ^= true;
        }
        assert_eq!(code.decode(&mut word), None);
    }
}
