use sdrmm_dsp::{ConvCode, ERASURE, Soft, ViterbiK7};

pub const MOTHER: [u16; 6] = [0o133, 0o171, 0o145, 0o133, 0o171, 0o145];
const TAIL_STEPS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub numerator: usize,
    pub denominator: usize,
    columns: &'static [u8],
}

pub const R1_6: Rate = rate(1, 6, &[0b11_1111]);
pub const R1_4: Rate = rate(1, 4, &[0b1111]);
#[cfg(test)]
pub const R3_10: Rate = rate(3, 10, &[0b1111, 0b111, 0b111]);
pub const R1_3: Rate = rate(1, 3, &[0b111]);
#[cfg(test)]
pub const R4_11: Rate = rate(4, 11, &[0b111, 0b111, 0b111, 0b11]);
pub const R2_5: Rate = rate(2, 5, &[0b111, 0b11]);
pub const R1_2: Rate = rate(1, 2, &[0b11]);
pub const R4_7: Rate = rate(4, 7, &[0b11, 0b101, 0b11, 0b1]);
pub const R3_5: Rate = rate(3, 5, &[0b11, 0b1, 0b11]);
pub const R2_3: Rate = rate(2, 3, &[0b11, 0b1]);
#[cfg(test)]
pub const R8_11: Rate = rate(8, 11, &[0b11, 0b1, 0b1, 0b11, 0b1, 0b1, 0b11, 0b1]);
pub const R3_4: Rate = rate(3, 4, &[0b11, 0b1, 0b1]);
pub const R4_5: Rate = rate(4, 5, &[0b11, 0b1, 0b1, 0b1]);
pub const R7_8: Rate = rate(7, 8, &[0b11, 0b1, 0b1, 0b1, 0b1, 0b1, 0b1]);
pub const R8_9: Rate = rate(8, 9, &[0b11, 0b1, 0b1, 0b1, 0b1, 0b1, 0b1, 0b1]);

const fn rate(numerator: usize, denominator: usize, columns: &'static [u8]) -> Rate {
    Rate {
        numerator,
        denominator,
        columns,
    }
}

