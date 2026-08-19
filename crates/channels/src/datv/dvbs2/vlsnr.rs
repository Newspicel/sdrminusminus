use num_complex::Complex;

use super::{
    bb::{BaseBandData, BaseBandFrame},
    bch::Bch,
    frame::{Constellation, Modulation, demodulate, modulate},
    ldpc::{Frame, Ldpc, Rate, Shape},
    pl::{self, Scrambler, Signalling},
};
use crate::datv::dvbs::PACKET;

const CORRECT: usize = 12;
const NOISE: f32 = 0.25;

pub const HEADER: usize = 900;
pub const PATTERN_BITS: usize = 896;
const ROW_BITS: usize = 56;
const ROWS: [u64; 16] = [
    0x00FB_F23E_837F_9BC4,
    0x0098_708E_0B39_345E,
    0x00F6_A2C9_FE1B_1737,
    0x0084_18D9_5A6F_997A,
    0x007B_7D7B_3E9F_C9EA,
    0x005E_78BA_03A6_D51A,
    0x0027_9CC2_6543_ECD0,
    0x0034_2B04_98BF_3D7D,
    0x00AD_D036_E9D5_312F,
    0x0010_61C6_DF82_6237,
    0x0072_D3E0_9073_84C7,
    0x003B_D5AC_EE25_E2C9,
    0x0059_087D_8261_5ADA,
    0x00E9_AF01_72CF_9DA7,
    0x003F_4835_A406_3F07,
    0x0023_C9AE_ECF2_ED41,
];
const WALSH: [u16; 16] = [
    0b1111_1111_1111_1111,
    0b1010_1010_1010_1010,
    0b1100_1100_1100_1100,
    0b1001_1001_1001_1001,
    0b1111_0000_1111_0000,
    0b1001_0110_1001_0110,
    0b1100_1100_0011_0011,
    0b1001_1001_0110_0110,
    0b1111_0000_0000_1111,
    0b1100_0011_1100_0011,
    0b1010_0101_1010_0101,
    0b0000_0000_1111_1111,
    0b1010_1010_0101_0101,
    0b1010_0101_0101_1010,
    0b1100_0011_0011_1100,
    0b1001_0110_0110_1001,
];

const SET1_CODE: u8 = 128;
const SET2_CODE: u8 = 130;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VlSet {
    One,
    Two,
}

impl VlSet {
    #[must_use]
    pub const fn stream(self) -> usize {
        match self {
            Self::One => 32_400,
            Self::Two => 16_200,
        }
    }

    #[must_use]
    pub const fn payload(self) -> usize {
        match self {
            Self::One => 30_780,
            Self::Two => 14_976,
        }
    }

    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::One => SET1_CODE,
            Self::Two => SET2_CODE,
        }
    }
}

