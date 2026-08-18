use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DpmrParams, DvFrame,
    DvFrameKind, DvMode,
};

use super::{
    INPUT_RATE_HZ, SymbolWindow, bits_to_u32, c4fm_demod, c4fm_params, tap_c4fm,
    vocoder::{AMBE_3600_INTERLEAVE, MbeDecoder, half_rate_code_vectors},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const BAUD: f64 = 2_400.0;
pub(crate) const DEVIATION_HZ: f64 = 1_050.0;
pub(crate) const RRC_ALPHA: f64 = 0.2;
pub(crate) const BANDWIDTH_HZ: f64 = 6_250.0;

pub(crate) const FS1: u64 = 0x57FF_5F75_D577;
pub(crate) const FS4: u64 = 0xFD55_F5DF_7FDD;
const FS3: u64 = 0x7D_DFF5;
const FS2: u64 = 0x5F_F77D;
pub(crate) const LONG_SYNC_BITS: u32 = 48;
const SHORT_SYNC_BITS: u32 = 24;
pub(crate) const LONG_TOLERANCE: u32 = 4;
const SHORT_TOLERANCE: u32 = 2;

const HI_BITS: usize = 72;
const HI_CODED_BITS: usize = 120;
const HI_BLOCKS: usize = 10;
const HI_SYMBOLS: usize = HI_CODED_BITS / 2;
const CC_SYMBOLS: usize = 12;
const HEADER_SYMBOLS: usize = HI_SYMBOLS * 2 + CC_SYMBOLS;
const SUPERFRAME_SYMBOLS: usize = 756;
const TCH_STARTS: [usize; 4] = [36, 228, 420, 612];
const TCH_SYMBOLS: usize = 144;
const AMBE_SYMBOLS: usize = 36;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dpmr".to_owned(),
    name: "dPMR".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct DpmrChannel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&DpmrParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dpmr(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dpmr channel got {} params",
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

impl ChannelRx for DpmrChannel {
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
        tap_c4fm(out, &self.demod, &self.symbols, BAUD, INPUT_RATE_HZ);
        for &symbol in &self.symbols {
            self.decoder.push(symbol, out);
        }
    }
}

struct Decoder {
    window: SymbolWindow,
    countdown: usize,
    pending: Option<Pending>,
    bits: Vec<bool>,
    in_call: bool,
    voice_call: bool,
    vocoder: MbeDecoder,
}

#[derive(Clone, Copy)]
enum Pending {
    Header { packet: bool },
    Superframe,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(SUPERFRAME_SYMBOLS),
            countdown: 0,
            pending: None,
            bits: Vec::with_capacity(SUPERFRAME_SYMBOLS * 2),
            in_call: false,
            voice_call: false,
            vocoder: MbeDecoder::half_rate(),
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.pending = None;
        self.in_call = false;
        self.voice_call = false;
        self.vocoder.reset();
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                match self.pending.take() {
                    Some(Pending::Header { packet }) => {
                        if let Some((frame, voice_call)) = self.header(packet) {
                            self.in_call = true;
                            self.voice_call = voice_call;
                            out.events.push(DecoderEvent::Dv(frame));
                        }
                    }
                    Some(Pending::Superframe) => self.superframe(out),
                    None => {}
                }
            }
            return;
        }
        if self.pending.is_some() {
            return;
        }
        for (sync, packet) in [(FS1, false), (FS4, true)] {
            if self.window.sync_distance(sync, LONG_SYNC_BITS) <= LONG_TOLERANCE {
                self.window.anchor(sync, LONG_SYNC_BITS);
                self.pending = Some(Pending::Header { packet });
                self.countdown = HEADER_SYMBOLS;
                return;
            }
        }
        if !self.in_call {
            return;
        }
        if self.window.sync_distance(FS2, SHORT_SYNC_BITS) <= SHORT_TOLERANCE {
            self.window.anchor(FS2, SHORT_SYNC_BITS);
            self.pending = Some(Pending::Superframe);
            self.countdown = SUPERFRAME_SYMBOLS;
            return;
        }
        if self.window.sync_distance(FS3, SHORT_SYNC_BITS) <= SHORT_TOLERANCE {
            self.window.anchor(FS3, SHORT_SYNC_BITS);
            self.in_call = false;
            self.voice_call = false;
            out.events.push(DecoderEvent::Dv(DvFrame::new(
                DvMode::Dpmr,
                DvFrameKind::Terminator,
            )));
        }
    }

    fn header(&mut self, packet: bool) -> Option<(DvFrame, bool)> {
        self.window.bits(0, HEADER_SYMBOLS, &mut self.bits);
        let colour = colour_code(&self.bits[HI_CODED_BITS..HI_CODED_BITS + CC_SYMBOLS * 2]);
        let hi = header_info(&self.bits[..HI_CODED_BITS])
            .or_else(|| header_info(&self.bits[HI_CODED_BITS + CC_SYMBOLS * 2..]))?;

        let mut frame = DvFrame::new(
            DvMode::Dpmr,
            if packet {
                DvFrameKind::Data
            } else {
                DvFrameKind::Header
            },
        );
        frame.color_code = colour;
        frame.destination = Some(bits_to_u32(&hi, 4, 24));
        frame.source = Some(bits_to_u32(&hi, 28, 24));
        let mode = bits_to_u32(&hi, 52, 3);
        frame.group_call = Some(mode != 0);
        Some((frame, matches!(mode, 0 | 1 | 5)))
    }

    fn superframe(&mut self, out: &mut ChannelOutputs) {
        if !self.voice_call {
            return;
        }
        self.window.bits(0, SUPERFRAME_SYMBOLS, &mut self.bits);
        for start in TCH_STARTS {
            for frame_start in (start..start + TCH_SYMBOLS).step_by(AMBE_SYMBOLS) {
                let mut frame = [false; 72];
                frame.copy_from_slice(&self.bits[frame_start * 2..frame_start * 2 + 72]);
                self.vocoder.decode_half_code_vectors(
                    half_rate_code_vectors(&frame, &AMBE_3600_INTERLEAVE),
                    false,
                    out,
                );
            }
        }
    }
}

