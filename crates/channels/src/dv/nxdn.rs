use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    Viterbi5,
    fec::conv::{CONFIDENT, ERASURE},
};
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    NxdnBandwidth, NxdnParams,
};

use super::{
    INPUT_RATE_HZ, SymbolWindow, bits_to_u32, c4fm_demod, c4fm_params,
    vocoder::{AMBE_3600_INTERLEAVE, MbeDecoder, half_rate_code_vectors},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const FSW: u64 = 0x000C_DF59;
pub(crate) const FSW_BITS: u32 = 20;
pub(crate) const SYNC_TOLERANCE: u32 = 2;

const LICH_BITS: usize = 8;
const LICH_SYMBOLS: usize = 8;
const FSW_SYMBOLS: usize = FSW_BITS as usize / 2;

const FRAME_SYMBOLS: u64 = 192;
const POST_FSW_SYMBOLS: usize = FRAME_SYMBOLS as usize - FSW_SYMBOLS;
const SACCH_SYMBOLS: usize = 30;
const VOICE_START: usize = LICH_SYMBOLS + SACCH_SYMBOLS;
const SACCH_BITS: usize = SACCH_SYMBOLS * 2;
const FACCH_BITS: usize = 144;
const FACCH_INFO_BITS: usize = 80;
const FACCH_FIRST: usize = VOICE_START * 2;
const FACCH_SECOND: usize = FACCH_FIRST + FACCH_BITS;

const SACCH_PUNCTURES: [usize; 12] = [5, 11, 17, 23, 29, 35, 41, 47, 53, 59, 65, 71];
const FACCH_PUNCTURES: [usize; 48] = [
    1, 5, 9, 13, 17, 21, 25, 29, 33, 37, 41, 45, 49, 53, 57, 61, 65, 69, 73, 77, 81, 85, 89, 93,
    97, 101, 105, 109, 113, 117, 121, 125, 129, 133, 137, 141, 145, 149, 153, 157, 161, 165, 169,
    173, 177, 181, 185, 189,
];

const L3_VOICE_CALL: u32 = 0x01;
const L3_TX_RELEASE: u32 = 0x08;

pub(crate) const RRC_ALPHA: f64 = 0.2;

pub(crate) fn shape(bandwidth: NxdnBandwidth) -> (f64, f64, f64) {
    match bandwidth {
        NxdnBandwidth::Narrow => (2_400.0, 1_050.0, 6_250.0),
        NxdnBandwidth::Wide => (4_800.0, 1_944.0, 12_500.0),
    }
}

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "nxdn".to_owned(),
    name: "NXDN".to_owned(),
    bandwidth_hz: 6_250.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct NxdnChannel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
    bandwidth: NxdnBandwidth,
    input_rate: f64,
}

