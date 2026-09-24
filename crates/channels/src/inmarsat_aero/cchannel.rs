use sdrmm_dsp::SoftViterbi;

use super::{frame::Scrambler, su};

pub(super) const FRAME_CODED_BITS: usize = 4096;
pub(super) const INFO_BITS: usize = 2714;
const DECODED_BITS: usize = 2730;
const SUBBLOCK_BITS: usize = 109;
const VOICE_FRAMES: usize = 25;
const VOICE_BITS: usize = 96;
const UW_LEN: usize = 52;
const UW_TOLERANCE: u32 = 6;
const BLOCK: usize = 256;
const ROWS: usize = 64;
const COLUMNS: usize = 4;
pub(super) const UW_RAIL1: u64 = 216_866_263_330_005;
pub(super) const UW_RAIL2: u64 = 3_012_071_630_031_408;

fn pattern_bits(value: u64) -> [u8; UW_LEN] {
    let mut pattern = [0u8; UW_LEN];
    for (index, bit) in pattern.iter_mut().enumerate() {
        *bit = ((value >> (UW_LEN - 1 - index)) & 1) as u8;
    }
    pattern
}

fn permute(row: usize) -> usize {
    (27 * row) % ROWS
}

fn depermute(row: usize) -> usize {
    (19 * row) % ROWS
}

struct UwDetector {
    patterns: [[u8; UW_LEN]; 2],
    window: [u8; UW_LEN],
    fill: usize,
    inverted: bool,
}

impl UwDetector {
    fn new() -> Self {
        Self {
            patterns: [pattern_bits(UW_RAIL1), pattern_bits(UW_RAIL2)],
            window: [0; UW_LEN],
            fill: 0,
            inverted: false,
        }
    }