fn colour_code(bits: &[bool]) -> Option<u16> {
    let mut value = 0;
    for &[first, second] in bits.as_chunks::<2>().0 {
        if !second {
            return None;
        }
        value = value << 1 | u16::from(first);
    }
    Some(value)
}

fn header_info(coded: &[bool]) -> Option<Vec<bool>> {
    let mut descrambled = [false; HI_CODED_BITS];
    let mut register = 0x1FFu16;
    for (i, slot) in descrambled.iter_mut().enumerate() {
        let feedback = (register >> 8 ^ register >> 4) & 1;
        register = (register << 1 | feedback) & 0x1FF;
        *slot = coded[i] ^ (feedback == 1);
    }
    let mut blocks = [false; HI_CODED_BITS];
    for r in 0..12 {
        for c in 0..HI_BLOCKS {
            blocks[c * 12 + r] = descrambled[r * HI_BLOCKS + c];
        }
    }
    let mut bytes = [0u8; HI_BLOCKS];
    let mut info = Vec::with_capacity(HI_BITS);
    for (block, byte) in bytes.iter_mut().enumerate() {
        for bit in 0..8 {
            let value = blocks[block * 12 + bit];
            *byte = *byte << 1 | u8::from(value);
            if block * 8 + bit < HI_BITS {
                info.push(value);
            }
        }
    }
    (crc8(&bytes[..9]) == bytes[9]).then_some(info)
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                crc << 1 ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dv::{
            testutil::{assert_tone_audio, decode, decode_with_audio},
            vocoder::testutil::half_rate_frames,
        },
        testgen::dv::dpmr as tx,
        testutil::settings,
    };

    fn channel() -> DpmrChannel {
        DpmrChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Dpmr(DpmrParams::default())),
        )
        .expect("dpmr channel")
    }

    #[test]
    fn decodes_a_header_and_its_end_frame() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.mode, DvMode::Dpmr);
        assert_eq!(header.kind, DvFrameKind::Header);
        assert_eq!(header.color_code, Some(call.colour_code));
        assert_eq!(header.destination, Some(call.called));
        assert_eq!(header.source, Some(call.own));
        assert_eq!(header.group_call, Some(true));
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Terminator),
            "no end frame: {frames:?}"
        );
    }

    #[test]
    fn decodes_an_individual_call() {
        let call = tx::Call {
            mode: 0,
            called: 0x00_00FF,
            ..tx::Call::default()
        };
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);
        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.group_call, Some(false));
        assert_eq!(header.destination, Some(call.called));
    }

    #[test]
    fn decodes_superframe_voice_to_audio() {
        let voice = half_rate_frames(32);
        let iq = tx::transmission_with_voice(&tx::Call::default(), &voice, INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert_tone_audio(&audio, 32);
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(69, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
