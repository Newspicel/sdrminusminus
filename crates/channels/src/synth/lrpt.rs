use num_complex::Complex;
use sdrmm_dsp::{design_rrc, fec::ccsds_rs::CcsdsReedSolomon};
use sdrmm_wire::LrptMode;

use crate::lrpt::{
    image::{FIRST_APID, IMAGE_HEADER, MCU_ID_AT, MCUS_PER_ROW, QUALITY_AT, TELEMETRY_APID, WIDTH},
    jpeg::{
        AC_TABLE, Block, Code, DC_TABLE, MCU_PIXELS, MCU_SIDE, MCUS_PER_PACKET, ZIGZAG,
        forward_dct, magnitude_bits, magnitude_category, quant_table,
    },
    link::{
        ASM, CADU_BYTES, CODED_BYTES, FILL_VCID, IMAGE_VCID, INTERLEAVE, MPDU_DATA,
        NO_PACKET_START, PACKET_HEADER, PacketHeader, SEQUENCE_MODULO, VCDU_BYTES, VcduHeader,
        conv_code, pn_sequence,
    },
};

pub const DEFAULT_APIDS: [u16; 3] = [64, 65, 66];
pub const QUALITY: u8 = 80;
const SYNTH_SPS: usize = 4;
const PULSE_SPAN: usize = 8;
const ROLL_OFF: f64 = 0.6;
const LEAD_FILL: usize = 4;
const TRAIL_FILL: usize = 2;
const ROW_MS: u32 = 1_200;
const IDLE_APID: u16 = 0x7FF;
const MIN_IDLE: usize = PACKET_HEADER + 1;

pub type Cadu = [u8; CADU_BYTES];

pub struct Scene {
    pub apids: Vec<u16>,
    pub lines: usize,
    pub planes: Vec<Vec<u8>>,
}

impl Scene {
    #[must_use]
    pub fn new(apids: &[u16], rows: u16) -> Self {
        let lines = usize::from(rows) * MCU_SIDE;
        let planes = apids
            .iter()
            .map(|&apid| {
                let tilt = f64::from(apid - FIRST_APID);
                (0..lines * WIDTH)
                    .map(|index| {
                        let (x, y) = ((index % WIDTH) as f64, (index / WIDTH) as f64);
                        let value = 128.0
                            + 70.0 * (x * 0.013 + tilt).sin()
                            + 40.0 * (y * 0.09 + 0.7 * tilt).cos();
                        value.round().clamp(0.0, 255.0) as u8
                    })
                    .collect()
            })
            .collect();
        Self {
            apids: apids.to_vec(),
            lines,
            planes,
        }
    }

    #[must_use]
    pub fn plane(&self, apid: u16) -> Option<&[u8]> {
        let index = self.apids.iter().position(|&a| a == apid)?;
        Some(&self.planes[index])
    }

    fn block(&self, plane: usize, row: usize, column: usize) -> Block {
        std::array::from_fn(|index| {
            let (y, x) = (index / MCU_SIDE, index % MCU_SIDE);
            self.planes[plane][(row * MCU_SIDE + y) * WIDTH + column * MCU_SIDE + x]
        })
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    filled: u8,
}

impl BitWriter {
    fn bit(&mut self, bit: bool) {
        if self.filled == 0 {
            self.bytes.push(0);
        }
        if bit && let Some(last) = self.bytes.last_mut() {
            *last |= 0x80 >> self.filled;
        }
        self.filled = (self.filled + 1) % 8;
    }

    fn bits(&mut self, value: u16, count: u8) {
        for shift in (0..count).rev() {
            self.bit(value >> shift & 1 == 1);
        }
    }

    fn code(&mut self, code: Code) {
        self.bits(code.bits, code.len);
    }

