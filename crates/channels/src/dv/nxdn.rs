//! NXDN decoder: 4FSK in 6.25 kHz at 2400 symbols per second, or 12.5 kHz at 4800.
use std::sync::LazyLock;

use num_complex::Complex;
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

/// Frame sync word 0xCDF59, 20 bits.
pub(crate) const FSW: u64 = 0x000C_DF59;
pub(crate) const FSW_BITS: u32 = 20;
pub(crate) const SYNC_TOLERANCE: u32 = 2;

/// The LICH is the 16 bits after the sync.
const LICH_BITS: usize = 8;
const LICH_SYMBOLS: usize = 8;
/// The sync outlasts the LICH in the window: its ten symbols are what the level and centre
/// estimates are anchored to.
const FSW_SYMBOLS: usize = FSW_BITS as usize / 2;

/// One frame, sync word to sync word: 384 bits at either channel width. The cadence every
/// real transmission holds, and the confirmation a lone parity bit cannot give.
const FRAME_SYMBOLS: u64 = 192;
const POST_FSW_SYMBOLS: usize = FRAME_SYMBOLS as usize - FSW_SYMBOLS;
const SACCH_SYMBOLS: usize = 30;
const VOICE_START: usize = LICH_SYMBOLS + SACCH_SYMBOLS;

pub(crate) const RRC_ALPHA: f64 = 0.2;

/// Descriptors differ only in the channel width, and the width picks the symbol rate: 6.25 kHz
/// NXDN is 2400 symbols per second at ±1050 Hz, 12.5 kHz is 4800 at ±1944 Hz.
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

/// Occupied RF band relative to the channel offset, in Hz.
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
            // A width change is a different symbol rate, so the front end is rebuilt rather
            // than retuned; nothing it has learned about the old signal applies.
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
    countdown: usize,
    hunting: bool,
    bits: Vec<bool>,
    last_kind: Option<DvFrameKind>,
    /// Symbols seen, the clock frame cadence is measured against.
    clock: u64,
    /// The last sync's frame and where its sync word ended, held until the next sync word
    /// confirms the cadence.
    held: Option<Held>,
    sync_at: u64,
    vocoder: MbeDecoder,
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
        if frame.kind == DvFrameKind::Voice && self.last_kind == Some(DvFrameKind::Voice) {
            return;
        }
        self.last_kind = Some(frame.kind);
        out.events.push(DecoderEvent::Dv(frame));
    }

    fn frame(&mut self) -> Option<(DvFrame, Vec<[bool; 72]>)> {
        self.window.bits(0, POST_FSW_SYMBOLS, &mut self.bits);
        let mut register = 0xE4u16;
        for symbol in 0..POST_FSW_SYMBOLS {
            let pn = register & 1 != 0;
            let feedback = (register ^ (register >> 4)) & 1;
            register = register >> 1 | feedback << 8;
            self.bits[symbol * 2] ^= pn;
        }
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

/// Whether two sync words sit a whole number of frames apart, give or take a symbol of clock
/// slip. Multiples count so one corrupted LICH does not also cost its neighbour the chain.
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

    /// The wide variant is a different radio to the front end: twice the symbol rate and twice
    /// the deviation, and the same framing above it.
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
        // The last frame deliberately remains unreported: NXDN waits for the following sync
        // to confirm cadence before trusting its one-bit LICH parity.
        assert_tone_audio(&audio, 16);
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(57, 0.5, 400_000);
        assert!(decode(&mut channel(NxdnBandwidth::Narrow), &noise).is_empty());
    }
}
