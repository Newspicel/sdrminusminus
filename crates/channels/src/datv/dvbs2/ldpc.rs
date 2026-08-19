use super::tables;

pub const GROUP: usize = 360;
pub const NORMAL: usize = 64_800;
pub const MEDIUM: usize = 32_400;
pub const SHORT: usize = 16_200;
const KNOWN: f32 = 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Frame {
    Short,
    Medium,
    #[default]
    Normal,
}

impl Frame {
    #[must_use]
    pub const fn of(short: bool) -> Self {
        if short { Self::Short } else { Self::Normal }
    }

    #[must_use]
    pub const fn length(self) -> usize {
        match self {
            Self::Short => SHORT,
            Self::Medium => MEDIUM,
            Self::Normal => NORMAL,
        }
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub const fn correct_bits(self) -> usize {
        match self {
            Self::Short => 14,
            Self::Medium => 15,
            Self::Normal => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    pub shorten: usize,
    pub period: usize,
    pub punctured: usize,
}

impl Shape {
    #[must_use]
    pub const fn is_punctured(&self, parity: usize) -> bool {
        self.period > 0
            && parity.is_multiple_of(self.period)
            && parity / self.period < self.punctured
    }
}
const NORMALIZE: f32 = 0.75;
const MAX_ITERATIONS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rate {
    R1_5,
    R2_9,
    R11_45,
    R1_4,
    R4_15,
    R1_3,
    R2_5,
    R1_2,
    R3_5,
    R2_3,
    R3_4,
    R4_5,
    R5_6,
    R8_9,
    R9_10,
}

impl Rate {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::R1_5 => "1/5",
            Self::R2_9 => "2/9",
            Self::R11_45 => "11/45",
            Self::R1_4 => "1/4",
            Self::R4_15 => "4/15",
            Self::R1_3 => "1/3",
            Self::R2_5 => "2/5",
            Self::R1_2 => "1/2",
            Self::R3_5 => "3/5",
            Self::R2_3 => "2/3",
            Self::R3_4 => "3/4",
            Self::R4_5 => "4/5",
            Self::R5_6 => "5/6",
            Self::R8_9 => "8/9",
            Self::R9_10 => "9/10",
        }
    }

    #[must_use]
    pub fn information(self, frame: Frame) -> usize {
        self.addresses(frame)
            .map_or(0, |addresses| addresses.len() * GROUP)
    }

    #[must_use]
    pub fn addresses(self, frame: Frame) -> Option<&'static [&'static [u16]]> {
        Some(match frame {
            Frame::Short => match self {
                Self::R11_45 => &tables::short::R11_45,
                Self::R1_4 => &tables::short::R1_4,
                Self::R4_15 => &tables::short::R4_15,
                Self::R1_3 => &tables::short::R1_3,
                Self::R2_5 => &tables::short::R2_5,
                Self::R1_2 => &tables::short::R1_2,
                Self::R3_5 => &tables::short::R3_5,
                Self::R2_3 => &tables::short::R2_3,
                Self::R3_4 => &tables::short::R3_4,
                Self::R4_5 => &tables::short::R4_5,
                Self::R5_6 => &tables::short::R5_6,
                Self::R8_9 => &tables::short::R8_9,
                Self::R1_5 | Self::R2_9 | Self::R9_10 => return None,
            },
            Frame::Medium => match self {
                Self::R1_5 => &tables::medium::R1_5,
                Self::R11_45 => &tables::medium::R11_45,
                Self::R1_3 => &tables::medium::R1_3,
                _ => return None,
            },
            Frame::Normal => match self {
                Self::R2_9 => &tables::normal::R2_9,
                Self::R1_4 => &tables::normal::R1_4,
                Self::R1_3 => &tables::normal::R1_3,
                Self::R2_5 => &tables::normal::R2_5,
                Self::R1_2 => &tables::normal::R1_2,
                Self::R3_5 => &tables::normal::R3_5,
                Self::R2_3 => &tables::normal::R2_3,
                Self::R3_4 => &tables::normal::R3_4,
                Self::R4_5 => &tables::normal::R4_5,
                Self::R5_6 => &tables::normal::R5_6,
                Self::R8_9 => &tables::normal::R8_9,
                Self::R9_10 => &tables::normal::R9_10,
                Self::R1_5 | Self::R11_45 | Self::R4_15 => return None,
            },
        })
    }
}

