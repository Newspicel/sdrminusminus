use num_complex::Complex;

use super::super::pl;

const POLYNOMIALS: [u8; 5] = [0b10101, 0b10111, 0b11011, 0b11111, 0b11001];
const STATES: usize = 16;
const MAX_BITS: usize = 16;
pub const SOF: [bool; 20] = {
    let word = 0x9D564u32;
    let mut out = [false; 20];
    let mut index = 0;
    while index < 20 {
        out[index] = word >> (19 - index) & 1 == 1;
        index += 1;
    }
    out
};
pub const PLSCODE: usize = 160;
const PUNCTURED: [usize; 20] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 84, 92, 100, 108, 116, 124, 132, 140, 148, 156,
];
pub const POSTAMBLE: usize = 900;

#[must_use]
pub fn bpsk(bit: bool) -> Complex<f32> {
    if bit {
        -pl::pilot_symbol()
    } else {
        pl::pilot_symbol()
    }
}

#[must_use]
pub fn soft(symbol: Complex<f32>) -> f32 {
    (symbol * pl::pilot_symbol().conj()).re
}

const fn output(register: u8) -> u8 {
    let mut word = 0u8;
    let mut index = 0;
    while index < 5 {
        word = word << 1 | ((register & POLYNOMIALS[index]).count_ones() % 2) as u8;
        index += 1;
    }
    word
}

pub fn convolve(bits: &[bool], out: &mut [bool]) {
    let count = bits.len();
    for i in 0..count {
        let register = (0..5).fold(0u8, |word, delay| {
            word << 1 | u8::from(bits[(i + count - delay) % count])
        });
        let word = output(register);
        for (j, bit) in out[5 * i..5 * i + 5].iter_mut().enumerate() {
            *bit = word >> (4 - j) & 1 == 1;
        }
    }
}

fn branch(soft: &[f32], register: u8) -> f32 {
    let word = output(register);
    soft.iter()
        .enumerate()
        .map(|(j, &value)| {
            if word >> (4 - j) & 1 == 1 {
                -value
            } else {
                value
            }
        })
        .sum()
}

fn run(soft: &[f32], count: usize, start: usize, bits: &mut [bool]) -> f32 {
    let mut metric = [f32::NEG_INFINITY; STATES];
    let mut history = [[0u8; STATES]; MAX_BITS];
    metric[start] = 0.0;
    for (step, row) in history.iter_mut().enumerate().take(count) {
        let mut next = [f32::NEG_INFINITY; STATES];
        for (state, &value) in metric.iter().enumerate() {
            if value == f32::NEG_INFINITY {
                continue;
            }
            for bit in 0..2u8 {
                let register = bit << 4 | state as u8;
                let target = usize::from(register >> 1);
                let candidate = value + branch(&soft[5 * step..5 * step + 5], register);
                if candidate > next[target] {
                    next[target] = candidate;
                    row[target] = state as u8;
                }
            }
        }
        metric = next;
    }
    let mut state = start;
    for step in (0..count).rev() {
        bits[step] = state >> 3 & 1 == 1;
        state = usize::from(history[step][state]);
    }
    metric[start]
}

pub fn viterbi(soft: &[f32], bits: &mut [bool]) -> f32 {
    let count = bits.len().min(MAX_BITS);
    let mut best = f32::NEG_INFINITY;
    let mut trial = [false; MAX_BITS];
    for start in 0..STATES {
        let metric = run(soft, count, start, &mut trial);
        if metric > best {
            best = metric;
            bits[..count].copy_from_slice(&trial[..count]);
        }
    }
    let total: f32 = soft[..5 * count].iter().map(|value| value.abs()).sum();
    best / total.max(1e-12)
}

#[must_use]
pub fn word(bits: &[bool]) -> u32 {
    bits.iter().fold(0, |word, &bit| word << 1 | u32::from(bit))
}

