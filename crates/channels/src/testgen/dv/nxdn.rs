use num_complex::Complex;
use sdrmm_dsp::fec::conv;

use super::{bits, c4fm, dibits, filler};

const RRC_ALPHA: f64 = 0.2;
const FSW: u64 = 0x000C_DF59;
const SACCH_PUNCTURES: [usize; 12] = [5, 11, 17, 23, 29, 35, 41, 47, 53, 59, 65, 71];
const FACCH_PUNCTURES: [usize; 48] = [
    1, 5, 9, 13, 17, 21, 25, 29, 33, 37, 41, 45, 49, 53, 57, 61, 65, 69, 73, 77, 81, 85, 89, 93,
    97, 101, 105, 109, 113, 117, 121, 125, 129, 133, 137, 141, 145, 149, 153, 157, 161, 165, 169,
    173, 177, 181, 185, 189,
];

pub struct Shape {
    pub baud: f64,
    pub deviation_hz: f64,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            baud: 2_400.0,
            deviation_hz: 1_050.0,
        }
    }
}

#[must_use]
pub fn transmission(shape: &Shape, rf_channel: u8, outbound: bool, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(shape, rf_channel, outbound, None, rate)
}

#[must_use]
pub fn addressed_transmission(
    shape: &Shape,
    ran: u8,
    source: u16,
    destination: u16,
    group: bool,
    rate: f64,
) -> Vec<Complex<f32>> {
    let call = layer3(0x01, source, destination, group);
    let release = layer3(0x08, source, destination, group);
    let mut symbols = dibits(&filler(400, 53));
    symbols.extend(addressed_frame(0, ran, &call, None, true));
    for quarter in 0..4 {
        symbols.extend(addressed_frame(2, ran, &call, Some(quarter), false));
    }
    symbols.extend(addressed_frame(0, ran, &release, None, true));
    symbols.extend(frame(1, 1, true, None));
    symbols.extend(dibits(&filler(200, 59)));
    c4fm(&symbols, rate, shape.baud, shape.deviation_hz, RRC_ALPHA)
}

fn addressed_frame(
    functional: u8,
    ran: u8,
    layer3: &[bool; 80],
    quarter: Option<usize>,
    facch: bool,
) -> Vec<u8> {
    let mut lich = bits(1, 2);
    lich.extend(bits(u64::from(functional), 2));
    lich.extend(bits(if facch { 0 } else { 3 }, 2));
    lich.push(true);
    let ones = lich.iter().filter(|b| **b).count();
    lich.push(ones % 2 == 0);

    let out = dibits(&bits(FSW, 20));
    let mut post: Vec<u8> = lich.into_iter().map(|bit| u8::from(bit) << 1).collect();
    let (structure, data) = match quarter {
        Some(quarter) => (3 - quarter as u8, &layer3[quarter * 18..quarter * 18 + 18]),
        None => (0, &layer3[..18]),
    };
    post.extend(dibits(&encode_sacch(structure, ran, data)));
    if facch {
        let coded = dibits(&encode_facch(layer3));
        post.extend_from_slice(&coded);
        post.extend_from_slice(&coded);
    } else {
        post.extend(dibits(&filler(288, 79 + u32::from(functional))));
    }
    finish_frame(out, post)
}

fn layer3(message_type: u8, source: u16, destination: u16, group: bool) -> [bool; 80] {
    let mut out = [false; 80];
    put_bits(&mut out, 2, 6, u32::from(message_type));
    out[16] = !group;
    put_bits(&mut out, 24, 16, u32::from(source));
    put_bits(&mut out, 40, 16, u32::from(destination));
    out
}

fn put_bits(out: &mut [bool], start: usize, len: usize, value: u32) {
    for i in 0..len {
        out[start + i] = value >> (len - 1 - i) & 1 != 0;
    }
}