    fn finish(mut self) -> Vec<u8> {
        while self.filled != 0 {
            self.bit(true);
        }
        self.bytes
    }
}

fn quantised(block: &Block, quant: &[u16; MCU_PIXELS]) -> [i32; MCU_PIXELS] {
    let coefficients = forward_dct(block);
    std::array::from_fn(|k| {
        let natural = ZIGZAG[k];
        (coefficients[natural] / f32::from(quant[natural]))
            .round()
            .clamp(-1023.0, 1023.0) as i32
    })
}

fn encode_block(writer: &mut BitWriter, zigzag: &[i32; MCU_PIXELS], previous_dc: &mut i32) {
    let difference = (zigzag[0] - *previous_dc).clamp(-2047, 2047);
    *previous_dc += difference;
    let category = magnitude_category(difference);
    writer.code(DC_TABLE.code(category));
    writer.bits(magnitude_bits(difference, category), category);
    let mut run = 0u8;
    for &value in &zigzag[1..] {
        if value == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            writer.code(AC_TABLE.code(0xF0));
            run -= 16;
        }
        let size = magnitude_category(value);
        writer.code(AC_TABLE.code(run << 4 | size));
        writer.bits(magnitude_bits(value, size), size);
        run = 0;
    }
    if run > 0 {
        writer.code(AC_TABLE.code(0x00));
    }
}

#[must_use]
pub fn encode_mcus(blocks: &[Block], quality: u8) -> Vec<u8> {
    let quant = quant_table(quality);
    let mut writer = BitWriter::default();
    let mut previous_dc = 0;
    for block in blocks {
        encode_block(&mut writer, &quantised(block, &quant), &mut previous_dc);
    }
    writer.finish()
}

fn packet(apid: u16, sequence: u32, row: usize, user: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; PACKET_HEADER];
    let ms = row as u32 * ROW_MS;
    bytes.extend_from_slice(&[0, 1]);
    bytes.extend_from_slice(&ms.to_be_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(user);
    PacketHeader {
        apid,
        sequence: (sequence % SEQUENCE_MODULO) as u16,
        length: bytes.len() - PACKET_HEADER,
    }
    .write(&mut bytes);
    bytes
}

fn image_packet(scene: &Scene, plane: usize, row: usize, first: usize, quality: u8) -> Vec<u8> {
    let blocks: Vec<Block> = (first..(first + MCUS_PER_PACKET).min(MCUS_PER_ROW))
        .map(|column| scene.block(plane, row, column))
        .collect();
    let mut user = vec![0u8; IMAGE_HEADER - MCU_ID_AT];
    user[0] = first as u8;
    user[QUALITY_AT - MCU_ID_AT] = quality;
    user.extend_from_slice(&encode_mcus(&blocks, quality));
    user
}

#[must_use]
pub fn packets(scene: &Scene, quality: u8, first_sequence: u32) -> Vec<Vec<u8>> {
    let mut sequence = first_sequence;
    let mut out = Vec::new();
    for row in 0..scene.lines / MCU_SIDE {
        for (plane, &apid) in scene.apids.iter().enumerate() {
            for first in (0..MCUS_PER_ROW).step_by(MCUS_PER_PACKET) {
                let user = image_packet(scene, plane, row, first, quality);
                out.push(packet(apid, sequence, row, &user));
                sequence += 1;
            }
        }
        out.push(packet(TELEMETRY_APID, sequence, row, &[0x55; 32]));
        sequence += 1;
    }
    out
}

fn idle_packet(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    PacketHeader {
        apid: IDLE_APID,
        sequence: 0,
        length: len - PACKET_HEADER,
    }
    .write(&mut bytes);
    bytes
}

fn cadu(header: VcduHeader, zone: &[u8], code: &CcsdsReedSolomon, pn: &[u8]) -> Cadu {
    let mut vcdu = vec![0u8; VCDU_BYTES];
    header.write(&mut vcdu);
    vcdu[VCDU_BYTES - MPDU_DATA..].copy_from_slice(zone);
    let mut coded = Vec::with_capacity(CODED_BYTES);
    code.encode_interleaved(&vcdu, INTERLEAVE, &mut coded);
    let mut out = [0u8; CADU_BYTES];
    out[..4].copy_from_slice(&ASM.to_be_bytes());
    for (index, (slot, &byte)) in out[4..].iter_mut().zip(&coded).enumerate() {
        *slot = byte ^ pn[index];
    }
    out
}

