//! P25 Phase 1 decoder (TIA-102.BAAA): C4FM at 4800 symbols per second in 12.5 kHz.
//!
//! Every frame — header, voice, terminator, trunking block — opens with the same 48-bit sync
//! and a 64-bit network identifier: the 12-bit network access code that separates one system
//! from the next on a shared frequency, and the 4-bit data unit id that says what the frame is.
//! The NID is a BCH(63,16,23) codeword with a parity bit, so eleven bit errors still decode.
//!
//! That is what this reports: the NAC, and the shape of the transmission — header, call in
//! progress, terminator, trunking. The link control inside a voice frame, and the trunking
//! blocks inside a TSDU, are not unpacked (FEATURES §9): both need the frame's own interleaver
//! and, for the trunking blocks, the rate-1/2 trellis code, and neither can be verified here
//! against anything but itself.
//!
//! Status symbols complicate the framing: the transmitter inserts a dibit of its own after
//! every 35 payload dibits, starting at bit 70 of the frame, and they have to come out before
//! the NID is a codeword again.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, Fsk4Demod};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    P25Params,
};

use super::{INPUT_RATE_HZ, SymbolWindow};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const BANDWIDTH_HZ: f64 = 12_500.0;

/// Frame sync: 0x5575F5FF77FF, 48 bits.
const SYNC: u64 = 0x5575_F5FF_77FF;
const SYNC_BITS: u32 = 48;
const SYNC_TOLERANCE: u32 = 4;

/// First status symbol of a frame, and how often one follows (TIA-102.BAAA §7.1).
const STATUS_START: usize = 70;
const STATUS_STRIDE: usize = 72;

/// The NID is 64 bits, and the two status bits sitting inside it make 66 on the wire.
const NID_BITS: usize = 64;
const NID_SYMBOLS: usize = (NID_BITS + 2) / 2;

/// Data unit ids (TIA-102.BAAA §7.3).
const DUID_HEADER: u8 = 0x0;
const DUID_TERMINATOR: u8 = 0x3;
const DUID_LDU1: u8 = 0x5;
const DUID_TSDU: u8 = 0x7;
const DUID_LDU2: u8 = 0xA;
const DUID_PDU: u8 = 0xC;
const DUID_TERMINATOR_LC: u8 = 0xF;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "p25".to_owned(),
    name: "P25 Phase 1".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct P25Channel {
    demod: Fsk4Demod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&P25Params, ChannelError> {
    match &settings.params {
        ChannelParams::P25(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "p25 channel got {} params",
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

impl ChannelRx for P25Channel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            demod: Fsk4Demod::new(ctx.input_rate, BAUD, DEVIATION_HZ, RRC_ALPHA),
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
    countdown: usize,
    hunting: bool,
    bits: Vec<bool>,
    /// The data unit id last reported, so the six voice frames of a transmission produce one
    /// "call in progress" line rather than one every 180 ms.
    last_duid: Option<u8>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(NID_SYMBOLS),
            countdown: 0,
            hunting: true,
            bits: Vec::with_capacity(NID_BITS + 2),
            last_duid: None,
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.hunting = true;
        self.last_duid = None;
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                self.hunting = true;
                if let Some(frame) = self.nid() {
                    out.events.push(DecoderEvent::Dv(frame));
                }
            }
            return;
        }
        if self.hunting && self.window.sync_distance(SYNC, SYNC_BITS) <= SYNC_TOLERANCE {
            self.hunting = false;
            self.countdown = NID_SYMBOLS;
        }
    }

    /// Decode the network identifier that follows the sync.
    fn nid(&mut self) -> Option<DvFrame> {
        self.window.bits(0, NID_SYMBOLS, &mut self.bits);
        // The sync ended at frame bit 48, so the status dibit at frame bits 70 and 71 sits at
        // offset 22 of what follows.
        let mut word = 0u64;
        for (i, &bit) in self.bits.iter().enumerate() {
            let frame_bit = SYNC_BITS as usize + i;
            if frame_bit >= STATUS_START && (frame_bit - STATUS_START) % STATUS_STRIDE < 2 {
                continue;
            }
            word = word << 1 | u64::from(bit);
        }
        let (info, errors) = CyclicCode::BCH_63_16.decode(word)?;
        let nac = (info >> 4) as u16 & 0x0FFF;
        let duid = info as u8 & 0x0F;

        let kind = match duid {
            DUID_HEADER => DvFrameKind::Header,
            DUID_TERMINATOR | DUID_TERMINATOR_LC => DvFrameKind::Terminator,
            DUID_TSDU => DvFrameKind::Control,
            DUID_PDU => DvFrameKind::Data,
            _ => DvFrameKind::Voice,
        };
        // Voice frames repeat; everything else is an event in its own right.
        if kind == DvFrameKind::Voice && self.last_duid.is_some_and(is_voice) {
            self.last_duid = Some(duid);
            return None;
        }
        self.last_duid = Some(duid);

        let mut frame = DvFrame::new(DvMode::P25, kind);
        frame.color_code = Some(nac);
        frame.errors_corrected = errors;
        frame.opcode = Some(duid_name(duid).to_owned());
        Some(frame)
    }
}

fn is_voice(duid: u8) -> bool {
    matches!(duid, DUID_LDU1 | DUID_LDU2)
}

fn duid_name(duid: u8) -> &'static str {
    match duid {
        DUID_HEADER => "header",
        DUID_TERMINATOR => "terminator",
        DUID_LDU1 => "voice (LDU1)",
        DUID_TSDU => "trunking block",
        DUID_LDU2 => "voice (LDU2)",
        DUID_PDU => "packet data",
        DUID_TERMINATOR_LC => "terminator with link control",
        _ => "reserved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dv::testutil::decode, testgen::dv::p25 as tx, testutil::settings};

    fn channel() -> P25Channel {
        P25Channel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::P25(P25Params::default())),
        )
        .expect("p25 channel")
    }

    /// The status symbols the transmitter interleaves into the frame are what make this test
    /// worth having: leave them in and the network identifier is not a codeword at all.
    #[test]
    fn decodes_the_network_identifier_of_a_transmission() {
        let iq = tx::transmission(0x293, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.mode, DvMode::P25);
        assert_eq!(header.kind, DvFrameKind::Header);
        assert_eq!(header.color_code, Some(0x293));
        assert_eq!(header.errors_corrected, 0);
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Terminator),
            "no terminator: {frames:?}"
        );
        // Two voice frames, one line.
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
    fn decodes_a_trunking_block() {
        let iq = tx::trunking(0x4D2, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);
        let control = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Control)
            .expect("trunking block");
        assert_eq!(control.color_code, Some(0x4D2));
        assert_eq!(control.opcode.as_deref(), Some("trunking block"));
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(33, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