struct Csr {
    offsets: Vec<u32>,
    values: Vec<u32>,
}

impl Csr {
    fn build(count: usize, edges: &[(u32, u32)]) -> Self {
        let mut offsets = vec![0u32; count + 1];
        for &(key, _) in edges {
            offsets[key as usize + 1] += 1;
        }
        for index in 0..count {
            offsets[index + 1] += offsets[index];
        }
        let mut cursor = offsets.clone();
        let mut values = vec![0u32; edges.len()];
        for &(key, value) in edges {
            let slot = &mut cursor[key as usize];
            values[*slot as usize] = value;
            *slot += 1;
        }
        Self { offsets, values }
    }

    fn row(&self, index: usize) -> &[u32] {
        let start = self.offsets[index] as usize;
        let end = self.offsets[index + 1] as usize;
        &self.values[start..end]
    }
}

pub struct Ldpc {
    length: usize,
    information: usize,
    checks: Csr,
    variables: Csr,
    check_to_variable: Vec<f32>,
    variable_to_check: Vec<f32>,
    totals: Vec<f32>,
    hard: Vec<bool>,
}

impl Ldpc {
    #[must_use]
    pub fn new(rate: Rate, frame: Frame) -> Option<Self> {
        let length = frame.length();
        let addresses = rate.addresses(frame)?;
        let information = addresses.len() * GROUP;
        let parity = length - information;
        let step = parity / GROUP;
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for bit in 0..information {
            for &address in addresses[bit / GROUP] {
                let check = (usize::from(address) + (bit % GROUP) * step) % parity;
                edges.push((check as u32, bit as u32));
            }
        }
        for check in 0..parity {
            edges.push((check as u32, (information + check) as u32));
            if check > 0 {
                edges.push((check as u32, (information + check - 1) as u32));
            }
        }
        edges.sort_unstable();
        let checks = Csr::build(parity, &edges);
        let mirrored: Vec<(u32, u32)> = (0..parity)
            .flat_map(|check| {
                let start = checks.offsets[check] as usize;
                checks
                    .row(check)
                    .iter()
                    .enumerate()
                    .map(move |(offset, &variable)| (variable, (start + offset) as u32))
            })
            .collect();
        let variables = Csr::build(length, &mirrored);
        let count = checks.values.len();
        Some(Self {
            length,
            information,
            checks,
            variables,
            check_to_variable: vec![0.0; count],
            variable_to_check: vec![0.0; count],
            totals: vec![0.0; length],
            hard: vec![false; length],
        })
    }

    #[must_use]
    pub fn parity(&self) -> usize {
        self.length - self.information
    }

    #[must_use]
    pub const fn message(&self, shape: Shape) -> usize {
        self.information - shape.shorten
    }

    #[must_use]
    pub const fn transmitted(&self, shape: Shape) -> usize {
        self.length - shape.shorten - shape.punctured
    }

    #[cfg(any(test, feature = "test-signals"))]
    fn parity_of(&self, full: &[bool]) -> Vec<bool> {
        let parity_len = self.parity();
        let mut parity = vec![false; parity_len];
        for (check, slot) in parity.iter_mut().enumerate() {
            for &variable in self.checks.row(check) {
                if (variable as usize) < self.information && full[variable as usize] {
                    *slot ^= true;
                }
            }
        }
        for index in 1..parity_len {
            let previous = parity[index - 1];
            parity[index] ^= previous;
        }
        parity
    }