fn fill_cadu(counter: u32, code: &CcsdsReedSolomon, pn: &[u8]) -> Cadu {
    let header = VcduHeader {
        vcid: FILL_VCID,
        counter,
        first_header: NO_PACKET_START,
    };
    cadu(header, &[0u8; MPDU_DATA], code, pn)
}

#[must_use]
pub fn cadus(packets: &[Vec<u8>]) -> Vec<Cadu> {
    let code = CcsdsReedSolomon::new();
    let pn = pn_sequence();
    let mut stream = Vec::new();
    let mut starts = Vec::new();
    for packet in packets {
        starts.push(stream.len());
        stream.extend_from_slice(packet);
    }
    let mut pad = MPDU_DATA - stream.len() % MPDU_DATA;
    if pad < MIN_IDLE {
        pad += MPDU_DATA;
    }
    starts.push(stream.len());
    stream.extend_from_slice(&idle_packet(pad));
    let mut out: Vec<Cadu> = (0..LEAD_FILL)
        .map(|index| fill_cadu(index as u32, &code, &pn))
        .collect();
    for (index, zone) in stream.as_chunks::<MPDU_DATA>().0.iter().enumerate() {
        let begin = index * MPDU_DATA;
        let first_header = starts
            .iter()
            .find(|&&start| (begin..begin + MPDU_DATA).contains(&start))
            .map_or(NO_PACKET_START, |&start| (start - begin) as u16);
        let header = VcduHeader {
            vcid: IMAGE_VCID,
            counter: index as u32,
            first_header,
        };
        out.push(cadu(header, zone, &code, &pn));
    }
    out.extend((0..TRAIL_FILL).map(|index| fill_cadu((LEAD_FILL + index) as u32, &code, &pn)));
    out
}

fn coded_bits(cadus: &[Cadu]) -> Vec<bool> {
    let bits: Vec<bool> = cadus
        .iter()
        .flat_map(|cadu| cadu.iter())
        .flat_map(|&byte| (0..8).rev().map(move |shift| byte >> shift & 1 == 1))
        .collect();
    let mut coded = Vec::with_capacity(bits.len() * 2);
    conv_code().encode(&bits, &mut coded);
    coded
}

#[must_use]
pub fn modulate(mode: LrptMode, cadus: &[Cadu], rate: f64) -> Vec<Complex<f32>> {
    let coded = coded_bits(cadus);
    let level = |bit: bool| if bit { 1.0f32 } else { -1.0 };
    let stagger = if mode.offset() { SYNTH_SPS / 2 } else { 0 };
    let symbols = coded.len() / 2;
    let pulse = design_rrc(SYNTH_SPS as f64, ROLL_OFF, PULSE_SPAN);
    let mut impulses = vec![Complex::new(0.0f32, 0.0); symbols * SYNTH_SPS + stagger + 1];
    for (index, &[first, second]) in coded.as_chunks::<2>().0.iter().enumerate() {
        impulses[index * SYNTH_SPS].re = level(first);
        impulses[index * SYNTH_SPS + stagger].im = level(second);
    }
    let peak = pulse.iter().copied().fold(0.0f32, f32::max);
    let mut shaped = vec![Complex::new(0.0f32, 0.0); impulses.len() + pulse.len()];
    for (index, &impulse) in impulses.iter().enumerate() {
        if impulse == Complex::new(0.0, 0.0) {
            continue;
        }
        for (offset, &tap) in pulse.iter().enumerate() {
            shaped[index + offset] += impulse * (tap / peak * 0.5);
        }
    }
    let native = mode.symbol_rate() * SYNTH_SPS as f64;
    if (native - rate).abs() < 1e-6 {
        shaped
    } else {
        super::resample(&shaped, native, rate)
    }
}

#[must_use]
pub fn transmission(mode: LrptMode, rows: u16, rate: f64) -> Vec<Complex<f32>> {
    let scene = Scene::new(&DEFAULT_APIDS, rows);
    modulate(mode, &cadus(&packets(&scene, QUALITY, 0)), rate)
}
