use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, Viterbi5, crc16_msb};
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    YsfParams,
};

use super::{
    INPUT_RATE_HZ, SymbolWindow, c4fm_demod, c4fm_params,
    vocoder::{AMBE_3600_INTERLEAVE, MbeDecoder, half_rate_code_vectors},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const BAUD: f64 = 4_800.0;
pub(crate) const DEVIATION_HZ: f64 = 1_944.0;
pub(crate) const RRC_ALPHA: f64 = 0.2;
pub(crate) const BANDWIDTH_HZ: f64 = 12_500.0;

pub(crate) const SYNC: u64 = 0x00D4_71C9_634D;
pub(crate) const SYNC_BITS: u32 = 40;
pub(crate) const SYNC_TOLERANCE: u32 = 6;

const FICH_SYMBOLS: usize = 100;
const FICH_BYTES: usize = 6;
const PAYLOAD_SYMBOLS: usize = 360;
const FRAME_AFTER_SYNC_SYMBOLS: usize = FICH_SYMBOLS + PAYLOAD_SYMBOLS;
const CALLSIGN_LEN: usize = 10;
const DCH_LARGE_DATA_BYTES: usize = 20;
const DCH_SMALL_DATA_BYTES: usize = 10;
const WHITENING: [u8; DCH_LARGE_DATA_BYTES] = [
    0x93, 0xD7, 0x51, 0x21, 0x9C, 0x2F, 0x6C, 0xD0, 0xEF, 0x0F, 0xF8, 0x3D, 0xF1, 0x73, 0x20, 0x94,
    0xED, 0x1E, 0x7C, 0xD8,
];

const VFR_INTERLEAVE: [usize; 144] = [
    0, 24, 48, 72, 96, 120, 25, 1, 73, 49, 121, 97, 2, 26, 50, 74, 98, 122, 27, 3, 75, 51, 123, 99,
    4, 28, 52, 76, 100, 124, 29, 5, 77, 53, 125, 101, 6, 30, 54, 78, 102, 126, 31, 7, 79, 55, 127,
    103, 8, 32, 56, 80, 104, 128, 33, 9, 81, 57, 129, 105, 10, 34, 58, 82, 106, 130, 35, 11, 83,
    59, 131, 107, 12, 36, 60, 84, 108, 132, 37, 13, 85, 61, 133, 109, 14, 38, 62, 86, 110, 134, 39,
    15, 87, 63, 135, 111, 16, 40, 64, 88, 112, 136, 41, 17, 89, 65, 137, 113, 18, 42, 66, 90, 114,
    138, 43, 19, 91, 67, 139, 115, 20, 44, 68, 92, 116, 140, 45, 21, 93, 69, 141, 117, 22, 46, 70,
    94, 118, 142, 47, 23, 95, 71, 143, 119,
];

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ysf".to_owned(),
    name: "System Fusion".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct YsfChannel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&YsfParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ysf(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "ysf channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-BANDWIDTH_HZ / 2.0, BANDWIDTH_HZ / 2.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    super::channel_filter(BANDWIDTH_HZ)
}

impl ChannelRx for YsfChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            demod: c4fm_demod(&c4fm_params(ctx.input_rate, BAUD, DEVIATION_HZ, RRC_ALPHA)),
            symbols: Vec::new(),
            decoder: Decoder::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings)?;
        Ok(())
    }

    fn retuned(&mut self) {
        self.demod.reset();
        self.decoder.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.symbols.clear();
        self.demod.process(iq, &mut self.symbols);
        for &symbol in &self.symbols {
            self.decoder.push(symbol, out);
        }
    }
}