    #[cfg(any(test, feature = "test-signals"))]
    pub fn encode(&self, information: &[bool], out: &mut Vec<bool>) {
        out.extend_from_slice(information);
        out.extend_from_slice(&self.parity_of(information));
    }

    #[cfg(any(test, feature = "test-signals"))]
    pub fn encode_shaped(&self, information: &[bool], shape: Shape, out: &mut Vec<bool>) {
        let mut full = vec![false; shape.shorten];
        full.extend_from_slice(information);
        full.resize(self.information, false);
        let parity = self.parity_of(&full);
        out.extend_from_slice(information);
        for (index, &bit) in parity.iter().enumerate() {
            if !shape.is_punctured(index) {
                out.push(bit);
            }
        }
    }

    pub fn expand(&self, llrs: &[f32], shape: Shape, out: &mut Vec<f32>) {
        out.clear();
        out.resize(shape.shorten, KNOWN);
        let split = self.message(shape);
        out.extend_from_slice(&llrs[..split.min(llrs.len())]);
        let mut cursor = split;
        for index in 0..self.parity() {
            if shape.is_punctured(index) {
                out.push(0.0);
            } else {
                out.push(llrs.get(cursor).copied().unwrap_or(0.0));
                cursor += 1;
            }
        }
    }

    fn satisfied(&self) -> bool {
        (0..self.checks.offsets.len() - 1).all(|check| {
            !self
                .checks
                .row(check)
                .iter()
                .fold(false, |parity, &variable| {
                    parity ^ self.hard[variable as usize]
                })
        })
    }

    pub fn decode(&mut self, llrs: &[f32], out: &mut Vec<bool>) -> Option<usize> {
        if llrs.len() != self.length {
            return None;
        }
        self.check_to_variable.fill(0.0);
        for iteration in 0..=MAX_ITERATIONS {
            self.update_variables(llrs);
            for (index, total) in self.totals.iter().enumerate() {
                self.hard[index] = *total < 0.0;
            }
            if self.satisfied() {
                out.extend_from_slice(&self.hard[..self.information]);
                return Some(iteration);
            }
            if iteration == MAX_ITERATIONS {
                break;
            }
            self.update_checks();
        }
        None
    }

    fn update_variables(&mut self, llrs: &[f32]) {
        for (variable, &llr) in llrs.iter().enumerate() {
            let mut total = llr;
            for &edge in self.variables.row(variable) {
                total += self.check_to_variable[edge as usize];
            }
            self.totals[variable] = total;
            for &edge in self.variables.row(variable) {
                self.variable_to_check[edge as usize] =
                    total - self.check_to_variable[edge as usize];
            }
        }
    }