pub fn unpack(value: u32, width: usize, out: &mut [bool]) {
    for (index, bit) in out.iter_mut().take(width).enumerate() {
        *bit = value >> (width - 1 - index) & 1 == 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    Standard,
    Robust,
    MostRobust,
    Efficient,
}

impl Protection {
    #[must_use]
    pub const fn from_bits(value: u32) -> Self {
        match value & 3 {
            0 => Self::Standard,
            1 => Self::Robust,
            2 => Self::MostRobust,
            _ => Self::Efficient,
        }
    }

    #[cfg(any(test, feature = "synth"))]
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::Standard => 0,
            Self::Robust => 1,
            Self::MostRobust => 2,
            Self::Efficient => 3,
        }
    }

    #[must_use]
    pub const fn copies(self) -> usize {
        match self {
            Self::Standard | Self::Efficient => 1,
            Self::Robust => 2,
            Self::MostRobust => 5,
        }
    }

    #[must_use]
    pub const fn symbols(self) -> usize {
        match self {
            Self::Efficient => pl::SLOT,
            _ => self.copies() * 2 * pl::SLOT,
        }
    }

    #[cfg(any(test, feature = "synth"))]
    #[must_use]
    pub const fn postamble(self) -> usize {
        match self {
            Self::Efficient => 90,
            Self::Standard => 180,
            Self::Robust => 360,
            Self::MostRobust => 900,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub pointer: u16,
    pub pilots: bool,
    pub protection: Protection,
    pub system: u8,
}

pub const HEADER: usize = 720;
pub const TRAILER_START: usize = 1350;

#[cfg(any(test, feature = "synth"))]
fn header_bits(header: Header, format: u8) -> ([bool; 16], usize) {
    let mut bits = [false; 16];
    let pointer = u32::from(header.pointer & 0x7FF);
    let value = if format == 4 {
        pointer << 3 | u32::from(header.pilots) << 2 | header.protection.bits()
    } else {
        pointer << 5
            | header.protection.bits() << 3
            | u32::from(header.pilots) << 2
            | u32::from(header.system & 3)
    };
    let width = if format == 4 { 14 } else { 16 };
    unpack(value, width, &mut bits);
    (bits, width)
}

fn parse_header(bits: &[bool], format: u8) -> Header {
    let value = word(bits);
    if format == 4 {
        Header {
            pointer: (value >> 3) as u16,
            pilots: value >> 2 & 1 == 1,
            protection: Protection::from_bits(value),
            system: 0,
        }
    } else {
        Header {
            pointer: (value >> 5) as u16,
            pilots: value >> 2 & 1 == 1,
            protection: Protection::from_bits(value >> 3),
            system: (value & 3) as u8,
        }
    }
}

#[must_use]
pub const fn header_symbols(format: u8) -> usize {
    if format == 4 { 630 } else { HEADER }
}

#[cfg(any(test, feature = "synth"))]
pub fn header(header: Header, format: u8, out: &mut Vec<Complex<f32>>) {
    let (bits, width) = header_bits(header, format);
    let mut coded = [false; 80];
    convolve(&bits[..width], &mut coded[..5 * width]);
    for index in 0..header_symbols(format) {
        let bit = if format == 4 {
            coded[index % (5 * width)]
        } else {
            coded[index / 9]
        };
        out.push(bpsk(bit));
    }
}

pub fn read_header(symbols: &[Complex<f32>], format: u8) -> Option<(Header, f32)> {
    let width = if format == 4 { 14 } else { 16 };
    let length = header_symbols(format);
    if symbols.len() < length {
        return None;
    }
    let mut soft_bits = [0.0f32; 80];
    for (index, &symbol) in symbols[..length].iter().enumerate() {
        let bit = if format == 4 {
            index % (5 * width)
        } else {
            index / 9
        };
        soft_bits[bit] += soft(symbol);
    }
    let mut bits = [false; 16];
    let confidence = viterbi(&soft_bits[..5 * width], &mut bits[..width]);
    Some((parse_header(&bits[..width], format), confidence))
}

#[must_use]
pub const fn indication(protection: Protection, k: usize) -> bool {
    let third = k / 72;
    match protection {
        Protection::Standard => false,
        Protection::Robust => third > 0,
        Protection::MostRobust => third < 2,
        Protection::Efficient => third != 1,
    }
}

#[must_use]
pub fn read_indication(symbols: &[Complex<f32>]) -> Option<(Protection, f32)> {
    if symbols.len() < 216 {
        return None;
    }
    let mut best = (Protection::Standard, f32::NEG_INFINITY);
    let mut total = 0.0;
    for protection in [
        Protection::Standard,
        Protection::Robust,
        Protection::MostRobust,
        Protection::Efficient,
    ] {
        let score: f32 = symbols[..216]
            .iter()
            .enumerate()
            .map(|(k, &symbol)| (symbol * bpsk(indication(protection, k)).conj()).re)
            .sum();
        if score > best.1 {
            best = (protection, score);
        }
    }
    for &symbol in &symbols[..216] {
        total += symbol.norm();
    }
    Some((best.0, best.1 / f32::max(total, 1e-12)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plh {
    pub mcs: u8,
    pub tsn: u8,
}

fn plh_code(plh: Plh) -> [bool; PLSCODE] {
    let mut bits = [false; 16];
    unpack(u32::from(plh.mcs) << 8 | u32::from(plh.tsn), 16, &mut bits);
    let mut coded = [false; PLSCODE];
    convolve(&bits, &mut coded[..80]);
    coded.copy_within(..80, 80);
    coded
}

fn qpsk(first: bool, second: bool) -> Complex<f32> {
    let amplitude = std::f32::consts::FRAC_1_SQRT_2;
    Complex::new(
        if first { -amplitude } else { amplitude },
        if second { -amplitude } else { amplitude },
    )
}

pub fn plh(plh: Plh, protection: Protection, out: &mut Vec<Complex<f32>>) {
    let coded = plh_code(plh);
    if protection == Protection::Efficient {
        out.extend(SOF.iter().map(|&bit| bpsk(bit)));
        let mut kept = coded
            .iter()
            .enumerate()
            .filter(|(index, _)| !PUNCTURED.contains(index))
            .map(|(_, &bit)| bit);
        while let (Some(first), Some(second)) = (kept.next(), kept.next()) {
            out.push(qpsk(first, second));
        }
        return;
    }
    for _ in 0..protection.copies() {
        out.extend(SOF.iter().map(|&bit| bpsk(bit)));
        out.extend(coded.iter().map(|&bit| bpsk(bit)));
    }
}

pub fn read_plh(symbols: &[Complex<f32>], protection: Protection) -> Option<(Plh, f32)> {
    if symbols.len() < protection.symbols() {
        return None;
    }
    let mut soft_bits = [0.0f32; 80];
    if protection == Protection::Efficient {
        let mut kept = (0..PLSCODE).filter(|index| !PUNCTURED.contains(index));
        for &symbol in &symbols[SOF.len()..pl::SLOT] {
            let scaled = symbol * std::f32::consts::SQRT_2;
            for value in [scaled.re, scaled.im] {
                if let Some(index) = kept.next() {
                    soft_bits[index % 80] += value;
                }
            }
        }
    } else {
        for copy in symbols[..protection.symbols()]
            .as_chunks::<{ 2 * pl::SLOT }>()
            .0
        {
            for (index, &symbol) in copy[SOF.len()..].iter().enumerate() {
                soft_bits[index % 80] += soft(symbol);
            }
        }
    }
    let mut bits = [false; 16];
    let confidence = viterbi(&soft_bits, &mut bits);
    let value = word(&bits);
    Some((
        Plh {
            mcs: (value >> 8) as u8,
            tsn: value as u8,
        },
        confidence,
    ))
}

#[must_use]
pub fn postamble() -> [bool; POSTAMBLE] {
    let mut out = [false; POSTAMBLE];
    let first = [false, true, true, false, true, true, false, true, true];
    let second = [true, false, true, false, true, true, true, false, true];
    out[..9].copy_from_slice(&first);
    for n in 9..511 {
        out[n] = out[n - 9] ^ out[n - 7] ^ out[n - 2] ^ out[n - 1];
    }
    out[511..520].copy_from_slice(&second);
    for n in 520..POSTAMBLE {
        out[n] = out[n - 9] ^ out[n - 4];
    }
    out
}

#[cfg(any(test, feature = "synth"))]
pub fn bundle_header(code: u8, replicas: usize, out: &mut Vec<Complex<f32>>) {
    let bits = pl::signalling_bits(pl::Signalling::from_code(code & 0x7F));
    for index in 0..replicas * bits.len() {
        out.push(pl::bpsk(index, bits[index % bits.len()]));
    }
}

#[must_use]
pub fn read_bundle_header(symbols: &[Complex<f32>], replicas: usize) -> Option<(u8, f32)> {
    let length = replicas * 64;
    if symbols.len() < length {
        return None;
    }
    let mut folded = [Complex::new(0.0f32, 0.0); 64];
    let mut total = 0.0f32;
    for (index, &symbol) in symbols[..length].iter().enumerate() {
        folded[index % 64] += symbol * pl::bpsk(index % 2, false).conj();
        total += symbol.norm();
    }
    let mut best = (0u8, f32::NEG_INFINITY);
    for code in 0..128u8 {
        let bits = pl::signalling_bits(pl::Signalling::from_code(code));
        let score: f32 = bits
            .iter()
            .zip(&folded)
            .map(|(&bit, value)| if bit { -value.re } else { value.re })
            .sum();
        if score > best.1 {
            best = (code, score);
        }
    }
    Some((best.0, best.1 / total.max(1e-12)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy(symbols: &mut [Complex<f32>], level: f32, seed: u32) {
        let mut state = seed | 1;
        for symbol in symbols {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *symbol += Complex::new(
                (state >> 16) as f32 / 32_768.0 - 1.0,
                (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
            ) * level;
        }
    }

    #[test]
    fn the_tail_biting_code_is_decoded_back() {
        for value in [0u32, 1, 0x8000, 0xA5C3, 0xFFFF, 0x1234] {
            let mut bits = [false; 16];
            unpack(value, 16, &mut bits);
            let mut coded = [false; 80];
            convolve(&bits, &mut coded);
            let soft: Vec<f32> = coded
                .iter()
                .enumerate()
                .map(|(index, &bit)| {
                    let flip = index % 11 == 3;
                    if bit != flip { -1.0 } else { 1.0 }
                })
                .collect();
            let mut decoded = [false; 16];
            viterbi(&soft, &mut decoded);
            assert_eq!(word(&decoded), value);
        }
    }

    #[test]
    fn every_header_layout_round_trips() {
        for format in [4u8, 5] {
            for protection in [
                Protection::Standard,
                Protection::Robust,
                Protection::MostRobust,
                Protection::Efficient,
            ] {
                let sent = Header {
                    pointer: 1234,
                    pilots: true,
                    protection,
                    system: if format == 4 { 0 } else { 2 },
                };
                let mut symbols = Vec::new();
                header(sent, format, &mut symbols);
                assert_eq!(symbols.len(), header_symbols(format));
                noisy(&mut symbols, 1.2, 7);
                let (read, confidence) = read_header(&symbols, format).expect("a header");
                assert_eq!(read, sent, "format {format}");
                assert!(confidence > 0.3, "{confidence}");
            }
        }
    }

    #[test]
    fn every_protection_level_carries_the_physical_layer_header() {
        for protection in [
            Protection::Standard,
            Protection::Robust,
            Protection::MostRobust,
            Protection::Efficient,
        ] {
            let sent = Plh { mcs: 157, tsn: 254 };
            let mut symbols = Vec::new();
            plh(sent, protection, &mut symbols);
            assert_eq!(symbols.len(), protection.symbols(), "{protection:?}");
            noisy(&mut symbols, 0.5, 11);
            let (read, confidence) = read_plh(&symbols, protection).expect("a header");
            assert_eq!(read, sent, "{protection:?}");
            assert!(confidence > 0.5);
        }
    }

    #[test]
    fn the_protection_indication_is_read_back() {
        for protection in [
            Protection::Standard,
            Protection::Robust,
            Protection::MostRobust,
            Protection::Efficient,
        ] {
            let mut symbols: Vec<Complex<f32>> =
                (0..216).map(|k| bpsk(indication(protection, k))).collect();
            noisy(&mut symbols, 1.5, 3);
            assert_eq!(
                read_indication(&symbols).map(|read| read.0),
                Some(protection)
            );
        }
    }

    #[test]
    fn the_postamble_matches_the_printed_sequence() {
        let bits = postamble();
        let head = "0110110111100111010010101111011000011111010000011000001001101010010110100001";
        let tail = "1000100011001000111010101101100011100010010101000110110011111001";
        let render = |range: &[bool]| -> String {
            range
                .iter()
                .map(|&bit| if bit { '1' } else { '0' })
                .collect()
        };
        assert_eq!(render(&bits[..head.len()]), head);
        assert_eq!(render(&bits[POSTAMBLE - tail.len()..]), tail);
    }

    #[test]
    fn a_bundle_header_names_its_code_through_noise() {
        for (code, replicas) in [(0u8, 6), (4 << 2, 6), (99, 6), (32 | 7, 4), (70, 4)] {
            let mut symbols = Vec::new();
            bundle_header(code, replicas, &mut symbols);
            noisy(&mut symbols, 1.5, 5);
            let (read, confidence) = read_bundle_header(&symbols, replicas).expect("a header");
            assert_eq!(read, code);
            assert!(confidence > 0.2);
        }
    }

    #[test]
    fn the_start_of_frame_is_the_documented_word() {
        assert_eq!(word(&SOF), 0x9D564);
    }
}
