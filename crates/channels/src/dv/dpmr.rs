//! dPMR decoder (ETSI TS 102 490): 4FSK at 2400 symbols per second in 6.25 kHz — the licence-
//! free 446 MHz digital radios and their commercial cousins.
//!
//! Four frame sync words separate the four things a transmission can be doing (§6.1), and the
//! header behind two of them carries the addressing in the clear:
//!
//! * **FS1** (48 bits) opens a voice or short-data header, twice over: two copies of the same
//!   72-bit header information either side of the colour code, so a receiver that misses one
//!   still has the other.
//! * **FS4** (48 bits) opens a packet-data header, laid out the same way.
//! * **FS3** (24 bits) marks the end frame. The superframe's own FS2 marker is not hunted
//!   for: 24 bits with no payload check behind them match noise several times a minute, and
//!   the header already says a call is running.
//!
//! Header information is CRC-8 checked, split into ten bytes, each carried by a shortened
//! Hamming(12,8), interleaved 12 × 10 and scrambled with `x⁹ + x⁵ + 1` (§7.7). The Hamming code
//! is systematic, so the information comes straight out of the first eight bits of each block —
//! this decoder detects errors with the CRC rather than correcting them with the Hamming
//! parity, which is what the doubled header is for.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::Fsk4Demod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DpmrParams, DvFrame,
    DvFrameKind, DvMode,
};

use super::{INPUT_RATE_HZ, SymbolWindow, bits_to_u32};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

/// 4800 bit/s over 2400 symbols, at half the deviation of the 12.5 kHz modes.
const BAUD: f64 = 2_400.0;
const DEVIATION_HZ: f64 = 1_050.0;
const RRC_ALPHA: f64 = 0.2;
const BANDWIDTH_HZ: f64 = 6_250.0;

/// ETSI TS 102 490 §6.1.
const FS1: u64 = 0x57FF_5F75_D577;
const FS4: u64 = 0xFD55_F5DF_7FDD;
const FS3: u64 = 0x7D_DFF5;
const LONG_SYNC_BITS: u32 = 48;
const SHORT_SYNC_BITS: u32 = 24;
const LONG_TOLERANCE: u32 = 4;
/// Half the tolerance for half the pattern: 24 bits at four errors is a pattern noise matches
/// several times a minute, and a superframe marker has no payload check behind it to catch that.
const SHORT_TOLERANCE: u32 = 2;

/// Header information: 72 bits under a CRC-8, ten Hamming(12,8) blocks on the wire.
const HI_BITS: usize = 72;
const HI_CODED_BITS: usize = 120;
const HI_BLOCKS: usize = 10;
const HI_SYMBOLS: usize = HI_CODED_BITS / 2;
/// The colour code between the two header copies: twelve bits, each sent as a di-bit (§6.1.5).
const CC_SYMBOLS: usize = 12;
/// Everything the header frame carries after FS1: HI0, the colour code, HI1.
const HEADER_SYMBOLS: usize = HI_SYMBOLS * 2 + CC_SYMBOLS;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dpmr".to_owned(),
    name: "dPMR".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct DpmrChannel {
    demod: Fsk4Demod,
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

/// Occupied RF band relative to the channel offset, in Hz.
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
    /// Set while a header's payload is arriving; carries whether it was a packet-data header.
    pending_packet: Option<bool>,
    bits: Vec<bool>,
    /// True while a superframe is being received, so its repeated sync marks nothing new.
    in_call: bool,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(HEADER_SYMBOLS),
            countdown: 0,
            pending_packet: None,
            bits: Vec::with_capacity(HEADER_SYMBOLS * 2),
            in_call: false,
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.pending_packet = None;
        self.in_call = false;
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0
                && let Some(packet) = self.pending_packet.take()
                && let Some(frame) = self.header(packet)
            {
                out.events.push(DecoderEvent::Dv(frame));
            }
            return;
        }
        if self.pending_packet.is_some() {
            return;
        }
        for (sync, packet) in [(FS1, false), (FS4, true)] {
            if self.window.sync_distance(sync, LONG_SYNC_BITS) <= LONG_TOLERANCE {
                self.pending_packet = Some(packet);
                self.countdown = HEADER_SYMBOLS;
                self.in_call = true;
                return;
            }
        }
        // Both superframe markers are only believed inside a call: nothing behind either of
        // them can be checked, and the header that opened the call is what vouches for them.
        // A call joined after its header has passed is therefore not reported (FEATURES §9).
        if !self.in_call {
            return;
        }
        if self.window.sync_distance(FS3, SHORT_SYNC_BITS) <= SHORT_TOLERANCE {
            self.in_call = false;
            out.events.push(DecoderEvent::Dv(DvFrame::new(
                DvMode::Dpmr,
                DvFrameKind::Terminator,
            )));
        }
    }

    /// The two header copies and the colour code between them.
    fn header(&mut self, packet: bool) -> Option<DvFrame> {
        self.window.bits(0, HEADER_SYMBOLS, &mut self.bits);
        let colour = colour_code(&self.bits[HI_CODED_BITS..HI_CODED_BITS + CC_SYMBOLS * 2]);
        // Either copy will do; the second exists because the first may not survive.
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
        // Communication mode 0 is an individual call; the rest are group and all-call modes.
        frame.group_call = Some(bits_to_u32(&hi, 52, 3) != 0);
        Some(frame)
    }
}

/// Twelve colour-code bits, each sent as the di-bit `01` for zero and `11` for one (§6.1.5).
/// Any other di-bit means the field did not survive, and a wrong colour code is worse than
/// none.
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

/// Undo scrambling, interleaving and the systematic Hamming blocks, and check the CRC-8.
fn header_info(coded: &[bool]) -> Option<Vec<bool>> {
    let mut descrambled = [false; HI_CODED_BITS];
    let mut register = 0x1FFu16;
    for (i, slot) in descrambled.iter_mut().enumerate() {
        let feedback = (register >> 8 ^ register >> 4) & 1;
        register = (register << 1 | feedback) & 0x1FF;
        *slot = coded[i] ^ (feedback == 1);
    }
    // The interleaver writes the ten 12-bit blocks down the columns of a 12 × 10 matrix and
    // reads the rows out, so the transmitted bit at row r, column c is block c bit r.
    let mut blocks = [false; HI_CODED_BITS];
    for r in 0..12 {
        for c in 0..HI_BLOCKS {
            blocks[c * 12 + r] = descrambled[r * HI_BLOCKS + c];
        }
    }
    // Hamming(12,8) is systematic: the byte is the first eight bits of its block.
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

/// CRC-8 with polynomial `x⁸ + x² + x + 1` (§7.2).
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
    use crate::{dv::testutil::decode, testgen::dv::dpmr as tx, testutil::settings};

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
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(69, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