    fn update_checks(&mut self) {
        for check in 0..self.checks.offsets.len() - 1 {
            let start = self.checks.offsets[check] as usize;
            let end = self.checks.offsets[check + 1] as usize;
            let mut sign = 1.0f32;
            let mut smallest = f32::INFINITY;
            let mut second = f32::INFINITY;
            for edge in start..end {
                let value = self.variable_to_check[edge];
                if value < 0.0 {
                    sign = -sign;
                }
                let magnitude = value.abs();
                if magnitude < smallest {
                    second = smallest;
                    smallest = magnitude;
                } else if magnitude < second {
                    second = magnitude;
                }
            }
            for edge in start..end {
                let value = self.variable_to_check[edge];
                let magnitude = if value.abs() == smallest {
                    second
                } else {
                    smallest
                };
                let outgoing = if value < 0.0 { -sign } else { sign };
                self.check_to_variable[edge] = NORMALIZE * outgoing * magnitude;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [Rate; 11] = [
        Rate::R1_4,
        Rate::R1_3,
        Rate::R2_5,
        Rate::R1_2,
        Rate::R3_5,
        Rate::R2_3,
        Rate::R3_4,
        Rate::R4_5,
        Rate::R5_6,
        Rate::R8_9,
        Rate::R9_10,
    ];

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

    fn syndrome_is_zero(code: &Ldpc, codeword: &[bool]) -> bool {
        (0..code.parity()).all(|check| {
            !code
                .checks
                .row(check)
                .iter()
                .fold(false, |parity, &variable| {
                    parity ^ codeword[variable as usize]
                })
        })
    }

    #[test]
    fn every_code_has_the_length_its_rate_promises() {
        for (rate, information) in RATES.into_iter().zip([
            16_200, 21_600, 25_920, 32_400, 38_880, 43_200, 48_600, 51_840, 54_000, 57_600, 58_320,
        ]) {
            let code = Ldpc::new(rate, Frame::Normal).unwrap_or_else(|| panic!("{rate:?} normal"));
            assert_eq!(code.length, NORMAL);
            assert_eq!(code.information, information, "{rate:?}");
            assert_eq!(rate.information(Frame::Normal), information, "{rate:?}");
            assert!(code.parity().is_multiple_of(GROUP));
        }
        for (rate, information) in RATES[..10].iter().zip([
            3_240, 5_400, 6_480, 7_200, 9_720, 10_800, 11_880, 12_600, 13_320, 14_400,
        ]) {
            let code = Ldpc::new(*rate, Frame::Short).unwrap_or_else(|| panic!("{rate:?} short"));
            assert_eq!(code.length, SHORT);
            assert_eq!(code.information, information, "{rate:?}");
            assert_eq!(rate.information(Frame::Short), information, "{rate:?}");
        }
        assert!(Ldpc::new(Rate::R9_10, Frame::Short).is_none());
        assert_eq!(Rate::R9_10.information(Frame::Short), 0);
    }

    #[test]
    fn every_encoded_word_satisfies_its_parity_checks() {
        for frame in [Frame::Short, Frame::Normal] {
            for rate in RATES {
                let Some(code) = Ldpc::new(rate, frame) else {
                    continue;
                };
                let information = message(code.information, 7);
                let mut codeword = Vec::new();
                code.encode(&information, &mut codeword);
                assert_eq!(codeword.len(), code.length);
                assert!(
                    syndrome_is_zero(&code, &codeword),
                    "{rate:?} {frame:?} leaves a non-zero syndrome"
                );
            }
        }
    }

    fn llrs(codeword: &[bool], confidence: f32) -> Vec<f32> {
        codeword
            .iter()
            .map(|&bit| if bit { -confidence } else { confidence })
            .collect()
    }

    #[test]
    fn every_very_low_rate_code_has_the_length_its_table_promises() {
        for (rate, frame, information) in [
            (Rate::R2_9, Frame::Normal, 14_400),
            (Rate::R1_5, Frame::Medium, 6_480),
            (Rate::R11_45, Frame::Medium, 7_920),
            (Rate::R1_3, Frame::Medium, 10_800),
            (Rate::R11_45, Frame::Short, 3_960),
            (Rate::R4_15, Frame::Short, 4_320),
        ] {
            let code = Ldpc::new(rate, frame).unwrap_or_else(|| panic!("{rate:?} {frame:?}"));
            assert_eq!(code.length, frame.length(), "{rate:?} {frame:?}");
            assert_eq!(code.information, information, "{rate:?} {frame:?}");
            assert!(code.parity().is_multiple_of(GROUP), "{rate:?} {frame:?}");
            let message = message(information, 29);
            let mut codeword = Vec::new();
            code.encode(&message, &mut codeword);
            assert!(syndrome_is_zero(&code, &codeword), "{rate:?} {frame:?}");
        }
        assert!(Ldpc::new(Rate::R2_9, Frame::Short).is_none());
        assert!(Ldpc::new(Rate::R3_4, Frame::Medium).is_none());
    }

    #[test]
    fn a_shortened_and_punctured_word_decodes_back_to_its_message() {
        for (rate, frame, shape) in [
            (
                Rate::R2_9,
                Frame::Normal,
                Shape {
                    shorten: 0,
                    period: 15,
                    punctured: 3_240,
                },
            ),
            (
                Rate::R1_5,
                Frame::Medium,
                Shape {
                    shorten: 640,
                    period: 25,
                    punctured: 980,
                },
            ),
            (
                Rate::R1_4,
                Frame::Short,
                Shape {
                    shorten: 560,
                    period: 30,
                    punctured: 250,
                },
            ),
            (
                Rate::R4_15,
                Frame::Short,
                Shape {
                    shorten: 0,
                    period: 8,
                    punctured: 1_224,
                },
            ),
        ] {
            let mut code = Ldpc::new(rate, frame).expect("a code");
            let information = message(code.message(shape), 31);
            let mut codeword = Vec::new();
            code.encode_shaped(&information, shape, &mut codeword);
            assert_eq!(
                codeword.len(),
                code.transmitted(shape),
                "{rate:?} {frame:?}"
            );
            let mut received = llrs(&codeword, 4.0);
            for position in (0..received.len()).step_by(419) {
                received[position] = -received[position];
            }
            let mut expanded = Vec::new();
            code.expand(&received, shape, &mut expanded);
            let mut out = Vec::new();
            assert!(
                code.decode(&expanded, &mut out).is_some(),
                "{rate:?} {frame:?} did not converge"
            );
            assert_eq!(out[shape.shorten..], information, "{rate:?} {frame:?}");
            assert!(out[..shape.shorten].iter().all(|&bit| !bit));
        }
    }

    #[test]
    fn a_clean_codeword_decodes_without_an_iteration() {
        let mut code = Ldpc::new(Rate::R1_2, Frame::Short).expect("short 1/2");
        let information = message(code.information, 11);
        let mut codeword = Vec::new();
        code.encode(&information, &mut codeword);
        let mut out = Vec::new();
        assert_eq!(code.decode(&llrs(&codeword, 4.0), &mut out), Some(0));
        assert_eq!(out, information);
    }

    #[test]
    fn scattered_errors_are_repaired() {
        let mut code = Ldpc::new(Rate::R3_4, Frame::Short).expect("short 3/4");
        let information = message(code.information, 13);
        let mut codeword = Vec::new();
        code.encode(&information, &mut codeword);
        let mut received = llrs(&codeword, 4.0);
        for position in (0..received.len()).step_by(37) {
            received[position] = -received[position];
        }
        let mut out = Vec::new();
        let iterations = code.decode(&received, &mut out).expect("a decoded frame");
        assert!(iterations > 0, "the errors were not actually present");
        assert_eq!(out, information);
    }

    #[test]
    fn a_normal_frame_decodes_at_every_rate() {
        for rate in RATES {
            let mut code = Ldpc::new(rate, Frame::Normal).expect("a normal code");
            let information = message(code.information, 17);
            let mut codeword = Vec::new();
            code.encode(&information, &mut codeword);
            let mut received = llrs(&codeword, 4.0);
            for position in (0..received.len()).step_by(211) {
                received[position] = -received[position];
            }
            let mut out = Vec::new();
            assert!(
                code.decode(&received, &mut out).is_some(),
                "{rate:?} did not converge"
            );
            assert_eq!(out, information, "{rate:?}");
        }
    }

    #[test]
    fn noise_does_not_converge_on_a_codeword() {
        let mut code = Ldpc::new(Rate::R1_2, Frame::Short).expect("short 1/2");
        let mut state = 0x1357_9bdfu32;
        let received: Vec<f32> = (0..code.length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 16) as f32 / 16_384.0 - 2.0
            })
            .collect();
        let mut out = Vec::new();
        assert!(code.decode(&received, &mut out).is_none());
        assert!(out.is_empty());
    }
}