fn params(settings: &ChannelSettings) -> Result<&NxdnParams, ChannelError> {
    match &settings.params {
        ChannelParams::Nxdn(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "nxdn channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band(p: &NxdnParams) -> (f64, f64) {
    let (_, _, bandwidth) = shape(p.bandwidth);
    (-bandwidth / 2.0, bandwidth / 2.0)
}

pub(crate) fn channel_filter(p: &NxdnParams) -> ChannelFilter {
    let (_, _, bandwidth) = shape(p.bandwidth);
    super::channel_filter(bandwidth)
}

impl ChannelRx for NxdnChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = *params(&settings)?;
        let (baud, deviation, _) = shape(p.bandwidth);
        Ok(Self {
            demod: c4fm_demod(&c4fm_params(ctx.input_rate, baud, deviation, RRC_ALPHA)),
            symbols: Vec::new(),
            decoder: Decoder::new(),
            bandwidth: p.bandwidth,
            input_rate: ctx.input_rate,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = *params(&settings)?;
        if p.bandwidth != self.bandwidth {
            let (baud, deviation, _) = shape(p.bandwidth);
            self.demod = c4fm_demod(&c4fm_params(self.input_rate, baud, deviation, RRC_ALPHA));
            self.decoder.reset();
            self.bandwidth = p.bandwidth;
        }
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
    countdown: usize,
    hunting: bool,
    bits: Vec<bool>,
    last_kind: Option<DvFrameKind>,
    clock: u64,
    held: Option<Held>,
    sync_at: u64,
    vocoder: MbeDecoder,
    viterbi: Viterbi5,
    soft: Vec<i16>,
    sacch: SacchAssembler,
}

struct Held {
    at: u64,
    frame: DvFrame,
    voice: Vec<[bool; 72]>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(POST_FSW_SYMBOLS.max(FSW_SYMBOLS)),
            countdown: 0,
            hunting: true,
            bits: Vec::with_capacity(LICH_BITS),
            last_kind: None,
            clock: 0,
            held: None,
            sync_at: 0,
            vocoder: MbeDecoder::half_rate(),
            viterbi: Viterbi5::new(),
            soft: Vec::with_capacity(POST_FSW_SYMBOLS * 2),
            sacch: SacchAssembler::default(),
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.hunting = true;
        self.last_kind = None;
        self.clock = 0;
        self.held = None;
        self.sync_at = 0;
        self.vocoder.reset();
        self.sacch.reset();
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        self.clock += 1;
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                self.hunting = true;
                self.held = self.frame().map(|(frame, voice)| Held {
                    at: self.sync_at,
                    frame,
                    voice,
                });
            }
            return;
        }
        if self.hunting && self.window.sync_distance(FSW, FSW_BITS) <= SYNC_TOLERANCE {
            if let Some(held) = &self.held
                && self.clock < held.at + FRAME_SYMBOLS - 1
            {
                return;
            }
            if let Some(held) = self.held.take()
                && on_cadence(self.clock - held.at)
            {
                self.emit(held.frame, out);
                for frame in held.voice {
                    self.vocoder.decode_half_code_vectors(
                        half_rate_code_vectors(&frame, &AMBE_3600_INTERLEAVE),
                        false,
                        out,
                    );
                }
            }
            self.window.anchor(FSW, FSW_BITS);
            self.hunting = false;
            self.sync_at = self.clock;
            self.countdown = POST_FSW_SYMBOLS;
        }
    }

    fn emit(&mut self, frame: DvFrame, out: &mut ChannelOutputs) {
        if frame.kind == DvFrameKind::Voice
            && self.last_kind == Some(DvFrameKind::Voice)
            && frame.source.is_none()
        {
            return;
        }
        self.last_kind = Some(frame.kind);
        out.events.push(DecoderEvent::Dv(frame));
    }

    fn frame(&mut self) -> Option<(DvFrame, Vec<[bool; 72]>)> {
        self.window.soft_bits(0, POST_FSW_SYMBOLS, &mut self.soft);
        let mut register = 0xE4u16;
        for symbol in 0..POST_FSW_SYMBOLS {
            let pn = register & 1 != 0;
            let feedback = (register ^ (register >> 4)) & 1;
            register = register >> 1 | feedback << 8;
            if pn {
                self.soft[symbol * 2] = -self.soft[symbol * 2];
            }
        }
        self.bits.clear();
        self.bits.extend(self.soft.iter().map(|&bit| bit > 0));
        let information: Vec<bool> = (0..LICH_SYMBOLS).map(|i| self.bits[i * 2]).collect();
        if information.iter().filter(|b| **b).count() % 2 == 0 {
            return None;
        }
        let rf_channel = bits_to_u32(&information, 0, 2);
        let functional = bits_to_u32(&information, 2, 2);
        let outbound = information[6];

        let kind = match functional {
            0 => DvFrameKind::Header,
            1 => DvFrameKind::Data,
            _ if rf_channel == 0 => DvFrameKind::Control,
            _ => DvFrameKind::Voice,
        };
        let mut frame = DvFrame::new(DvMode::Nxdn, kind);
        frame.opcode = Some(format!(
            "{} {}",
            channel_name(rf_channel),
            if outbound { "outbound" } else { "inbound" }
        ));
        let option = bits_to_u32(&information, 4, 2);
        let sacch_start = LICH_SYMBOLS * 2;
        if let Some(sacch) = decode_sacch(
            &self.soft[sacch_start..sacch_start + SACCH_BITS],
            &mut self.viterbi,
        ) {
            frame.color_code = Some(u16::from(sacch.ran));
            if let Some(layer3) = self.sacch.push(sacch) {
                apply_layer3(&mut frame, &layer3);
            }
        }
        let facch_ranges: &[(usize, usize)] = match option {
            0 => &[
                (FACCH_FIRST, FACCH_FIRST + FACCH_BITS),
                (FACCH_SECOND, FACCH_SECOND + FACCH_BITS),
            ],
            1 => &[(FACCH_FIRST, FACCH_FIRST + FACCH_BITS)],
            2 => &[(FACCH_SECOND, FACCH_SECOND + FACCH_BITS)],
            _ => &[],
        };
        for &(start, end) in facch_ranges {
            if let Some(layer3) = decode_facch(&self.soft[start..end], &mut self.viterbi) {
                apply_layer3(&mut frame, &layer3);
                break;
            }
        }
        let mut voice = Vec::new();
        if rf_channel != 0 && matches!(functional, 0 | 2) {
            let ranges: &[(usize, usize)] = match option {
                1 => &[(VOICE_START + 72, VOICE_START + 144)],
                2 => &[(VOICE_START, VOICE_START + 72)],
                3 => &[(VOICE_START, VOICE_START + 144)],
                _ => &[],
            };
            for &(start, end) in ranges {
                for frame_start in (start..end).step_by(36) {
                    let mut frame_bits = [false; 72];
                    frame_bits.copy_from_slice(&self.bits[frame_start * 2..frame_start * 2 + 72]);
                    voice.push(frame_bits);
                }
            }
        }
        Some((frame, voice))
    }
}

