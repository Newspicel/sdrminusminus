use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::pocsag_bch_encode;

const WORDS: usize = 88;
const SYNC_MARKER: u32 = 0xA6C6_AAAA;

#[derive(Clone, Debug)]
pub struct Page {
    pub address: u32,
    pub text: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Flex1600_2,
    Flex1600_4,
    Flex3200_2,
    Flex3200_4,
}

impl Mode {
    fn sync(self) -> u16 {
        match self {
            Self::Flex1600_2 => 0x870C,
            Self::Flex1600_4 => 0xB068,
            Self::Flex3200_2 => 0x7B18,
            Self::Flex3200_4 => 0xDEA0,
        }
    }

    fn symbol_rate(self) -> usize {
        match self {
            Self::Flex1600_2 | Self::Flex1600_4 => 1_600,
            Self::Flex3200_2 | Self::Flex3200_4 => 3_200,
        }
    }

    fn levels(self) -> usize {
        match self {
            Self::Flex1600_2 | Self::Flex3200_2 => 2,
            Self::Flex1600_4 | Self::Flex3200_4 => 4,
        }
    }
}

fn reverse31(word: u32) -> u32 {
    word.reverse_bits() >> 1
}

fn reverse21(word: u32) -> u32 {
    word.reverse_bits() >> 11
}

fn encode_word(data: u32) -> u32 {
    let pocsag = pocsag_bch_encode(reverse21(data & 0x1F_FFFF));
    reverse31(pocsag >> 1) | (pocsag & 1) << 31
}

fn checksum(mut data: u32) -> u32 {
    let sum = (1..5).map(|index| data >> (index * 4) & 0xF).sum::<u32>() + (data >> 20);
    data |= (0xF_u32.wrapping_sub(sum)) & 0xF;
    data
}

fn text_words(text: &str) -> Vec<u32> {
    let mut slots = vec![0u8];
    slots.extend(text.bytes().filter(u8::is_ascii).map(|byte| byte & 0x7F));
    slots.push(3);
    while !slots.len().is_multiple_of(3) {
        slots.push(3);
    }
    slots
        .as_chunks::<3>()
        .0
        .iter()
        .map(|chunk| u32::from(chunk[0]) | u32::from(chunk[1]) << 7 | u32::from(chunk[2]) << 14)
        .collect()
}

fn frame_words(page: &Page) -> [u32; WORDS] {
    let mut words = [0u32; WORDS];
    let text = text_words(&page.text);
    let message_start = 3;
    let message_len = text.len() + 1;
    words[0] = checksum(2 << 10);
    words[1] = page.address + 0x8000;
    words[2] = checksum((5 << 4) | (message_start as u32) << 7 | (message_len as u32) << 14);
    words[message_start] = 3 << 11 | 1 << 19;
    for (slot, word) in words[message_start + 1..].iter_mut().zip(text) {
        *slot = word;
    }
    words.map(encode_word)
}

fn push_msb(bits: &mut Vec<bool>, value: u64, count: usize) {
    bits.extend((0..count).rev().map(|bit| value >> bit & 1 == 1));
}

fn push_lsb(bits: &mut Vec<bool>, value: u32, count: usize) {
    bits.extend((0..count).map(|bit| value >> bit & 1 == 1));
}

fn header_bits(mode: Mode, cycle: u8, frame: u8) -> Vec<bool> {
    let sync_code = mode.sync();
    let sync = u64::from(sync_code) << 48 | u64::from(SYNC_MARKER) << 16 | u64::from(!sync_code);
    let fiw = encode_word(checksum(u32::from(cycle) << 4 | u32::from(frame) << 8));
    let mut bits = (0..960).map(|index| index % 2 == 0).collect::<Vec<_>>();
    push_msb(&mut bits, sync, 64);
    bits.extend((0..16).map(|index| index % 2 == 0));
    push_lsb(&mut bits, fiw, 32);
    bits
}

fn phase_bits(words: &[u32; WORDS]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(WORDS * 32);
    for block in 0..11 {
        for bit in 0..32 {
            for word in 0..8 {
                bits.push(words[block * 8 + word] >> bit & 1 == 1);
            }
        }
    }
    bits
}