const TAIL: [[u8; 6]; 12] = [
    [0b11, 0b11, 0b11, 0b11, 0b11, 0b11],
    [0b111, 0b11, 0b11, 0b11, 0b11, 0b11],
    [0b111, 0b11, 0b11, 0b111, 0b11, 0b11],
    [0b111, 0b111, 0b11, 0b111, 0b11, 0b11],
    [0b111, 0b111, 0b11, 0b111, 0b111, 0b11],
    [0b111, 0b111, 0b111, 0b111, 0b111, 0b11],
    [0b111, 0b111, 0b111, 0b111, 0b111, 0b111],
    [0b1111, 0b111, 0b111, 0b111, 0b111, 0b111],
    [0b1111, 0b111, 0b111, 0b1111, 0b111, 0b111],
    [0b1111, 0b1111, 0b111, 0b1111, 0b111, 0b111],
    [0b1111, 0b1111, 0b111, 0b1111, 0b111, 0b1111],
    [0b1111, 0b1111, 0b1111, 0b1111, 0b111, 0b1111],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qam {
    Q4,
    Q16,
    Q64,
}

impl Qam {
    #[must_use]
    pub const fn levels(self) -> usize {
        match self {
            Self::Q4 => 1,
            Self::Q16 => 2,
            Self::Q64 => 3,
        }
    }

    #[must_use]
    pub fn scale(self) -> f32 {
        match self {
            Self::Q4 => 2f32.sqrt().recip(),
            Self::Q16 => 10f32.sqrt().recip(),
            Self::Q64 => 42f32.sqrt().recip(),
        }
    }

    #[must_use]
    pub const fn interleaver(self, level: usize) -> Option<usize> {
        match (self, level) {
            (Self::Q64, 1) | (Self::Q16, 0) => Some(13),
            (Self::Q64, 2) | (Self::Q16, 1) | (Self::Q4, 0) => Some(21),
            _ => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Q4 => "4-QAM",
            Self::Q16 => "16-QAM",
            Self::Q64 => "64-QAM",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    Table(usize),
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub higher: Option<Rate>,
    pub lower: Rate,
    pub higher_bits: usize,
    pub lower_bits: usize,
    pub tail: Tail,
}

impl Level {
    #[must_use]
    pub const fn bits(&self) -> usize {
        self.higher_bits + self.lower_bits
    }

    fn mask(&self, step: usize) -> u8 {
        if step < self.higher_bits {
            let rate = self.higher.unwrap_or(self.lower);
            return rate.columns[step % rate.columns.len()];
        }
        let lower = step - self.higher_bits;
        match self.tail {
            Tail::Table(index) if lower >= self.lower_bits => TAIL[index][lower - self.lower_bits],
            _ => self.lower.columns[lower % self.lower.columns.len()],
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn coded_bits(&self) -> usize {
        (0..self.bits() + TAIL_STEPS)
            .map(|step| self.mask(step).count_ones() as usize)
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub qam: Qam,
    pub higher_cells: usize,
    pub lower_cells: usize,
    pub levels: Vec<Level>,
}

impl Plan {
    #[must_use]
    pub fn bits(&self) -> usize {
        self.levels.iter().map(Level::bits).sum()
    }

    #[must_use]
    pub fn higher_bits(&self) -> usize {
        self.levels.iter().map(|level| level.higher_bits).sum()
    }

    #[must_use]
    pub const fn cells(&self) -> usize {
        self.higher_cells + self.lower_cells
    }

    #[must_use]
    pub fn eep(qam: Qam, cells: usize, rates: &[Rate]) -> Option<Self> {
        Self::protected(qam, 0, cells, None, rates)
    }

    #[must_use]
    pub fn protected(
        qam: Qam,
        higher_cells: usize,
        lower_cells: usize,
        higher: Option<&[Rate]>,
        lower: &[Rate],
    ) -> Option<Self> {
        if lower.len() != qam.levels()
            || higher.is_some_and(|rates| rates.len() != qam.levels())
            || lower_cells < 20
        {
            return None;
        }
        let levels = (0..qam.levels())
            .map(|p| {
                let rate = lower[p];
                let span = (2 * lower_cells - 12) / rate.denominator;
                Level {
                    higher: higher.map(|rates| rates[p]),
                    lower: rate,
                    higher_bits: higher.map_or(0, |rates| {
                        2 * higher_cells * rates[p].numerator / rates[p].denominator
                    }),
                    lower_bits: rate.numerator * span,
                    tail: Tail::Table(2 * lower_cells - 12 - rate.denominator * span),
                }
            })
            .collect();
        Some(Self {
            qam,
            higher_cells,
            lower_cells,
            levels,
        })
    }

    #[must_use]
    pub fn fac(qam: Qam, cells: usize, rate: Rate, bits: usize) -> Self {
        Self {
            qam,
            higher_cells: 0,
            lower_cells: cells,
            levels: vec![Level {
                higher: None,
                lower: rate,
                higher_bits: 0,
                lower_bits: bits,
                tail: Tail::Continue,
            }],
        }
    }

    fn level_offsets(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        let higher = self.higher_bits();
        let mut a = 0;
        let mut b = higher;
        self.levels.iter().map(move |level| {
            let offsets = (a, b, level.higher_bits);
            a += level.higher_bits;
            b += level.lower_bits;
            offsets
        })
    }

    pub fn split(&self, bits: &[bool], level: usize, out: &mut Vec<bool>) {
        out.clear();
        if let Some((a, b, higher)) = self.level_offsets().nth(level) {
            out.extend_from_slice(&bits[a..a + higher]);
            out.extend_from_slice(&bits[b..b + self.levels[level].lower_bits]);
        }
    }

    pub fn merge(&self, level: usize, decoded: &[bool], out: &mut [bool]) {
        if let Some((a, b, higher)) = self.level_offsets().nth(level) {
            out[a..a + higher].copy_from_slice(&decoded[..higher]);
            let lower = self.levels[level].lower_bits;
            out[b..b + lower].copy_from_slice(&decoded[higher..higher + lower]);
        }
    }
}

#[must_use]
pub fn rates_for(qam: Qam, plus: bool, protection: u8) -> Option<[Rate; 3]> {
    let none = R1_2;
    Some(match (qam, plus, protection) {
        (Qam::Q4, true, 0) => [R1_4, none, none],
        (Qam::Q4, true, 1) => [R1_3, none, none],
        (Qam::Q4, true, 2) => [R2_5, none, none],
        (Qam::Q4, true, 3) => [R1_2, none, none],
        (Qam::Q16, false, 0) | (Qam::Q16, true, 2) => [R1_3, R2_3, none],
        (Qam::Q16, false, 1) | (Qam::Q16, true, 3) => [R1_2, R3_4, none],
        (Qam::Q16, true, 0) => [R1_6, R1_2, none],
        (Qam::Q16, true, 1) => [R1_4, R4_7, none],
        (Qam::Q64, false, 0) => [R1_4, R1_2, R3_4],
        (Qam::Q64, false, 1) => [R1_3, R2_3, R4_5],
        (Qam::Q64, false, 2) => [R1_2, R3_4, R7_8],
        (Qam::Q64, false, 3) => [R2_3, R4_5, R8_9],
        _ => return None,
    })
}

#[must_use]
pub fn rate_lcm(rates: &[Rate]) -> usize {
    rates.iter().fold(1, |acc, rate| lcm(acc, rate.denominator))
}

const fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

const fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
}

#[must_use]
pub fn higher_cells(bytes: usize, rates: &[Rate]) -> usize {
    if bytes == 0 {
        return 0;
    }
    let lcm = rate_lcm(rates);
    let rate_sum: usize = rates
        .iter()
        .map(|rate| rate.numerator * lcm / rate.denominator)
        .sum();
    let numerator = 8 * bytes * lcm;
    let denominator = 2 * lcm * rate_sum;
    numerator.div_ceil(denominator) * lcm
}

pub fn permutation(inputs: usize, t: usize, out: &mut Vec<usize>) {
    out.clear();
    if inputs == 0 {
        return;
    }
    let s = inputs.next_power_of_two();
    let q = s / 4 - 1;
    let mut value = 0;
    out.push(0);
    for _ in 1..inputs {
        value = (t * value + q) % s;
        while value >= inputs {
            value = (t * value + q) % s;
        }
        out.push(value);
    }
}

pub fn encode_level(level: &Level, bits: &[bool], code: &ConvCode, out: &mut Vec<bool>) {
    let mut coded = Vec::with_capacity(6 * (bits.len() + TAIL_STEPS));
    let mut padded = bits.to_vec();
    padded.extend_from_slice(&[false; TAIL_STEPS]);
    code.encode(&padded, &mut coded);
    out.clear();
    for (step, outputs) in coded.as_chunks::<6>().0.iter().enumerate() {
        let mask = level.mask(step);
        for (index, &bit) in outputs.iter().enumerate() {
            if mask >> index & 1 == 1 {
                out.push(bit);
            }
        }
    }
}

pub struct LevelDecoder {
    viterbi: ViterbiK7,
    mother: Vec<Soft>,
    decoded: Vec<bool>,
}

impl Default for LevelDecoder {
    fn default() -> Self {
        Self {
            viterbi: ViterbiK7::new(ConvCode::new(&MOTHER)),
            mother: Vec::new(),
            decoded: Vec::new(),
        }
    }
}

impl LevelDecoder {
    pub fn decode(&mut self, level: &Level, soft: &[Soft], out: &mut Vec<bool>) -> Option<i32> {
        let steps = level.bits() + TAIL_STEPS;
        self.mother.clear();
        let mut source = soft.iter();
        for step in 0..steps {
            let mask = level.mask(step);
            for index in 0..6 {
                let value = if mask >> index & 1 == 1 {
                    *source.next()?
                } else {
                    ERASURE
                };
                self.mother.push(value);
            }
        }
        self.decoded.clear();
        let metric = self.viterbi.decode_tailed(&self.mother, &mut self.decoded);
        out.clear();
        out.extend_from_slice(&self.decoded[..level.bits()]);
        Some(metric)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puncturing_tables_keep_their_rates() {
        for rate in [
            R1_6, R1_4, R3_10, R1_3, R4_11, R2_5, R1_2, R4_7, R3_5, R2_3, R8_11, R3_4, R4_5, R7_8,
            R8_9,
        ] {
            assert_eq!(rate.columns.len(), rate.numerator);
            let kept: u32 = rate.columns.iter().map(|mask| mask.count_ones()).sum();
            assert_eq!(kept as usize, rate.denominator);
        }
        for (index, tail) in TAIL.iter().enumerate() {
            let kept: u32 = tail.iter().map(|mask| mask.count_ones()).sum();
            assert_eq!(kept as usize, 12 + index);
        }
    }

    #[test]
    fn fac_codewords_fill_their_cells() {
        assert_eq!(Plan::fac(Qam::Q4, 65, R3_5, 72).levels[0].coded_bits(), 130);
        assert_eq!(
            Plan::fac(Qam::Q4, 244, R1_4, 116).levels[0].coded_bits(),
            488
        );
    }

    #[test]
    fn protected_levels_fill_their_cells_exactly() {
        for qam in [Qam::Q16, Qam::Q64] {
            for protection in 0..4 {
                let Some(rates) = rates_for(qam, false, protection) else {
                    continue;
                };
                let rates = &rates[..qam.levels()];
                let higher = higher_cells(97, rates);
                let plan = Plan::protected(qam, higher, 2959 - higher, Some(rates), rates)
                    .expect("a valid plan");
                for level in &plan.levels {
                    assert_eq!(level.coded_bits(), 2 * 2959, "{qam:?} {protection}");
                }
                assert!(plan.higher_bits() >= 8 * 97);
            }
        }
    }

    #[test]
    fn sdc_capacity_matches_the_data_field_table() {
        let plan = Plan::eep(Qam::Q4, 405, &[R1_2]).expect("SDC plan");
        assert_eq!((plan.bits() - 20) / 8, 47);
        let plan = Plan::eep(Qam::Q16, 405, &[R1_3, R2_3]).expect("SDC plan");
        assert_eq!((plan.bits() - 20) / 8, 97);
        let plan = Plan::eep(Qam::Q4, 936, &[R1_4]).expect("SDC plan");
        assert_eq!((plan.bits() - 20) / 8, 55);
    }

    #[test]
    fn the_permutation_is_a_bijection() {
        let mut order = Vec::new();
        for (inputs, t) in [(130, 21), (2 * 2959, 13), (7460, 5), (17, 21)] {
            permutation(inputs, t, &mut order);
            let mut seen = vec![false; inputs];
            for &index in &order {
                assert!(!seen[index]);
                seen[index] = true;
            }
        }
    }

    #[test]
    fn a_level_survives_encoding_and_decoding() {
        let plan = Plan::eep(Qam::Q4, 405, &[R1_2]).expect("SDC plan");
        let level = plan.levels[0];
        let bits: Vec<bool> = (0..level.bits()).map(|i| (i * 7 + i / 3) % 5 < 2).collect();
        let mut coded = Vec::new();
        encode_level(&level, &bits, &ConvCode::new(&MOTHER), &mut coded);
        assert_eq!(coded.len(), 810);
        let soft: Vec<Soft> = coded
            .iter()
            .map(|&bit| if bit { 40 } else { -40 })
            .collect();
        let mut decoded = Vec::new();
        LevelDecoder::default()
            .decode(&level, &soft, &mut decoded)
            .expect("enough soft bits");
        assert_eq!(decoded, bits);
    }
}