#[derive(Clone, Copy)]
struct Sacch {
    ran: u8,
    structure: u8,
    data: [bool; 18],
}

struct SacchAssembler {
    data: [bool; 72],
    received: u8,
}

impl Default for SacchAssembler {
    fn default() -> Self {
        Self {
            data: [false; 72],
            received: 0,
        }
    }
}

impl SacchAssembler {
    fn reset(&mut self) {
        self.received = 0;
        self.data.fill(false);
    }

    fn push(&mut self, sacch: Sacch) -> Option<[bool; 72]> {
        let quarter = 3usize.saturating_sub(usize::from(sacch.structure));
        if quarter == 0 {
            self.received = 0;
        }
        self.data[quarter * 18..quarter * 18 + 18].copy_from_slice(&sacch.data);
        self.received |= 1 << quarter;
        if self.received == 0x0f {
            self.received = 0;
            Some(self.data)
        } else {
            None
        }
    }
}

fn decode_sacch(channel: &[i16], viterbi: &mut Viterbi5) -> Option<Sacch> {
    if channel.len() != SACCH_BITS {
        return None;
    }
    let mut deinterleaved = [ERASURE; SACCH_BITS];
    for (i, value) in deinterleaved.iter_mut().enumerate() {
        *value = channel[(i % 12) * 5 + i / 12];
    }
    let mut coded = Vec::with_capacity(80);
    let mut read = 0;
    for i in 0..72 {
        if SACCH_PUNCTURES.contains(&i) {
            coded.push(ERASURE);
        } else {
            coded.push(deinterleaved[read]);
            read += 1;
        }
    }
    coded.extend([-CONFIDENT; 8]);
    let mut info = Vec::with_capacity(40);
    viterbi.decode(&coded, &mut info);
    if info.len() < 32 || crc_msb(0x27, 0x3f, &info[..26]) != bits_to_u32(&info, 26, 6) {
        return None;
    }
    let data = std::array::from_fn(|i| info[8 + i]);
    Some(Sacch {
        structure: bits_to_u32(&info, 0, 2) as u8,
        ran: bits_to_u32(&info, 2, 6) as u8,
        data,
    })
}

fn decode_facch(channel: &[i16], viterbi: &mut Viterbi5) -> Option<[bool; FACCH_INFO_BITS]> {
    if channel.len() != FACCH_BITS {
        return None;
    }
    let mut deinterleaved = [ERASURE; FACCH_BITS];
    for (i, value) in deinterleaved.iter_mut().enumerate() {
        *value = channel[(i % 16) * 9 + i / 16];
    }
    let mut coded = Vec::with_capacity(200);
    let mut read = 0;
    for i in 0..192 {
        if FACCH_PUNCTURES.contains(&i) {
            coded.push(ERASURE);
        } else {
            coded.push(deinterleaved[read]);
            read += 1;
        }
    }
    coded.extend([-CONFIDENT; 8]);
    let mut info = Vec::with_capacity(100);
    viterbi.decode(&coded, &mut info);
    if info.len() < 92 || crc_msb(0x080f, 0x0fff, &info[..80]) != bits_to_u32(&info, 80, 12) {
        return None;
    }
    Some(std::array::from_fn(|i| info[i]))
}

fn crc_msb(poly: u32, init: u32, bits: &[bool]) -> u32 {
    let width = 32 - poly.leading_zeros();
    let top = 1 << (width - 1);
    let mask = (1 << width) - 1;
    bits.iter().fold(init, |crc, &bit| {
        let feedback = bit ^ (crc & top != 0);
        ((crc << 1) ^ if feedback { poly } else { 0 }) & mask
    })
}

fn apply_layer3(frame: &mut DvFrame, layer3: &[bool]) {
    let message_type = bits_to_u32(layer3, 2, 6);
    frame.kind = match message_type {
        L3_VOICE_CALL => frame.kind,
        L3_TX_RELEASE => DvFrameKind::Terminator,
        _ => return,
    };
    frame.group_call = Some(!layer3[16]);
    frame.source = Some(bits_to_u32(layer3, 24, 16));
    frame.destination = Some(bits_to_u32(layer3, 40, 16));
    frame.opcode = Some(
        match message_type {
            L3_VOICE_CALL => "voice call",
            L3_TX_RELEASE => "transmit release",
            _ => unreachable!(),
        }
        .to_owned(),
    );
}