    fn update(&mut self, bit: u8) -> bool {
        self.window.rotate_left(1);
        self.window[UW_LEN - 1] = bit;
        self.fill = (self.fill + 1).min(UW_LEN);
        if self.fill < UW_LEN {
            return false;
        }
        for pattern in &self.patterns {
            let distance: u32 = self
                .window
                .iter()
                .zip(pattern)
                .map(|(&bit, &expected)| u32::from(bit ^ expected))
                .sum();
            if distance <= UW_TOLERANCE {
                self.inverted = false;
                return true;
            }
            if distance >= UW_LEN as u32 - UW_TOLERANCE {
                self.inverted = true;
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum CChannelEvent {
    Voice([u8; 12]),
    SignalUnit([u8; 12]),
}

pub(super) fn su_type_name(type_byte: u8) -> &'static str {
    match type_byte {
        0x01 => "fill",
        0x30 => "call-progress",
        0x60 => "telephony-acknowledge",
        _ => "other",
    }
}

struct LsbAccumulator {
    value: u8,
    count: u8,
}

impl LsbAccumulator {
    fn push(&mut self, bit: u8) -> Option<u8> {
        self.value |= bit << 7;
        self.count += 1;
        if self.count == 8 {
            let byte = self.value;
            self.value = 0;
            self.count = 0;
            return Some(byte);
        }
        self.value >>= 1;
        None
    }
}

pub(super) struct CChannelDeframer {
    detectors: [UwDetector; 2],
    inverted: [bool; 2],
    rail: bool,
    sync_armed: bool,
    synced: bool,
    frame: Vec<f32>,
    viterbi: SoftViterbi,
}

impl CChannelDeframer {
    pub(super) fn new() -> Self {
        Self {
            detectors: [UwDetector::new(), UwDetector::new()],
            inverted: [false; 2],
            rail: false,
            sync_armed: false,
            synced: false,
            frame: Vec::with_capacity(FRAME_CODED_BITS),
            viterbi: SoftViterbi::k7(),
        }
    }

    pub(super) fn push(&mut self, soft: f32) -> Vec<CChannelEvent> {
        self.rail = !self.rail;
        let rail = usize::from(!self.rail);
        let hit = self.detectors[rail].update(u8::from(soft > 0.0));
        if hit {
            self.inverted[rail] = self.detectors[rail].inverted;
        }
        if hit && self.sync_armed {
            self.sync_armed = false;
            self.synced = true;
            self.frame.clear();
            return Vec::new();
        }
        self.sync_armed = hit;
        if !self.synced || self.frame.len() >= FRAME_CODED_BITS {
            return Vec::new();
        }
        self.frame
            .push(if self.inverted[rail] { -soft } else { soft });
        if self.frame.len() < FRAME_CODED_BITS {
            return Vec::new();
        }
        self.decode_frame()
    }

    fn decode_frame(&mut self) -> Vec<CChannelEvent> {
        self.synced = false;
        let mut deleaved = Vec::with_capacity(FRAME_CODED_BITS);
        for block in self.frame.as_chunks::<BLOCK>().0 {
            for column in 0..COLUMNS {
                for row in 0..ROWS {
                    deleaved.push(block[depermute(row) * COLUMNS + column]);
                }
            }
        }
        let mut depunctured = Vec::with_capacity(5460);
        for (index, &soft) in deleaved[..deleaved.len() - 1].iter().enumerate() {
            depunctured.push(soft);
            if index % 3 == 2 {
                depunctured.push(0.0);
            }
        }
        let mut bits = self.viterbi.decode(&depunctured);
        bits.truncate(INFO_BITS);
        if bits.len() < INFO_BITS {
            return Vec::new();
        }
        Scrambler::new().apply(&mut bits);
        let mut out = voice_frames(&bits);
        out.extend(signal_units(&bits));
        out
    }
}

fn voice_frames(bits: &[u8]) -> Vec<CChannelEvent> {
    let mut accumulator = LsbAccumulator { value: 0, count: 0 };
    let mut bytes = Vec::with_capacity(VOICE_FRAMES * 12);
    let mut position = 1usize;
    let mut run = 0usize;
    while position < INFO_BITS {
        bytes.extend(accumulator.push(bits[position]));
        run += 1;
        position += 1;
        if run == VOICE_BITS {
            run = 0;
            position += SUBBLOCK_BITS - VOICE_BITS;
        }
    }
    bytes
        .as_chunks::<12>()
        .0
        .iter()
        .take(VOICE_FRAMES)
        .map(|frame| CChannelEvent::Voice(*frame))
        .collect()
}

fn signal_units(bits: &[u8]) -> Vec<CChannelEvent> {
    let mut accumulator = LsbAccumulator { value: 0, count: 0 };
    let mut unit = Vec::with_capacity(su::SU_LEN);
    let mut out = Vec::new();
    for subblock in 0..24 {
        let offset = subblock * SUBBLOCK_BITS;
        for &bit in &bits[offset + 97..offset + SUBBLOCK_BITS] {
            unit.extend(accumulator.push(bit));
        }
        if unit.len() == su::SU_LEN {
            if su::su_crc_ok(&unit)
                && let Ok(bytes) = <[u8; 12]>::try_from(unit.as_slice())
            {
                out.push(CChannelEvent::SignalUnit(bytes));
            }
            unit.clear();
        }
    }
    out
}

pub(super) struct CChannelEncoder {
    viterbi: SoftViterbi,
}

impl CChannelEncoder {
    pub(super) fn new() -> Self {
        Self {
            viterbi: SoftViterbi::k7(),
        }
    }

    pub(super) fn encode(&mut self, info: &[u8]) -> Vec<u8> {
        let mut bits = info.to_vec();
        bits.resize(DECODED_BITS, 0);
        Scrambler::new().apply(&mut bits);
        let mut punctured: Vec<u8> = self
            .viterbi
            .encode(&bits)
            .into_iter()
            .enumerate()
            .filter(|&(index, _)| index % 4 != 3)
            .map(|(_, bit)| bit)
            .collect();
        punctured.push(0);
        let mut out = vec![0u8; 8];
        for index in 0..UW_LEN {
            out.push(((UW_RAIL1 >> (UW_LEN - 1 - index)) & 1) as u8);
            out.push(((UW_RAIL2 >> (UW_LEN - 1 - index)) & 1) as u8);
        }
        for block in punctured.as_chunks::<BLOCK>().0 {
            let mut written = [0u8; BLOCK];
            for column in 0..COLUMNS {
                for row in 0..ROWS {
                    written[row * COLUMNS + column] = block[column * ROWS + permute(row)];
                }
            }
            out.extend_from_slice(&written);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::{
        oqpsk::{CHANNEL_RATE_HR, OqpskDemod, modulate_oqpsk_rate},
        su::su_with_crc,
        tests::Noise,
    };

    fn frame_with(unit: &[u8], voice_byte: u8) -> Vec<u8> {
        let mut bits = vec![0u8; INFO_BITS];
        for subblock in 0..VOICE_FRAMES {
            let offset = subblock * SUBBLOCK_BITS;
            for index in 0..VOICE_BITS {
                if let Some(bit) = bits.get_mut(offset + 1 + index) {
                    *bit = (voice_byte >> (index % 8)) & 1;
                }
            }
        }
        let mut stream = unit
            .iter()
            .flat_map(|&byte| (0..8).map(move |index| (byte >> index) & 1));
        for subblock in 0..8 {
            let offset = subblock * SUBBLOCK_BITS;
            for bit in &mut bits[offset + 97..offset + SUBBLOCK_BITS] {
                *bit = stream.next().unwrap_or(0);
            }
        }
        bits
    }

    fn call_progress(aes: [u8; 3], ges: u8) -> Vec<u8> {
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x30;
        su10[1..4].copy_from_slice(&aes);
        su10[4] = ges;
        su_with_crc(su10)
    }

    fn split(events: &[CChannelEvent]) -> (Vec<[u8; 12]>, Vec<[u8; 12]>) {
        let voices = events
            .iter()
            .filter_map(|event| match event {
                CChannelEvent::Voice(voice) => Some(*voice),
                CChannelEvent::SignalUnit(_) => None,
            })
            .collect();
        let units = events
            .iter()
            .filter_map(|event| match event {
                CChannelEvent::SignalUnit(unit) => Some(*unit),
                CChannelEvent::Voice(_) => None,
            })
            .collect();
        (voices, units)
    }

    #[test]
    fn loopback_voice_and_su() {
        let info = frame_with(&call_progress([0xAB, 0xCD, 0xEF], 0x44), 0x5A);
        let mut encoder = CChannelEncoder::new();
        let mut deframer = CChannelDeframer::new();
        let mut events = Vec::new();
        for _ in 0..3 {
            for &bit in &encoder.encode(&info) {
                events.extend(deframer.push(if bit == 1 { 0.9 } else { -0.9 }));
            }
        }
        let (voices, units) = split(&events);
        assert_eq!(voices.first().map(|voice| voice[0]), Some(0x5A));
        let unit = units.first().expect("signal unit");
        assert_eq!(unit[0], 0x30);
        assert_eq!(&unit[1..4], &[0xAB, 0xCD, 0xEF]);
        assert_eq!(su_type_name(unit[0]), "call-progress");
    }

    #[test]
    fn interleaver_roundtrip() {
        let block: Vec<u8> = (0..256u32)
            .map(|index| ((index % 2) ^ ((index / 3) % 2)) as u8)
            .collect();
        let mut written = [0u8; BLOCK];
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                written[row * COLUMNS + column] = block[column * ROWS + permute(row)];
            }
        }
        let read: Vec<u8> = (0..COLUMNS)
            .flat_map(|column| (0..ROWS).map(move |row| (row, column)))
            .map(|(row, column)| written[depermute(row) * COLUMNS + column])
            .collect();
        assert_eq!(read, block);
    }

    #[test]
    fn uw_detector_normal_and_inverted() {
        for raw in [UW_RAIL1, UW_RAIL2] {
            for invert in [0u8, 1] {
                let mut detector = UwDetector::new();
                for index in 0..UW_LEN {
                    let bit = invert ^ ((raw >> (UW_LEN - 1 - index)) & 1) as u8;
                    assert_eq!(detector.update(bit), index == UW_LEN - 1);
                }
                assert_eq!(detector.inverted, invert == 1);
            }
        }
    }

    #[test]
    fn decodes_c_channel_voice_and_su_from_iq() {
        let info = frame_with(&call_progress([0x12, 0x34, 0x56], 0x7E), 0xA7);
        let mut encoder = CChannelEncoder::new();
        let bits: Vec<u8> = (0..4).flat_map(|_| encoder.encode(&info)).collect();
        let mut iq = modulate_oqpsk_rate(&bits, 8_400.0, 0.6, CHANNEL_RATE_HR, 90.0, 0.5);
        Noise(0x0123_4567_89ab_cdef).add(&mut iq, 0.02);
        let mut demod = OqpskDemod::new_c_channel(CHANNEL_RATE_HR);
        let mut deframer = CChannelDeframer::new();
        let mut soft = Vec::new();
        let mut events = Vec::new();
        for chunk in iq.chunks(8192) {
            soft.clear();
            demod.process(chunk, &mut soft);
            for &(value, _) in &soft {
                events.extend(deframer.push(value));
            }
        }
        let (voices, units) = split(&events);
        assert_eq!(voices.first().map(|voice| voice[0]), Some(0xA7));
        let unit = units.first().expect("signal unit");
        assert_eq!(unit[0], 0x30);
        assert_eq!(&unit[1..5], &[0x12, 0x34, 0x56, 0x7E]);
    }
}