struct Decoder {
    window: SymbolWindow,
    viterbi: Viterbi5,
    countdown: usize,
    hunting: bool,
    soft: Vec<i16>,
    coded: Vec<i16>,
    info: Vec<bool>,
    last_kind: Option<DvFrameKind>,
    callsigns: Callsigns,
    reported_callsigns: Callsigns,
    bits: Vec<bool>,
    half_vocoder: MbeDecoder,
    full_vocoder: MbeDecoder,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(FRAME_AFTER_SYNC_SYMBOLS),
            viterbi: Viterbi5::new(),
            countdown: 0,
            hunting: true,
            soft: Vec::with_capacity(PAYLOAD_SYMBOLS * 2),
            coded: Vec::with_capacity(PAYLOAD_SYMBOLS),
            info: Vec::with_capacity(PAYLOAD_SYMBOLS / 2),
            last_kind: None,
            callsigns: Callsigns::default(),
            reported_callsigns: Callsigns::default(),
            bits: Vec::with_capacity(PAYLOAD_SYMBOLS * 2),
            half_vocoder: MbeDecoder::half_rate(),
            full_vocoder: MbeDecoder::full_rate(),
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.hunting = true;
        self.last_kind = None;
        self.callsigns = Callsigns::default();
        self.reported_callsigns = Callsigns::default();
        self.half_vocoder.reset();
        self.full_vocoder.reset();
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                self.hunting = true;
                if let Some(mut info) = self.fich() {
                    if info.frame.kind == DvFrameKind::Header {
                        self.callsigns = Callsigns::default();
                    }
                    self.signalling(&info);
                    self.voice(info.data_mode, info.frame.kind, out);
                    if info.frame.kind != DvFrameKind::Voice
                        || self.last_kind != Some(DvFrameKind::Voice)
                        || self.callsigns != self.reported_callsigns
                    {
                        self.callsigns.apply(&mut info.frame);
                        out.events.push(DecoderEvent::Dv(info.frame.clone()));
                        self.reported_callsigns = self.callsigns.clone();
                    }
                    self.last_kind = Some(info.frame.kind);
                }
            }
            return;
        }
        if self.hunting && self.window.sync_distance(SYNC, SYNC_BITS) <= SYNC_TOLERANCE {
            self.window.anchor(SYNC, SYNC_BITS);
            self.hunting = false;
            self.countdown = FRAME_AFTER_SYNC_SYMBOLS;
        }
    }

    fn fich(&mut self) -> Option<FrameInfo> {
        self.window
            .soft_bits(PAYLOAD_SYMBOLS, FICH_SYMBOLS, &mut self.soft);
        self.coded.clear();
        for i in 0..FICH_SYMBOLS {
            let n = 2 * (i / 5) + 40 * (i % 5);
            self.coded.push(self.soft[n]);
            self.coded.push(self.soft[n + 1]);
        }
        self.info.clear();
        self.viterbi.decode(&self.coded, &mut self.info);

        let mut fich = [0u8; FICH_BYTES];
        let mut errors = 0;
        let mut value = 0u64;
        for block in 0..4 {
            let word = self.info[block * 24..(block + 1) * 24]
                .iter()
                .fold(0u64, |acc, &b| acc << 1 | u64::from(b));
            let (info, repaired) = CyclicCode::GOLAY_24_12.decode(word)?;
            errors += repaired;
            value = value << 12 | u64::from(info);
        }
        for (i, byte) in fich.iter_mut().enumerate() {
            *byte = (value >> (40 - i * 8)) as u8;
        }
        let crc = !crc16_msb(0x1021, 0, &fich[..4]);
        if crc != u16::from_be_bytes([fich[4], fich[5]]) {
            return None;
        }

        let frame_type = fich[0] >> 6 & 0x03;
        let kind = match frame_type {
            0 => DvFrameKind::Header,
            2 => DvFrameKind::Terminator,
            _ => DvFrameKind::Voice,
        };
        let mut frame = DvFrame::new(DvMode::Ysf, kind);
        frame.errors_corrected = errors;
        frame.group_call = Some(fich[0] >> 2 & 0x03 != 0x03);
        let data_mode = fich[2] & 0x03;
        frame.opcode = Some(data_mode_name(data_mode).to_owned());
        let dg_id = fich[3] & 0x7F;
        if dg_id != 0 {
            frame.destination = Some(u32::from(dg_id));
        }
        Some(FrameInfo {
            frame,
            callsign_type: fich[0] >> 4 & 0x03,
            frame_number: fich[1] >> 3 & 0x07,
            data_mode,
        })
    }

    fn signalling(&mut self, info: &FrameInfo) {
        if info.callsign_type != 2 {
            return;
        }
        match info.frame.kind {
            DvFrameKind::Header | DvFrameKind::Terminator => {
                if let Some(data) = self.large_data_unit(0) {
                    self.callsigns.learn_pair(&data);
                }
                if let Some(data) = self.large_data_unit(1) {
                    self.callsigns.learn_path(&data);
                }
            }
            DvFrameKind::Voice => match (info.data_mode, info.frame_number) {
                (0 | 1, 0) => {
                    if let Some(data) = self.large_data_unit(0) {
                        self.callsigns.learn_pair(&data);
                    }
                }
                (2, 0) => {
                    if let Some(data) = self.small_data_unit() {
                        remember(&mut self.callsigns.destination, &data);
                    }
                }
                (2, 1) => {
                    if let Some(data) = self.small_data_unit() {
                        remember(&mut self.callsigns.source, &data);
                    }
                }
                (2, 2) => {
                    if let Some(data) = self.small_data_unit() {
                        remember(&mut self.callsigns.downlink, &data);
                    }
                }
                (2, 3) => {
                    if let Some(data) = self.small_data_unit() {
                        remember(&mut self.callsigns.uplink, &data);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn large_data_unit(&mut self, lane: usize) -> Option<[u8; DCH_LARGE_DATA_BYTES]> {
        self.window.soft_bits(0, PAYLOAD_SYMBOLS, &mut self.soft);
        self.coded.clear();
        for i in 0..180 {
            let n = 2 * (i / 9) + 40 * (i % 9);
            for bit in n..n + 2 {
                let block = bit / 72;
                let source = block * 144 + lane * 72 + bit % 72;
                self.coded.push(self.soft[source]);
            }
        }
        self.info.clear();
        self.viterbi.decode(&self.coded, &mut self.info);
        let encoded = pack::<22>(&self.info)?;
        let expected = !crc16_msb(0x1021, 0, &encoded[..20]);
        if expected != u16::from_be_bytes([encoded[20], encoded[21]]) {
            return None;
        }
        Some(std::array::from_fn(|i| encoded[i] ^ WHITENING[i]))
    }

    fn small_data_unit(&mut self) -> Option<[u8; DCH_SMALL_DATA_BYTES]> {
        self.window.soft_bits(0, PAYLOAD_SYMBOLS, &mut self.soft);
        self.coded.clear();
        for i in 0..100 {
            let n = 2 * (i / 5) + 40 * (i % 5);
            for bit in n..n + 2 {
                let block = bit / 40;
                let source = block * 144 + bit % 40;
                self.coded.push(self.soft[source]);
            }
        }
        self.info.clear();
        self.viterbi.decode(&self.coded, &mut self.info);
        let encoded = pack::<12>(&self.info)?;
        let expected = !crc16_msb(0x1021, 0, &encoded[..10]);
        if expected != u16::from_be_bytes([encoded[10], encoded[11]]) {
            return None;
        }
        Some(std::array::from_fn(|i| encoded[i] ^ WHITENING[i]))
    }

    fn voice(&mut self, data_mode: u8, kind: DvFrameKind, out: &mut ChannelOutputs) {
        self.window.bits(0, PAYLOAD_SYMBOLS, &mut self.bits);
        match data_mode {
            0 => {
                if kind != DvFrameKind::Voice {
                    return;
                }
                for block in 0..5 {
                    let start = (block * 72 + 36) * 2;
                    let mut frame = [false; 72];
                    frame.copy_from_slice(&self.bits[start..start + 72]);
                    self.half_vocoder.decode_half_code_vectors(
                        half_rate_code_vectors(&frame, &AMBE_3600_INTERLEAVE),
                        false,
                        out,
                    );
                }
            }
            2 => {
                if kind != DvFrameKind::Voice {
                    return;
                }
                for block in 0..5 {
                    let start = (block * 72 + 20) * 2;
                    let raw = &self.bits[start..start + 104];
                    let mut deinterleaved = [false; 104];
                    let mut register = 0x1C9u16;
                    let mut pn = [false; 104];
                    for bit in &mut pn {
                        *bit = register & 1 != 0;
                        let feedback = (register ^ (register >> 4)) & 1;
                        register = register >> 1 | feedback << 8;
                    }
                    for (i, &bit) in raw.iter().enumerate() {
                        let target = (i % 4) * 26 + i / 4;
                        deinterleaved[target] = bit ^ pn[target];
                    }
                    let mut info = [false; 49];
                    for i in 0..27 {
                        let ones = deinterleaved[i * 3..i * 3 + 3]
                            .iter()
                            .filter(|&&bit| bit)
                            .count();
                        info[i] = ones >= 2;
                    }
                    info[27..].copy_from_slice(&deinterleaved[81..103]);
                    self.half_vocoder.decode_half_info(&info, false, out);
                }
            }
            3 => {
                let (first, count) = match kind {
                    DvFrameKind::Header => (216, 2),
                    DvFrameKind::Voice => (0, 5),
                    _ => return,
                };
                for frame_index in 0..count {
                    let start = (first + frame_index * 72) * 2;
                    self.voice_fr(start, out);
                }
            }
            _ => {}
        }
    }

    fn voice_fr(&mut self, start: usize, out: &mut ChannelOutputs) {
        let transmitted = &self.bits[start..start + 144];
        let mut raw = [false; 144];
        for (i, &bit) in transmitted.iter().enumerate() {
            raw[VFR_INTERLEAVE[i]] = bit;
        }
        let seed = raw[..12]
            .iter()
            .fold(0u16, |acc, &bit| acc << 1 | u16::from(bit));
        let mut state = seed << 4;
        for bit in &mut raw[23..137] {
            state = state.wrapping_mul(173).wrapping_add(13_849);
            *bit ^= state >> 15 != 0;
        }
        let widths = [23usize, 23, 23, 23, 15, 15, 15, 7];
        let mut code = [0u32; 8];
        let mut offset = 0;
        for (word, width) in code.iter_mut().zip(widths) {
            *word = raw[offset..offset + width]
                .iter()
                .fold(0u32, |acc, &bit| acc << 1 | u32::from(bit));
            offset += width;
        }
        self.full_vocoder.decode_full_code_vectors(code, false, out);
    }
}

struct FrameInfo {
    frame: DvFrame,
    callsign_type: u8,
    frame_number: u8,
    data_mode: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Callsigns {
    destination: Option<String>,
    source: Option<String>,
    downlink: Option<String>,
    uplink: Option<String>,
}

impl Callsigns {
    fn learn_pair(&mut self, data: &[u8]) {
        remember(&mut self.destination, &data[..CALLSIGN_LEN]);
        remember(&mut self.source, &data[CALLSIGN_LEN..]);
    }

    fn learn_path(&mut self, data: &[u8]) {
        remember(&mut self.downlink, &data[..CALLSIGN_LEN]);
        remember(&mut self.uplink, &data[CALLSIGN_LEN..]);
    }

    fn apply(&self, frame: &mut DvFrame) {
        frame.destination_call.clone_from(&self.destination);
        frame.source_call.clone_from(&self.source);
        frame.via = match (&self.uplink, &self.downlink) {
            (Some(uplink), Some(downlink)) if uplink != downlink => {
                Some(format!("{uplink} → {downlink}"))
            }
            (Some(uplink), _) => Some(uplink.clone()),
            (_, Some(downlink)) => Some(downlink.clone()),
            _ => None,
        };
    }
}

fn pack<const N: usize>(bits: &[bool]) -> Option<[u8; N]> {
    if bits.len() < N * 8 {
        return None;
    }
    Some(std::array::from_fn(|byte| {
        bits[byte * 8..byte * 8 + 8]
            .iter()
            .fold(0u8, |value, &bit| value << 1 | u8::from(bit))
    }))
}

fn remember(field: &mut Option<String>, bytes: &[u8]) {
    if let Some(value) = callsign(bytes) {
        *field = Some(value);
    }
}

fn callsign(bytes: &[u8]) -> Option<String> {
    if bytes.len() != CALLSIGN_LEN || bytes.iter().any(|byte| !(0x20..=0x7E).contains(byte)) {
        return None;
    }
    let value = std::str::from_utf8(bytes).ok()?.trim_end();
    (!value.is_empty()).then(|| value.to_owned())
}

fn data_mode_name(dt: u8) -> &'static str {
    match dt {
        0 => "V/D mode 1",
        1 => "data FR mode",
        2 => "V/D mode 2",
        _ => "voice FR mode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dv::{
            testutil::{assert_tone_audio, decode, decode_with_audio},
            vocoder::testutil::{full_rate_frames, half_rate_frames, natural_half_rate_frames},
        },
        testgen::dv::ysf as tx,
        testutil::settings,
    };

    fn channel() -> YsfChannel {
        YsfChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Ysf(YsfParams::default())),
        )
        .expect("ysf channel")
    }

    #[test]
    fn decodes_the_frame_information_channel() {
        let fich = tx::Fich::default();
        let iq = tx::transmission(&fich, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.mode, DvMode::Ysf);
        assert_eq!(header.kind, DvFrameKind::Header);
        assert_eq!(header.opcode.as_deref(), Some("V/D mode 2"));
        assert_eq!(header.destination, Some(u32::from(fich.dg_id)));
        assert_eq!(header.errors_corrected, 0);
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Terminator),
            "no terminator: {frames:?}"
        );
    }

    #[test]
    fn a_run_of_identical_frames_is_reported_once() {
        let iq = tx::transmission(&tx::Fich::default(), INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);
        assert_eq!(
            frames
                .iter()
                .filter(|f| f.kind == DvFrameKind::Voice)
                .count(),
            1,
            "{frames:?}"
        );
    }

    #[test]
    fn decodes_callsigns_from_the_data_channel() {
        let fich = tx::Fich::default();
        let call = tx::Call::default();
        let iq = tx::transmission_with_callsigns(&fich, &call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(
            header.destination_call.as_deref(),
            Some(call.destination.as_str())
        );
        assert_eq!(header.source_call.as_deref(), Some(call.source.as_str()));
        assert_eq!(header.via.as_deref(), Some("DB0XYZ → DB0ABC"));
        assert!(frames.iter().all(|frame| {
            frame.source_call.as_deref() == Some(call.source.as_str())
                && frame.destination_call.as_deref() == Some(call.destination.as_str())
        }));
    }

    #[test]
    fn padded_signalling_units_keep_the_callsigns_already_heard() {
        let fich = tx::Fich::default();
        let call = tx::Call::default();
        let iq = tx::transmission_with_padded_communication(&fich, &call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        assert!(!frames.is_empty(), "nothing decoded");
        assert!(
            frames.iter().all(|frame| {
                frame.source_call.as_deref() == Some(call.source.as_str())
                    && frame.destination_call.as_deref() == Some(call.destination.as_str())
                    && frame.via.as_deref() == Some("DB0XYZ → DB0ABC")
            }),
            "{frames:?}"
        );
    }

    #[test]
    fn a_transmission_that_opens_on_a_communication_frame_is_built() {
        let fich = tx::Fich {
            frame_type: 1,
            ..tx::Fich::default()
        };
        assert!(!decode(&mut channel(), &tx::transmission(&fich, INPUT_RATE_HZ)).is_empty());
    }

    #[test]
    fn recovers_vd2_callsigns_after_a_missed_header() {
        let fich = tx::Fich::default();
        let call = tx::Call::default();
        let iq = tx::late_entry_transmission(&fich, &call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let addressed = frames
            .iter()
            .find(|frame| frame.source_call.is_some())
            .expect("a late-entry source callsign");
        assert_eq!(addressed.kind, DvFrameKind::Voice);
        assert_eq!(
            addressed.destination_call.as_deref(),
            Some(call.destination.as_str())
        );
        assert_eq!(addressed.source_call.as_deref(), Some(call.source.as_str()));
    }

    #[test]
    fn decodes_vd1_ambe_voice() {
        let voice = half_rate_frames(15);
        let fich = tx::Fich {
            data_mode: 0,
            ..tx::Fich::default()
        };
        let iq = tx::transmission_with_voice(&fich, tx::Voice::Vd1(&voice), INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert_tone_audio(&audio, 15);
    }

    #[test]
    fn decodes_vd2_ambe_voice() {
        let voice = natural_half_rate_frames(15);
        let fich = tx::Fich {
            data_mode: 2,
            ..tx::Fich::default()
        };
        let iq = tx::transmission_with_voice(&fich, tx::Voice::Vd2(&voice), INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert_tone_audio(&audio, 15);
    }

    #[test]
    fn decodes_voice_fr_imbe() {
        let voice = full_rate_frames(17);
        let fich = tx::Fich {
            data_mode: 3,
            ..tx::Fich::default()
        };
        let iq = tx::transmission_with_voice(&fich, tx::Voice::FullRate(&voice), INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert_tone_audio(&audio, 17);
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(21, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