fn on_cadence(delta: u64) -> bool {
    let rem = delta % FRAME_SYMBOLS;
    delta > 0 && rem.min(FRAME_SYMBOLS - rem) <= 1
}

fn channel_name(rf_channel: u32) -> &'static str {
    match rf_channel {
        0 => "control channel",
        1 => "traffic channel",
        2 => "composite channel",
        _ => "traffic channel (composite control)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dv::{
            testutil::{assert_tone_audio, decode, decode_with_audio},
            vocoder::testutil::half_rate_frames,
        },
        testgen::dv::nxdn as tx,
        testutil::settings,
    };

    fn channel(bandwidth: NxdnBandwidth) -> NxdnChannel {
        NxdnChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Nxdn(NxdnParams { bandwidth })),
        )
        .expect("nxdn channel")
    }

    #[test]
    fn decodes_the_link_information_channel_of_a_traffic_channel() {
        let iq = tx::transmission(&tx::Shape::default(), 1, true, INPUT_RATE_HZ);
        let frames = decode(&mut channel(NxdnBandwidth::Narrow), &iq);
        let first = frames.first().expect("a decoded frame");
        assert_eq!(first.mode, DvMode::Nxdn);
        assert_eq!(first.kind, DvFrameKind::Header);
        assert_eq!(
            first.opcode.as_deref(),
            Some("traffic channel outbound"),
            "{frames:?}"
        );
    }

    #[test]
    fn decodes_the_twelve_and_a_half_kilohertz_variant() {
        let shape = tx::Shape {
            baud: 4_800.0,
            deviation_hz: 1_944.0,
        };
        let iq = tx::transmission(&shape, 0, false, INPUT_RATE_HZ);
        let frames = decode(&mut channel(NxdnBandwidth::Wide), &iq);
        let control = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Control)
            .expect("control channel frame");
        assert_eq!(control.opcode.as_deref(), Some("control channel inbound"));
    }

    #[test]
    fn decodes_ehr_voice_to_audio() {
        let encoded = half_rate_frames(20);
        let voice: [[[bool; 72]; 4]; 5] =
            std::array::from_fn(|frame| std::array::from_fn(|slot| encoded[frame * 4 + slot]));
        let iq = tx::transmission_with_voice(&tx::Shape::default(), 1, true, &voice, INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(NxdnBandwidth::Narrow), &iq);
        assert_tone_audio(&audio, 16);
    }

    #[test]
    fn facch_reports_the_call_addresses_and_ran() {
        let iq =
            tx::addressed_transmission(&tx::Shape::default(), 17, 12_345, 234, true, INPUT_RATE_HZ);
        let frames = decode(&mut channel(NxdnBandwidth::Narrow), &iq);
        let header = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Header && frame.source.is_some())
            .expect("FACCH voice-call header");
        assert_eq!(header.color_code, Some(17));
        assert_eq!(header.source, Some(12_345));
        assert_eq!(header.destination, Some(234));
        assert_eq!(header.group_call, Some(true));
        assert_eq!(header.opcode.as_deref(), Some("voice call"));

        let release = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Terminator)
            .expect("FACCH transmit release");
        assert_eq!(
            (release.source, release.destination),
            (Some(12_345), Some(234))
        );
        assert_eq!(release.opcode.as_deref(), Some("transmit release"));
    }

    #[test]
    fn sacch_superframe_supports_late_entry_addressing() {
        let iq =
            tx::addressed_transmission(&tx::Shape::default(), 9, 65_000, 42, false, INPUT_RATE_HZ);
        let frames = decode(&mut channel(NxdnBandwidth::Narrow), &iq);
        let late_entry = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Voice && frame.source == Some(65_000))
            .expect("four assembled SACCH quarters");
        assert_eq!(late_entry.color_code, Some(9));
        assert_eq!(late_entry.destination, Some(42));
        assert_eq!(late_entry.group_call, Some(false));
    }

    #[test]
    fn decodes_the_committed_addressed_iq_fixture() {
        const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/nxdn_addressed_48k.sigmf-data");
        let iq: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|sample| {
                Complex::new(
                    f32::from_le_bytes(sample[..4].try_into().expect("I bytes")),
                    f32::from_le_bytes(sample[4..].try_into().expect("Q bytes")),
                )
            })
            .collect();
        let frames = decode(&mut channel(NxdnBandwidth::Narrow), &iq);
        let addressed = frames
            .iter()
            .find(|frame| frame.source == Some(12_345))
            .expect("fixture FACCH/SACCH addressing");
        assert_eq!(addressed.color_code, Some(17));
        assert_eq!(addressed.destination, Some(234));
        assert_eq!(addressed.group_call, Some(true));
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(57, 0.5, 400_000);
        assert!(decode(&mut channel(NxdnBandwidth::Narrow), &noise).is_empty());
    }
}
