pub(crate) mod ptu;

use sdrmm_dsp::{ReedSolomon, crc16_msb};
use sdrmm_wire::{DecoderEvent, RadiosondeFrame, SondeType};

use super::{
    demod::{Polarity, SymbolClock, SyncMatcher},
    fields::{
        GPS_UTC_LEAP_SECONDS, Motion, ecef_to_geodetic, ecef_velocity_to_enu, empty_frame,
        finite_f32, gps_time, plausible, set_motion, set_position,
    },
};
use ptu::{Calibration, Measurement};

pub(crate) const BAUD: f64 = 4_800.0;
pub(crate) const HEADER: [u8; 8] = [0x10, 0xB6, 0xCA, 0x11, 0x22, 0x96, 0x12, 0xF8];
pub(crate) const MASK: [u8; 64] = [
    0x96, 0x83, 0x3E, 0x51, 0xB1, 0x49, 0x08, 0x98, 0x32, 0x05, 0x59, 0x0E, 0xF9, 0x44, 0xC6, 0x26,
    0x21, 0x60, 0xC2, 0xEA, 0x79, 0x5D, 0x6D, 0xA1, 0x54, 0x69, 0x47, 0x0C, 0xDC, 0xE8, 0x5C, 0xF1,
    0xF7, 0x76, 0x82, 0x7F, 0x07, 0x99, 0xA2, 0x2C, 0x93, 0x7C, 0x30, 0x63, 0xF5, 0x10, 0x2E, 0x61,
    0xD0, 0xBC, 0xB4, 0xB6, 0x06, 0xAA, 0xF4, 0x23, 0x78, 0x6E, 0x3B, 0xAE, 0xBF, 0x7B, 0x4C, 0xC1,
];
pub(crate) const STANDARD_LEN: usize = 320;
pub(crate) const EXTENDED_LEN: usize = 518;
pub(crate) const PARITY_POS: usize = 8;
pub(crate) const PARITY_LEN: usize = 24;
pub(crate) const MESSAGE_POS: usize = 56;
pub(crate) const FRAME_TYPE_POS: usize = 56;
pub(crate) const STANDARD_TYPE: u8 = 0x0F;
pub(crate) const BLOCKS_POS: usize = 0x39;
pub(crate) const RS_PRIMITIVE: u16 = 0x11D;

pub(crate) const BLOCK_STATUS: u8 = 0x79;
pub(crate) const BLOCK_PTU: u8 = 0x7A;
pub(crate) const BLOCK_GPS_POSITION: u8 = 0x7B;
pub(crate) const BLOCK_GPS_INFO: u8 = 0x7C;
pub(crate) const BLOCK_GPS_RAW: u8 = 0x7D;
pub(crate) const BLOCK_EMPTY: u8 = 0x76;

pub(crate) const STATUS_CALIBRATION_INDEX: usize = 23;
pub(crate) const PTU_SENSOR_TEMPERATURE: usize = 38;

const SYNC_TOLERANCE: u32 = 4;
const SMOOTHING: f64 = 0.5;
const MAX_CODEWORD: usize = 255;
const CRC_POLY: u16 = 0x1021;
const CRC_INIT: u16 = 0xFFFF;

pub(crate) fn crc(data: &[u8]) -> u16 {
    crc16_msb(CRC_POLY, CRC_INIT, data)
}

pub(crate) fn reed_solomon() -> ReedSolomon {
    ReedSolomon::new(RS_PRIMITIVE, 0, PARITY_LEN)
}

pub(crate) fn frame_len(type_byte: u8) -> usize {
    let score: i32 = (0..4)
        .map(|bit| i32::from((type_byte >> bit) & 1) - i32::from((type_byte >> (bit + 4)) & 1))
        .sum();
    if score >= 0 {
        STANDARD_LEN
    } else {
        EXTENDED_LEN
    }
}

pub(crate) fn message_len(len: usize) -> usize {
    (len - MESSAGE_POS) / 2
}

pub(crate) fn gather(frame: &[u8; EXTENDED_LEN], len: usize, lane: usize) -> [u8; MAX_CODEWORD] {
    let k = message_len(len);
    let mut codeword = [0u8; MAX_CODEWORD];
    for j in 0..k {
        codeword[j] = frame[MESSAGE_POS + 2 * (k - 1 - j) + lane];
    }
    for m in 0..PARITY_LEN {
        codeword[k + m] = frame[PARITY_POS + PARITY_LEN * lane + PARITY_LEN - 1 - m];
    }
    codeword
}