fn data_symbols(page: &Page, mode: Mode) -> Vec<f64> {
    let active = phase_bits(&frame_words(page));
    let idle = phase_bits(&[encode_word(0); WORDS]);
    let phases = [&active, &idle, &idle, &idle];
    let mut frequencies = Vec::with_capacity(mode.symbol_rate() * 176 / 100);
    let pair = |left: bool, right: bool| match (left, right) {
        (true, false) => -4_800.0,
        (true, true) => -1_600.0,
        (false, true) => 1_600.0,
        (false, false) => 4_800.0,
    };
    match (mode.symbol_rate(), mode.levels()) {
        (1_600, 2) => frequencies.extend(
            active
                .iter()
                .map(|bit| if *bit { -4_800.0 } else { 4_800.0 }),
        ),
        (1_600, 4) => frequencies.extend(
            phases[0]
                .iter()
                .zip(phases[1])
                .map(|(left, right)| pair(*left, *right)),
        ),
        (3_200, 2) => {
            for (&phase_a, &phase_c) in phases[0].iter().zip(phases[2]) {
                frequencies.push(if phase_a { -4_800.0 } else { 4_800.0 });
                frequencies.push(if phase_c { -4_800.0 } else { 4_800.0 });
            }
        }
        (3_200, 4) => {
            for ((&phase_a, &phase_b), (&phase_c, &phase_d)) in phases[0]
                .iter()
                .zip(phases[1])
                .zip(phases[2].iter().zip(phases[3]))
            {
                frequencies.push(pair(phase_a, phase_b));
                frequencies.push(pair(phase_c, phase_d));
            }
        }
        _ => unreachable!(),
    }
    frequencies
}

fn append_symbols(
    iq: &mut Vec<Complex<f32>>,
    phase: &mut f64,
    frequencies: &[f64],
    sps: usize,
    rate: f64,
) {
    for &frequency in frequencies {
        let step = TAU * frequency / rate;
        for _ in 0..sps {
            *phase += step;
            iq.push(Complex::new(phase.cos() as f32, phase.sin() as f32));
        }
    }
}

#[must_use]
pub fn transmission_mode(
    page: &Page,
    cycle: u8,
    frame: u8,
    rate: f64,
    mode: Mode,
) -> Vec<Complex<f32>> {
    let header = header_bits(mode, cycle, frame)
        .into_iter()
        .map(|bit| if bit { -4_800.0 } else { 4_800.0 })
        .collect::<Vec<_>>();
    let sync2 = (0..mode.symbol_rate() / 40)
        .map(|index| if index % 2 == 0 { -4_800.0 } else { 4_800.0 })
        .collect::<Vec<_>>();
    let data = data_symbols(page, mode);
    let tail = (0..mode.symbol_rate() * 6 / 100)
        .map(|index| if index % 2 == 0 { -4_800.0 } else { 4_800.0 })
        .collect::<Vec<_>>();
    let mut iq = Vec::new();
    let mut phase = 0.0;
    append_symbols(
        &mut iq,
        &mut phase,
        &header,
        (rate / 1_600.0) as usize,
        rate,
    );
    let data_sps = (rate / mode.symbol_rate() as f64) as usize;
    append_symbols(&mut iq, &mut phase, &sync2, data_sps, rate);
    append_symbols(&mut iq, &mut phase, &data, data_sps, rate);
    append_symbols(&mut iq, &mut phase, &tail, data_sps, rate);
    iq
}

#[must_use]
pub fn transmission(page: &Page, cycle: u8, frame: u8, rate: f64) -> Vec<Complex<f32>> {
    transmission_mode(page, cycle, frame, rate, Mode::Flex1600_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_words_survive_the_interleaver() {
        let page = Page {
            address: 123_456,
            text: "FLEX TEST".to_owned(),
        };
        for mode in [
            Mode::Flex1600_2,
            Mode::Flex1600_4,
            Mode::Flex3200_2,
            Mode::Flex3200_4,
        ] {
            assert_eq!(
                data_symbols(&page, mode).len(),
                mode.symbol_rate() * 176 / 100
            );
        }
    }
}