#[must_use]
pub fn set_of(signalling: Signalling) -> Option<VlSet> {
    match signalling.code() & !1 {
        SET1_CODE => Some(VlSet::One),
        SET2_CODE => Some(VlSet::Two),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carrier {
    Qpsk,
    Bpsk,
    BpskSpread,
}

impl Carrier {
    #[must_use]
    pub const fn symbols(self, coded: usize) -> usize {
        match self {
            Self::Qpsk => coded / 2,
            Self::Bpsk => coded,
            Self::BpskSpread => coded * 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VlMode {
    pub header: u8,
    pub set: VlSet,
    pub carrier: Carrier,
    pub rate: Rate,
    pub frame: Frame,
    pub shape: Shape,
    pub message: usize,
    pub label: &'static str,
}

const fn shape(shorten: usize, period: usize, punctured: usize) -> Shape {
    Shape {
        shorten,
        period,
        punctured,
    }
}

pub const CATALOGUE: [VlMode; 9] = [
    VlMode {
        header: 0,
        set: VlSet::One,
        carrier: Carrier::Qpsk,
        rate: Rate::R2_9,
        frame: Frame::Normal,
        shape: shape(0, 15, 3_240),
        message: 14_208,
        label: "QPSK 2/9",
    },
    VlMode {
        header: 1,
        set: VlSet::One,
        carrier: Carrier::Bpsk,
        rate: Rate::R1_5,
        frame: Frame::Medium,
        shape: shape(640, 25, 980),
        message: 5_660,
        label: "BPSK 1/5",
    },
    VlMode {
        header: 2,
        set: VlSet::One,
        carrier: Carrier::Bpsk,
        rate: Rate::R11_45,
        frame: Frame::Medium,
        shape: shape(0, 15, 1_620),
        message: 7_740,
        label: "BPSK 11/45",
    },
    VlMode {
        header: 3,
        set: VlSet::One,
        carrier: Carrier::Bpsk,
        rate: Rate::R1_3,
        frame: Frame::Medium,
        shape: shape(0, 13, 1_620),
        message: 10_620,
        label: "BPSK 1/3",
    },
    VlMode {
        header: 4,
        set: VlSet::One,
        carrier: Carrier::BpskSpread,
        rate: Rate::R1_4,
        frame: Frame::Short,
        shape: shape(560, 30, 250),
        message: 2_512,
        label: "BPSK-S 1/5",
    },
    VlMode {
        header: 5,
        set: VlSet::One,
        carrier: Carrier::BpskSpread,
        rate: Rate::R11_45,
        frame: Frame::Short,
        shape: shape(0, 15, 810),
        message: 3_792,
        label: "BPSK-S 11/45",
    },
    VlMode {
        header: 9,
        set: VlSet::Two,
        carrier: Carrier::Bpsk,
        rate: Rate::R1_4,
        frame: Frame::Short,
        shape: shape(0, 10, 1_224),
        message: 3_072,
        label: "BPSK 1/5",
    },
    VlMode {
        header: 10,
        set: VlSet::Two,
        carrier: Carrier::Bpsk,
        rate: Rate::R4_15,
        frame: Frame::Short,
        shape: shape(0, 8, 1_224),
        message: 4_152,
        label: "BPSK 4/15",
    },
    VlMode {
        header: 11,
        set: VlSet::Two,
        carrier: Carrier::Bpsk,
        rate: Rate::R1_3,
        frame: Frame::Short,
        shape: shape(0, 8, 1_224),
        message: 5_232,
        label: "BPSK 1/3",
    },
];

impl VlMode {
    #[must_use]
    pub fn from_header(header: u8) -> Option<Self> {
        CATALOGUE.iter().copied().find(|mode| mode.header == header)
    }

    #[must_use]
    pub const fn signalling(self) -> Signalling {
        Signalling::from_code(self.set.code() | 1)
    }
}

#[must_use]
pub fn pattern(index: u8) -> [bool; PATTERN_BITS] {
    let walsh = WALSH[usize::from(index) & 15];
    let mut out = [false; PATTERN_BITS];
    for (row, &bits) in ROWS.iter().enumerate() {
        let keep = walsh >> (15 - row) & 1 == 1;
        for step in 0..ROW_BITS {
            let bit = bits >> (ROW_BITS - 1 - step) & 1 == 1;
            out[row * ROW_BITS + step] = bit ^ !keep;
        }
    }
    out
}

pub fn header(index: u8, out: &mut Vec<Complex<f32>>) {
    let bits = pattern(index);
    let start = out.len();
    for step in 0..HEADER {
        let bit = (2..HEADER - 2).contains(&step) && bits[step - 2];
        out.push(pl::bpsk(step, bit));
    }
    debug_assert_eq!(out.len() - start, HEADER);
}

#[must_use]
pub fn read_header(symbols: &[Complex<f32>]) -> Option<(u8, f32)> {
    if symbols.len() < HEADER {
        return None;
    }
    let base = pattern(0);
    let mut rows = [0.0f32; 16];
    let mut energy = 0.0f32;
    for (row, slot) in rows.iter_mut().enumerate() {
        for step in 0..ROW_BITS {
            let at = row * ROW_BITS + step;
            *slot += (symbols[at + 2] * pl::bpsk(at + 2, base[at]).conj()).re;
            energy += symbols[at + 2].norm();
        }
    }
    let mut best = (f32::NEG_INFINITY, 0u8);
    for (index, &walsh) in WALSH.iter().enumerate() {
        let score: f32 = rows
            .iter()
            .enumerate()
            .map(|(row, &value)| {
                if walsh >> (15 - row) & 1 == 1 {
                    value
                } else {
                    -value
                }
            })
            .sum();
        if score > best.0 {
            best = (score, index as u8);
        }
    }
    Some((best.1, best.0 / energy.max(1e-12)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Header,
    Payload(usize),
    Pilot(usize),
    Boundary,
}

impl Piece {
    #[must_use]
    pub const fn len(self) -> usize {
        match self {
            Self::Header => HEADER,
            Self::Payload(len) | Self::Pilot(len) => len,
            Self::Boundary => pl::PILOT_LENGTH,
        }
    }

    #[must_use]
    pub const fn is_pilot(self) -> bool {
        matches!(self, Self::Pilot(_) | Self::Boundary)
    }
}

#[must_use]
pub fn layout(set: VlSet) -> Vec<Piece> {
    let mut out = vec![Piece::Header, Piece::Payload(540), Piece::Boundary];
    let (narrow, extra, wide, groups) = match set {
        VlSet::One => (703, 34, 702, (18, 3)),
        VlSet::Two => (704, 32, 702, (9, 1)),
    };
    let group = pl::PILOT_PERIOD * pl::SLOT;
    let tail = set.stream() - group * (1 + groups.0 + groups.1);
    for _ in 0..groups.0 {
        out.push(Piece::Payload(narrow));
        out.push(Piece::Pilot(extra));
        out.push(Piece::Payload(narrow));
        out.push(Piece::Boundary);
    }
    for _ in 0..groups.1 {
        out.push(Piece::Payload(wide));
        out.push(Piece::Pilot(pl::PILOT_LENGTH));
        out.push(Piece::Payload(wide));
        out.push(Piece::Boundary);
    }
    out.push(Piece::Payload(tail));
    out
}

#[must_use]
pub fn frame_symbols(set: VlSet) -> usize {
    pl::HEADER + layout(set).iter().map(|piece| piece.len()).sum::<usize>()
}

pub struct VlSnrCodec {
    pub mode: VlMode,
    pub ldpc: Ldpc,
    pub bch: Bch,
    pub baseband: BaseBandFrame,
    pub layout: Vec<Piece>,
    constellation: Constellation,
    coded: Vec<bool>,
    bits: Vec<bool>,
    llrs: Vec<f32>,
    expanded: Vec<f32>,
}

impl VlSnrCodec {
    #[must_use]
    pub fn new(mode: VlMode) -> Option<Self> {
        let ldpc = Ldpc::new(mode.rate, mode.frame)?;
        if mode.carrier.symbols(ldpc.transmitted(mode.shape)) != mode.set.payload() {
            return None;
        }
        Some(Self {
            mode,
            ldpc,
            bch: Bch::new(mode.frame, CORRECT, mode.message),
            baseband: BaseBandFrame::new(mode.message),
            layout: layout(mode.set),
            constellation: Constellation::new(Modulation::Qpsk, mode.rate),
            coded: Vec::new(),
            bits: Vec::new(),
            llrs: Vec::new(),
            expanded: Vec::new(),
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.baseband.capacity()
    }

    #[must_use]
    pub const fn field_bytes(&self) -> usize {
        self.baseband.field_bytes()
    }

    fn map(&self, out: &mut Vec<Complex<f32>>) {
        match self.mode.carrier {
            Carrier::Qpsk => modulate(&self.coded, &self.constellation, out),
            Carrier::Bpsk => {
                for (index, &bit) in self.coded.iter().enumerate() {
                    out.push(pl::bpsk(index, bit));
                }
            }
            Carrier::BpskSpread => {
                for (index, &bit) in self.coded.iter().enumerate() {
                    out.push(pl::bpsk(2 * index, bit));
                    out.push(pl::bpsk(2 * index + 1, bit));
                }
            }
        }
    }

    pub fn encode(&mut self, baseband: &[bool], out: &mut Vec<Complex<f32>>) {
        let mut protected = Vec::new();
        self.bch.encode(baseband, &mut protected);
        self.coded.clear();
        self.ldpc
            .encode_shaped(&protected, self.mode.shape, &mut self.coded);
        self.map(out);
    }

    fn soft(&mut self, payload: &[Complex<f32>]) {
        self.llrs.clear();
        match self.mode.carrier {
            Carrier::Qpsk => demodulate(payload, &self.constellation, NOISE, &mut self.llrs),
            Carrier::Bpsk => {
                for (index, &symbol) in payload.iter().enumerate() {
                    self.llrs
                        .push((symbol * pl::bpsk(index, false).conj()).re / NOISE);
                }
            }
            Carrier::BpskSpread => {
                for (index, pair) in payload.as_chunks::<2>().0.iter().enumerate() {
                    let first = (pair[0] * pl::bpsk(2 * index, false).conj()).re;
                    let second = (pair[1] * pl::bpsk(2 * index + 1, false).conj()).re;
                    self.llrs.push((first + second) / NOISE);
                }
            }
        }
    }

    pub fn decode(&mut self, payload: &[Complex<f32>]) -> Option<BaseBandData> {
        self.soft(payload);
        let llrs = std::mem::take(&mut self.llrs);
        self.ldpc.expand(&llrs, self.mode.shape, &mut self.expanded);
        self.llrs = llrs;
        self.bits.clear();
        let expanded = std::mem::take(&mut self.expanded);
        let converged = self.ldpc.decode(&expanded, &mut self.bits).is_some();
        self.expanded = expanded;
        if !converged {
            return None;
        }
        self.bits.drain(..self.mode.shape.shorten);
        self.bch.decode(&mut self.bits)?;
        self.bits.truncate(self.bch.message());
        self.baseband.read(&self.bits)
    }
}

pub struct VlSnrEncoder {
    codec: VlSnrCodec,
    carry: u8,
    scrambler: Scrambler,
    payload: Vec<Complex<f32>>,
    frame: Vec<Complex<f32>>,
}

impl VlSnrEncoder {
    #[must_use]
    pub fn new(mode: VlMode) -> Option<Self> {
        Some(Self {
            codec: VlSnrCodec::new(mode)?,
            carry: 0x47,
            scrambler: Scrambler::new(),
            payload: Vec::new(),
            frame: Vec::new(),
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.codec.capacity()
    }

    #[must_use]
    pub const fn field_bytes(&self) -> usize {
        self.codec.field_bytes()
    }

    pub fn frame(&mut self, packets: &[[u8; PACKET]], out: &mut Vec<Complex<f32>>) -> bool {
        let Some(baseband) = self.codec.baseband.build(packets, &mut self.carry) else {
            return false;
        };
        self.emit(&baseband, out);
        true
    }

    pub fn generic(&mut self, field: &[u8], isi: Option<u8>, out: &mut Vec<Complex<f32>>) -> bool {
        let Some(baseband) = self.codec.baseband.encapsulate(field, isi) else {
            return false;
        };
        self.emit(&baseband, out);
        true
    }

    fn emit(&mut self, baseband: &[bool], out: &mut Vec<Complex<f32>>) {
        self.payload.clear();
        self.codec.encode(baseband, &mut self.payload);
        self.frame.clear();
        let mut taken = 0;
        for piece in &self.codec.layout {
            match piece {
                Piece::Header => header(self.codec.mode.header, &mut self.frame),
                Piece::Payload(len) => {
                    self.frame
                        .extend_from_slice(&self.payload[taken..taken + len]);
                    taken += len;
                }
                Piece::Pilot(len) => self
                    .frame
                    .extend(std::iter::repeat_n(pl::pilot_symbol(), *len)),
                Piece::Boundary => self
                    .frame
                    .extend(std::iter::repeat_n(pl::pilot_symbol(), pl::PILOT_LENGTH)),
            }
        }
        scramble(
            &mut self.scrambler,
            &self.codec.layout,
            self.codec.mode.carrier,
            &mut self.frame,
            true,
        );
        pl::header(self.codec.mode.signalling(), out);
        out.extend_from_slice(&self.frame);
    }
}

/// The sequence is reset after the PLHEADER and runs on through the VL-SNR header without being
/// applied, so the payload starts at the sequence's 900th step.
pub fn scramble(
    scrambler: &mut Scrambler,
    layout: &[Piece],
    carrier: Carrier,
    frame: &mut [Complex<f32>],
    forward: bool,
) {
    scrambler.reset();
    let mut at = 0;
    for piece in layout {
        for index in 0..piece.len() {
            let turn = scrambler.next();
            match piece {
                Piece::Header => continue,
                Piece::Payload(_) if carrier != Carrier::Qpsk => {
                    if turn & 1 == 1 {
                        frame[at + index] = -frame[at + index];
                    }
                }
                _ => {
                    let symbol = &mut frame[at + index];
                    *symbol = pl::rotate(*symbol, if forward { turn } else { 4 - turn });
                }
            }
        }
        at += piece.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_differs_from_every_other_in_half_its_bits() {
        let patterns: Vec<[bool; PATTERN_BITS]> = (0..16).map(pattern).collect();
        for (index, first) in patterns.iter().enumerate() {
            for second in &patterns[index + 1..] {
                let apart = first
                    .iter()
                    .zip(second.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                assert!(
                    apart.abs_diff(PATTERN_BITS / 2) <= PATTERN_BITS / 8,
                    "{apart} bits apart"
                );
            }
        }
    }

    #[test]
    fn a_clean_header_names_its_mode() {
        for mode in CATALOGUE {
            let mut symbols = Vec::new();
            header(mode.header, &mut symbols);
            assert_eq!(symbols.len(), HEADER);
            let (read, confidence) = read_header(&symbols).expect("a full header");
            assert_eq!(read, mode.header, "{}", mode.label);
            assert!(confidence > 0.99, "{}: {confidence}", mode.label);
        }
        for index in 0..16u8 {
            let mut symbols = Vec::new();
            header(index, &mut symbols);
            assert_eq!(read_header(&symbols).map(|fit| fit.0), Some(index));
        }
    }

    #[test]
    fn the_header_is_padded_with_two_zeroes_at_each_end() {
        let mut symbols = Vec::new();
        header(0, &mut symbols);
        for step in [0, 1, HEADER - 2, HEADER - 1] {
            assert_eq!(symbols[step], pl::bpsk(step, false), "symbol {step}");
        }
    }

    #[test]
    fn noise_does_not_pick_a_header_out_of_nothing() {
        let mut state = 0x51ee_7c0du32;
        let noise: Vec<Complex<f32>> = (0..HEADER)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    (state >> 16) as f32 / 32_768.0 - 1.0,
                    (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
                )
            })
            .collect();
        let (_, confidence) = read_header(&noise).expect("a full block");
        assert!(confidence < 0.2, "{confidence}");
    }

    #[test]
    fn each_set_is_the_length_of_the_legacy_frame_it_hides_in() {
        assert_eq!(frame_symbols(VlSet::One), 33_282);
        assert_eq!(frame_symbols(VlSet::Two), 16_686);
        for set in [VlSet::One, VlSet::Two] {
            let pieces = layout(set);
            let payload: usize = pieces
                .iter()
                .filter_map(|piece| match piece {
                    Piece::Payload(len) => Some(*len),
                    _ => None,
                })
                .sum();
            assert_eq!(payload, set.payload(), "{set:?}");
            let stream: usize = pieces
                .iter()
                .filter(|piece| !matches!(piece, Piece::Boundary))
                .map(|piece| piece.len())
                .sum();
            assert_eq!(stream, set.stream(), "{set:?}");
            let regular = pieces
                .iter()
                .filter(|piece| matches!(piece, Piece::Boundary))
                .count();
            assert_eq!(regular, (set.stream() / 90 - 1) / 16, "{set:?}");
        }
    }

    #[test]
    fn a_group_is_sixteen_slots_of_the_stream_it_hides_in() {
        for set in [VlSet::One, VlSet::Two] {
            let mut run = 0usize;
            for piece in layout(set) {
                if matches!(piece, Piece::Boundary) {
                    assert_eq!(run, 16 * 90, "{set:?}");
                    run = 0;
                } else {
                    run += piece.len();
                }
            }
            assert!(run < 16 * 90, "{set:?} ends mid-group");
        }
    }

    #[test]
    fn the_signalling_code_names_the_set_it_belongs_to() {
        for mode in CATALOGUE {
            let signalling = mode.signalling();
            assert!(signalling.pilots);
            assert_eq!(set_of(signalling), Some(mode.set), "{}", mode.label);
        }
        assert_eq!(set_of(Signalling::from_code(128)), Some(VlSet::One));
        assert_eq!(set_of(Signalling::from_code(131)), Some(VlSet::Two));
        assert_eq!(set_of(Signalling::from_code(28 << 2 | 1)), None);
    }

    #[test]
    fn every_mode_fills_the_symbols_its_set_promises() {
        for mode in CATALOGUE {
            let code = super::super::ldpc::Ldpc::new(mode.rate, mode.frame)
                .unwrap_or_else(|| panic!("{}", mode.label));
            assert_eq!(
                code.message(mode.shape),
                mode.message + 12 * mode.frame.correct_bits(),
                "{}",
                mode.label
            );
            assert_eq!(
                mode.carrier.symbols(code.transmitted(mode.shape)),
                mode.set.payload(),
                "{}",
                mode.label
            );
        }
    }
}