pub(crate) fn scatter(
    frame: &mut [u8; EXTENDED_LEN],
    len: usize,
    lane: usize,
    codeword: &[u8; MAX_CODEWORD],
) {
    let k = message_len(len);
    for j in 0..k {
        frame[MESSAGE_POS + 2 * (k - 1 - j) + lane] = codeword[j];
    }
    for m in 0..PARITY_LEN {
        frame[PARITY_POS + PARITY_LEN * lane + PARITY_LEN - 1 - m] = codeword[k + m];
    }
}

pub(crate) fn whiten(frame: &mut [u8]) {
    for (index, byte) in frame.iter_mut().enumerate() {
        *byte ^= MASK[index % MASK.len()];
    }
}

fn u16_le(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn i16_le(bytes: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn i32_le(bytes: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u24_le(bytes: &[u8], at: usize) -> f64 {
    f64::from(u32::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        0,
    ]))
}

#[derive(Default)]
struct Blocks {
    status: Option<usize>,
    ptu: Option<usize>,
    gps_info: Option<usize>,
    gps_position: Option<usize>,
}

pub(crate) struct Rs41Frames {
    code: ReedSolomon,
    calibration: Calibration,
    rejected: u32,
}

impl Rs41Frames {
    pub(crate) fn new() -> Self {
        Self {
            code: reed_solomon(),
            calibration: Calibration::new(),
            rejected: 0,
        }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.rejected
    }

    pub(crate) fn decode(
        &mut self,
        frame: &mut [u8; EXTENDED_LEN],
        len: usize,
    ) -> Option<RadiosondeFrame> {
        whiten(&mut frame[..len]);
        frame[len..].fill(0);
        let corrected = self.correct(frame, len);
        let blocks = self.blocks(frame, len);
        let Some(status) = blocks.status else {
            self.rejected += 1;
            return None;
        };
        let Some(mut out) = self.status(frame, status) else {
            self.rejected += 1;
            return None;
        };
        out.errors_corrected = corrected;
        self.measurements(frame, &blocks, &mut out);
        Self::gps(frame, &blocks, &mut out);
        Some(out)
    }

    fn correct(&self, frame: &mut [u8; EXTENDED_LEN], len: usize) -> u32 {
        let n = message_len(len) + PARITY_LEN;
        (0..2)
            .map(|lane| {
                let mut codeword = gather(frame, len, lane);
                match self.code.decode(&mut codeword[..n]) {
                    Some(errors) => {
                        scatter(frame, len, lane, &codeword);
                        errors
                    }
                    None => 0,
                }
            })
            .sum()
    }

    fn blocks(&mut self, frame: &[u8; EXTENDED_LEN], len: usize) -> Blocks {
        let mut found = Blocks::default();
        let mut pos = BLOCKS_POS;
        while pos + 4 <= len {
            let id = frame[pos];
            let end = pos + 2 + usize::from(frame[pos + 1]);
            if end + 2 > len {
                break;
            }
            if crc(&frame[pos + 2..end]) != u16_le(frame, end) {
                self.rejected += u32::from(id != BLOCK_STATUS);
                pos = end + 2;
                continue;
            }
            let slot = match id {
                BLOCK_STATUS => &mut found.status,
                BLOCK_PTU => &mut found.ptu,
                BLOCK_GPS_INFO => &mut found.gps_info,
                BLOCK_GPS_POSITION => &mut found.gps_position,
                _ => {
                    pos = end + 2;
                    continue;
                }
            };
            *slot = Some(pos + 2);
            pos = end + 2;
        }
        found
    }

    fn status(&mut self, frame: &[u8; EXTENDED_LEN], at: usize) -> Option<RadiosondeFrame> {
        let mut serial = [0u8; 8];
        serial.copy_from_slice(&frame[at + 2..at + 10]);
        if !serial.iter().all(u8::is_ascii_alphanumeric) {
            return None;
        }
        let text = String::from_utf8_lossy(&serial).into_owned();
        let mut out = empty_frame(SondeType::Rs41, text);
        out.frame = Some(u32::from(u16_le(frame, at)));
        out.battery_v = Some(f32::from(frame[at + 10]) / 10.0);
        let index = frame[at + STATUS_CALIBRATION_INDEX];
        let start = at + STATUS_CALIBRATION_INDEX + 1;
        self.calibration
            .store(serial, index, &frame[start..start + ptu::SUBFRAME_LEN]);
        Some(out)
    }

    fn measurements(&self, frame: &[u8; EXTENDED_LEN], blocks: &Blocks, out: &mut RadiosondeFrame) {
        let Some(at) = blocks.ptu else {
            return;
        };
        let channel = |index: usize| Measurement {
            signal: u24_le(frame, at + 9 * index),
            low: u24_le(frame, at + 9 * index + 3),
            high: u24_le(frame, at + 9 * index + 6),
        };
        let Some(celsius) = self.calibration.temperature(channel(0)) else {
            return;
        };
        out.temperature_c = finite_f32(celsius);
        out.humidity_pct = self
            .calibration
            .humidity(channel(1), celsius)
            .and_then(finite_f32);
        let sensor = f64::from(i16_le(frame, at + PTU_SENSOR_TEMPERATURE)) / 100.0;
        out.pressure_hpa = self
            .calibration
            .pressure(channel(3), sensor)
            .and_then(finite_f32);
    }

    fn gps(frame: &[u8; EXTENDED_LEN], blocks: &Blocks, out: &mut RadiosondeFrame) {
        if let Some(at) = blocks.gps_info {
            let week = i64::from(u16_le(frame, at));
            let tow_ms = i64::from(i32_le(frame, at + 2));
            out.time = gps_time(week, tow_ms.div_euclid(1_000), GPS_UTC_LEAP_SECONDS);
        }
        let Some(at) = blocks.gps_position else {
            return;
        };
        let axis = |k: usize| f64::from(i32_le(frame, at + 4 * k)) / 100.0;
        let position = ecef_to_geodetic(axis(0), axis(1), axis(2));
        if !plausible(position) {
            return;
        }
        let velocity: [f64; 3] =
            std::array::from_fn(|k| f64::from(i16_le(frame, at + 12 + 2 * k)) / 100.0);
        let [east, north, up] = ecef_velocity_to_enu(position, velocity);
        set_position(out, position);
        set_motion(out, Motion::from_enu(east, north, up));
        out.satellites = Some(frame[at + 18]);
    }
}

enum Receive {
    Hunt,
    Collect {
        polarity: Polarity,
        len: usize,
        expected: usize,
        byte: u8,
        bits: u8,
    },
}

pub(crate) struct Rs41 {
    clock: SymbolClock,
    sync: SyncMatcher,
    receive: Receive,
    frame: [u8; EXTENDED_LEN],
    frames: Rs41Frames,
}

impl Rs41 {
    pub(crate) fn new() -> Self {
        Self {
            clock: SymbolClock::new(BAUD, SMOOTHING),
            sync: SyncMatcher::from_bytes_lsb_first(&HEADER, SYNC_TOLERANCE),
            receive: Receive::Hunt,
            frame: [0; EXTENDED_LEN],
            frames: Rs41Frames::new(),
        }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.frames.rejected()
    }

    pub(crate) fn push(&mut self, audio: &[f32], out: &mut Vec<DecoderEvent>) {
        for &sample in audio {
            if let Some(symbol) = self.clock.push(sample) {
                self.bit(symbol > 0.0, out);
            }
        }
    }

    fn bit(&mut self, bit: bool, out: &mut Vec<DecoderEvent>) {
        match &mut self.receive {
            Receive::Hunt => {
                if let Some(polarity) = self.sync.push(bit) {
                    self.frame[..HEADER.len()].copy_from_slice(&HEADER);
                    self.receive = Receive::Collect {
                        polarity,
                        len: HEADER.len(),
                        expected: EXTENDED_LEN,
                        byte: 0,
                        bits: 0,
                    };
                }
            }
            Receive::Collect {
                polarity,
                len,
                expected,
                byte,
                bits,
            } => {
                *byte |= u8::from(polarity.apply(bit)) << *bits;
                *bits += 1;
                if *bits < 8 {
                    return;
                }
                self.frame[*len] = *byte;
                *len += 1;
                *byte = 0;
                *bits = 0;
                if *len == FRAME_TYPE_POS + 1 {
                    *expected = frame_len(self.frame[FRAME_TYPE_POS] ^ MASK[FRAME_TYPE_POS]);
                }
                if *len == *expected {
                    let len = *len;
                    self.receive = Receive::Hunt;
                    self.sync.clear();
                    if let Some(mut frame) = self.frames.decode(&mut self.frame, len) {
                        frame.rejected = self.frames.rejected();
                        out.push(DecoderEvent::Radiosonde(frame));
                    }
                }
            }
        }
    }
}