fn encode_sacch(structure: u8, ran: u8, data: &[bool]) -> Vec<bool> {
    let mut info = bits(u64::from(structure), 2);
    info.extend(bits(u64::from(ran & 0x3f), 6));
    info.extend_from_slice(data);
    append_crc(&mut info, 0x27, 0x3f, 6);
    info.extend([false; 4]);
    let mut coded = Vec::new();
    conv::encode(&info, &mut coded);
    let punctured: Vec<bool> = coded
        .into_iter()
        .enumerate()
        .filter_map(|(i, bit)| (!SACCH_PUNCTURES.contains(&i)).then_some(bit))
        .collect();
    let mut interleaved = vec![false; 60];
    for (i, bit) in punctured.into_iter().enumerate() {
        interleaved[(i % 12) * 5 + i / 12] = bit;
    }
    interleaved
}

fn encode_facch(data: &[bool; 80]) -> Vec<bool> {
    let mut info = data.to_vec();
    append_crc(&mut info, 0x080f, 0x0fff, 12);
    info.extend([false; 4]);
    let mut coded = Vec::new();
    conv::encode(&info, &mut coded);
    let punctured: Vec<bool> = coded
        .into_iter()
        .enumerate()
        .filter_map(|(i, bit)| (!FACCH_PUNCTURES.contains(&i)).then_some(bit))
        .collect();
    let mut interleaved = vec![false; 144];
    for (i, bit) in punctured.into_iter().enumerate() {
        interleaved[(i % 16) * 9 + i / 16] = bit;
    }
    interleaved
}

fn append_crc(info: &mut Vec<bool>, poly: u32, init: u32, width: usize) {
    let top = 1 << (width - 1);
    let mask = (1 << width) - 1;
    let crc = info.iter().fold(init, |crc, &bit| {
        let feedback = bit ^ (crc & top != 0);
        ((crc << 1) ^ if feedback { poly } else { 0 }) & mask
    });
    info.extend((0..width).map(|i| crc >> (width - 1 - i) & 1 != 0));
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    shape: &Shape,
    rf_channel: u8,
    outbound: bool,
    voice: &[[[bool; 72]; 4]; 5],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(shape, rf_channel, outbound, Some(voice), rate)
}

fn transmission_inner(
    shape: &Shape,
    rf_channel: u8,
    outbound: bool,
    voice: Option<&[[[bool; 72]; 4]; 5]>,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 53));
    for (index, functional) in [0u8, 2, 2, 2, 0].into_iter().enumerate() {
        symbols.extend(frame(
            rf_channel,
            functional,
            outbound,
            voice.map(|frames| &frames[index]),
        ));
    }
    symbols.extend(dibits(&filler(200, 59)));
    c4fm(&symbols, rate, shape.baud, shape.deviation_hz, RRC_ALPHA)
}

fn frame(
    rf_channel: u8,
    functional: u8,
    outbound: bool,
    voice: Option<&[[bool; 72]; 4]>,
) -> Vec<u8> {
    let mut lich = bits(u64::from(rf_channel) & 0x03, 2);
    lich.extend(bits(u64::from(functional) & 0x03, 2));
    lich.extend(bits(3, 2));
    lich.push(outbound);
    let ones = lich.iter().filter(|b| **b).count();
    lich.push(ones % 2 == 0);
    let out = dibits(&bits(FSW, 20));
    let mut post: Vec<u8> = lich.into_iter().map(|bit| u8::from(bit) << 1).collect();
    post.extend(dibits(&filler(60, 61 + u32::from(functional))));
    if let Some(voice) = voice {
        for frame in voice {
            post.extend(dibits(frame));
        }
    } else {
        post.extend(dibits(&filler(288, 67 + u32::from(functional))));
    }
    finish_frame(out, post)
}

fn finish_frame(mut out: Vec<u8>, mut post: Vec<u8>) -> Vec<u8> {
    assert_eq!(post.len(), 182);
    let mut register = 0xE4u16;
    for dibit in &mut post {
        let pn = register & 1 != 0;
        let feedback = (register ^ (register >> 4)) & 1;
        register = register >> 1 | feedback << 8;
        if pn {
            *dibit ^= 0b10;
        }
    }
    out.extend(post);
    out
}
