mod lanes;
mod layered;
mod layout;

use layered::Layered;
use layout::Layout;

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
const MAX_ITERATIONS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rate {
    R100_180,
    R104_180,
    R116_180,
    R11_20,
    R124_180,
    R128_180,
    R132_180,
    R135_180,
    R13_18,
    R13_45,
    R140_180,
    R14_45,
    R154_180,
    R18_30,
    R20_30,
    R22_30,
    R23_36,
    R25_36,
    R26_45,
    R28_45,
    R32_45,
    R7_15,
    R7_9,
    R8_15,
    R90_180,
    R96_180,
    R9_20,

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
            Self::R100_180 => "100/180",
            Self::R104_180 => "104/180",
            Self::R116_180 => "116/180",
            Self::R11_20 => "11/20",
            Self::R124_180 => "124/180",
            Self::R128_180 => "128/180",
            Self::R132_180 => "132/180",
            Self::R135_180 => "135/180",
            Self::R13_18 => "13/18",
            Self::R13_45 => "13/45",
            Self::R140_180 => "140/180",
            Self::R14_45 => "14/45",
            Self::R154_180 => "154/180",
            Self::R18_30 => "18/30",
            Self::R20_30 => "20/30",
            Self::R22_30 => "22/30",
            Self::R23_36 => "23/36",
            Self::R25_36 => "25/36",
            Self::R26_45 => "26/45",
            Self::R28_45 => "28/45",
            Self::R32_45 => "32/45",
            Self::R7_15 => "7/15",
            Self::R7_9 => "7/9",
            Self::R8_15 => "8/15",
            Self::R90_180 => "90/180",
            Self::R96_180 => "96/180",
            Self::R9_20 => "9/20",
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
        if let Some(addresses) = super::s2x::tables::addresses(self, frame) {
            return Some(addresses);
        }
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
                _ => return None,
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
                _ => return None,
            },
        })
    }
}

pub struct Ldpc {
    length: usize,
    information: usize,
    decoder: Layered,
    hard: Vec<bool>,
}

impl Ldpc {
    #[must_use]
    pub fn new(rate: Rate, frame: Frame) -> Option<Self> {
        let addresses = rate.addresses(frame)?;
        Self::with_addresses(frame, addresses)
    }

    pub(crate) fn with_addresses(frame: Frame, addresses: &[&[u16]]) -> Option<Self> {
        let length = frame.length();
        let layout = Layout::build(length, addresses)?;
        let information = layout.information;
        Some(Self {
            length,
            information,
            decoder: Layered::new(layout, length),
            hard: vec![false; information],
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

    #[cfg(any(test, feature = "synth"))]
    fn parity_of(&self, full: &[bool]) -> Vec<bool> {
        let layout = &self.decoder.layout;
        let mut parity = vec![false; self.parity()];
        for layer in 0..layout.layers {
            for edge in layout.layer(layer) {
                for lane in 0..GROUP {
                    if let Some(position) = edge.position(lane)
                        && position < self.information
                        && full[position]
                    {
                        parity[layout.check(layer, lane)] ^= true;
                    }
                }
            }
        }
        for index in 1..parity.len() {
            let previous = parity[index - 1];
            parity[index] ^= previous;
        }
        parity
    }

    #[cfg(any(test, feature = "synth"))]
    pub fn encode(&self, information: &[bool], out: &mut Vec<bool>) {
        out.extend_from_slice(information);
        out.extend_from_slice(&self.parity_of(information));
    }

    #[cfg(any(test, feature = "synth"))]
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

    pub fn decode(&mut self, llrs: &[f32], out: &mut Vec<bool>) -> Option<usize> {
        self.decode_with_iterations(llrs, out, MAX_ITERATIONS)
    }

    pub(crate) fn decode_with_iterations(
        &mut self,
        llrs: &[f32],
        out: &mut Vec<bool>,
        limit: usize,
    ) -> Option<usize> {
        if llrs.len() != self.length {
            return None;
        }
        self.decoder.load(llrs);
        let converged = self.decoder.run(limit);
        self.decoder.harden(&mut self.hard);
        let iterations = converged?;
        out.extend_from_slice(&self.hard);
        Some(iterations)
    }

    pub(crate) fn hard_information(&self) -> &[bool] {
        &self.hard
    }
}

#[cfg(test)]
mod flooding;

#[cfg(test)]
mod tests;
