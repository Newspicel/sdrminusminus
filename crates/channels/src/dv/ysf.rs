//! System Fusion (YSF) decoder: C4FM at 4800 symbols per second in 12.5 kHz, 100 ms frames.
//!
//! Every frame opens with the same 40-bit sync and a 100-symbol frame information channel. The
//! FICH is the part worth decoding without a vocoder: it says whether the frame is a header, a
//! communication frame or a terminator, which of the four data modes it carries, and where it
//! sits in the transmission — so a log gets one line when a call starts and one when it ends,
//! rather than ten a second.
//!
//! The FICH is protected three times over: a rate-1/2 convolutional code, four Golay(24,12,8)
//! blocks over its 48 bits, and a CRC-16 across the result. All three have to agree, which is
//! what makes it safe to report a frame from a mode that carries no addresses in the clear.
//!
//! Callsigns travel in the data channel alongside the vocoder frames and are not decoded here:
//! they need the payload de-interleaver and the same Viterbi pass per sub-block, which is
//! follow-up work (FEATURES §9).

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, Fsk4Demod, Viterbi5, crc16_msb};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    YsfParams,
};

use super::{INPUT_RATE_HZ, SymbolWindow};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const BANDWIDTH_HZ: f64 = 12_500.0;

/// The sync every frame opens with: 0xD471C9634D, 40 bits.
const SYNC: u64 = 0x00D4_71C9_634D;
const SYNC_BITS: u32 = 40;
const SYNC_TOLERANCE: u32 = 3;

/// The FICH occupies the 100 symbols after the sync.
const FICH_SYMBOLS: usize = 100;
/// Its 200 coded bits carry 96 after the convolutional code, and 48 after the Golay blocks.
const FICH_CODED_BITS: usize = 200;
const FICH_INFO_BITS: usize = 96;
const FICH_BYTES: usize = 6;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ysf".to_owned(),
    name: "System Fusion".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct YsfChannel {
    demod: Fsk4Demod,
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
    viterbi: Viterbi5,
    /// Symbols still to arrive before the FICH of a matched frame is complete.
    countdown: usize,
    hunting: bool,
    soft: Vec<i16>,
    coded: Vec<i16>,
    info: Vec<bool>,
    /// Frame type last reported, so a 100 ms heartbeat does not become a log entry per frame.
    last_kind: Option<DvFrameKind>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(FICH_SYMBOLS),
            viterbi: Viterbi5::new(),
            countdown: 0,
            hunting: true,
            soft: Vec::with_capacity(FICH_CODED_BITS),
            coded: Vec::with_capacity(FICH_CODED_BITS),
            info: Vec::with_capacity(FICH_INFO_BITS),
            last_kind: None,
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.countdown = 0;
        self.hunting = true;
        self.last_kind = None;
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0 {
                self.hunting = true;
                if let Some(frame) = self.fich() {
                    out.events.push(DecoderEvent::Dv(frame));
                }
            }
            return;
        }
        if self.hunting && self.window.sync_distance(SYNC, SYNC_BITS) <= SYNC_TOLERANCE {
            self.window.anchor(SYNC, SYNC_BITS);
            self.hunting = false;
            self.countdown = FICH_SYMBOLS;
        }
    }

    /// Decode the FICH sitting in the last 100 symbols.
    fn fich(&mut self) -> Option<DvFrame> {
        self.window.soft_bits(0, FICH_SYMBOLS, &mut self.soft);
        // De-interleave: the FICH is written into a 20 × 5 matrix by column and read by row,
        // so coded pair `i` comes from bit 2·(i/5) + 40·(i%5).
        self.coded.clear();
        for i in 0..FICH_SYMBOLS {
            let n = 2 * (i / 5) + 40 * (i % 5);
            self.coded.push(self.soft[n]);
            self.coded.push(self.soft[n + 1]);
        }
        // 100 steps in, 96 information bits out: the last four are the encoder's flush.
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
        // CRC-16 over the four information bytes, sent high byte first.
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
        // A communication frame arrives ten times a second and says the same thing each time;
        // only a change of frame type is news.
        if kind == DvFrameKind::Voice && self.last_kind == Some(DvFrameKind::Voice) {
            return None;
        }
        self.last_kind = Some(kind);

        let mut frame = DvFrame::new(DvMode::Ysf, kind);
        frame.errors_corrected = errors;
        frame.group_call = Some(fich[0] >> 2 & 0x03 != 0x03);
        frame.opcode = Some(data_mode_name(fich[2] & 0x03).to_owned());
        let dg_id = fich[3] & 0x7F;
        if dg_id != 0 {
            frame.destination = Some(u32::from(dg_id));
        }
        Some(frame)
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
    use crate::{dv::testutil::decode, testgen::dv::ysf as tx, testutil::settings};

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
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(21, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
