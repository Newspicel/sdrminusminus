//! System Fusion (YSF) decoder: C4FM at 4800 symbols per second in 12.5 kHz, 100 ms frames.
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

/// The sync every frame opens with: 0xD471C9634D, 40 bits.
pub(crate) const SYNC: u64 = 0x00D4_71C9_634D;
pub(crate) const SYNC_BITS: u32 = 40;
/// Looser than the other modes' roughly-a-tenth, and measured: a transmission's first sync
/// meets a cold front end whose clock and level scale are still converging, and unlike the
/// all-outer-symbol syncs of DMR and P25 this pattern mixes ±1 and ±3 — acquisition ISI was
/// measured putting six bit errors into it where the steady state puts zero. YSF can afford
/// the width, alone in the family: everything reported stands behind the FICH's three codes,
/// so a chance match costs a hundred symbols of hunting and never a frame.
pub(crate) const SYNC_TOLERANCE: u32 = 6;

/// The FICH occupies the 100 symbols after the sync.
const FICH_SYMBOLS: usize = 100;
/// Its 200 coded bits carry 96 after the convolutional code, and 48 after the Golay blocks.
const FICH_CODED_BITS: usize = 200;
const FICH_INFO_BITS: usize = 96;
const FICH_BYTES: usize = 6;
const PAYLOAD_SYMBOLS: usize = 360;
const FRAME_AFTER_SYNC_SYMBOLS: usize = FICH_SYMBOLS + PAYLOAD_SYMBOLS;

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

/// Occupied RF band relative to the channel offset, in Hz.
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
        // The front end appends, as every streaming primitive in `dsp` does; the symbols of
        // the last block have already been decoded.
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
    /// Symbols still to arrive before the FICH of a matched frame is complete.
    countdown: usize,
    hunting: bool,
    soft: Vec<i16>,
    coded: Vec<i16>,
    info: Vec<bool>,
    /// Frame type last reported, so a 100 ms heartbeat does not become a log entry per frame.
    last_kind: Option<DvFrameKind>,
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
            soft: Vec::with_capacity(FICH_CODED_BITS),
            coded: Vec::with_capacity(FICH_CODED_BITS),
            info: Vec::with_capacity(FICH_INFO_BITS),
            last_kind: None,
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
        self.half_vocoder.reset();
        self.full_vocoder.reset();
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                self.hunting = true;
                if let Some((frame, data_mode)) = self.fich() {
                    self.voice(data_mode, frame.kind, out);
                    if frame.kind != DvFrameKind::Voice
                        || self.last_kind != Some(DvFrameKind::Voice)
                    {
                        out.events.push(DecoderEvent::Dv(frame.clone()));
                    }
                    self.last_kind = Some(frame.kind);
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

    /// Decode the 100-symbol FICH immediately before the payload now at the window tail.
    fn fich(&mut self) -> Option<(DvFrame, u8)> {
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

        // Four Golay(24,12,8) blocks, 12 information bits each.
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
        Some((frame, data_mode))
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
            // Five 20-symbol DCH blocks alternating with 52-symbol, repetition-protected
            // natural-order AMBE+2 frames.
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

/// The four payload layouts a YSF transmission can use (the FICH "DT" field).
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

    /// Three communication frames arrive between the header and the terminator, and they say
    /// the same thing; the log gets one line, not three.
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
