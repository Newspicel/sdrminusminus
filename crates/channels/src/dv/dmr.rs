//! DMR Tier II/III decoder (ETSI TS 102 361-1): 4FSK at 4800 symbols per second in 12.5 kHz,
//! two 30 ms TDMA slots on one carrier.
//!
//! A burst is 264 bits: 108 payload, a 48-bit sync or embedded signalling field, 108 payload.
//! Which of the eight sync patterns matched says whether the burst is voice or data and whether
//! it came from a repeater, a radio, or a direct-mode radio naming its own slot. From there:
//!
//! * **Data bursts** carry a Golay(20,8) slot type split either side of the sync — colour code
//!   and data type — and 196 bits of BPTC(196,96) product code around it, which unpacks into a
//!   voice LC header, a terminator, a CSBK or a data header. The link control's Reed-Solomon
//!   (12,9) parity is masked per frame type, so verifying it also *confirms* the frame type.
//! * **Voice bursts** carry a QR(16,7,6) embedded signalling field instead of a sync, and
//!   bursts B to E of a superframe carry one quarter each of a BPTC(128,77) embedded link
//!   control. That is the late-entry path: a receiver that joins a call in progress learns who
//!   is talking within 240 ms rather than waiting for the next transmission.
//!
//! Only burst A of a voice superframe has a sync to find, so B to F are located by counting:
//! the superframe is six bursts one 60 ms TDMA frame apart, in the slot the sync arrived on.
//!
//! **Repeater slot numbering is not decoded.** A repeater names the slot in the CACH that
//! precedes each burst, which this does not read yet; direct-mode transmissions name their slot
//! in the sync pattern itself and are reported with it.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Bptc128, Bptc196, CyclicCode, crc16_msb, rs129_parity};
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DmrParams, DvFrame,
    DvFrameKind, DvMode,
};

use super::{INPUT_RATE_HZ, SymbolWindow, bits_to_u32, c4fm_demod, c4fm_params, pack_bytes};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const BAUD: f64 = 4_800.0;
/// Outer-symbol deviation of a 12.5 kHz C4FM transmitter (ETSI TS 102 361-1 §4.2.2).
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const BANDWIDTH_HZ: f64 = 12_500.0;

const BURST_BITS: usize = 264;
const BURST_SYMBOLS: usize = BURST_BITS / 2;
const SYNC_BITS: u32 = 48;
/// Bits of the burst before the sync field, and so symbols still to arrive when it matches.
const HALF_PAYLOAD_BITS: usize = 108;
const TRAILING_SYMBOLS: usize = HALF_PAYLOAD_BITS / 2;

/// Symbols in one 30 ms TDMA slot. The 132-symbol burst occupies 27.5 ms of it; the remaining
/// 2.5 ms is the CACH a repeater sends and the guard time a radio keys down for
/// (ETSI TS 102 361-1 §4.2.2), so a slot is *longer* than the burst it carries.
const SLOT_SYMBOLS: usize = 144;

/// One 60 ms TDMA cycle: this burst's slot, then the other's. Bursts B to F of a voice
/// superframe are found by counting these out from burst A rather than by a sync search.
const SUPERFRAME_STRIDE: usize = SLOT_SYMBOLS * 2;

/// Bit errors tolerated in a 48-bit sync. Four is under a tenth of the pattern and well inside
/// what its distance from the other seven allows.
const SYNC_TOLERANCE: u32 = 4;

/// Data types carried in the slot type (ETSI TS 102 361-1 §9.3.6).
const DT_PI_HEADER: u8 = 0x0;
const DT_VOICE_LC_HEADER: u8 = 0x1;
const DT_TERMINATOR_WITH_LC: u8 = 0x2;
const DT_CSBK: u8 = 0x3;
const DT_DATA_HEADER: u8 = 0x6;

/// CRC masks that make a frame type part of its own integrity check (§B.3.11).
const VOICE_LC_HEADER_MASK: [u8; 3] = [0x96, 0x96, 0x96];
const TERMINATOR_LC_MASK: [u8; 3] = [0x99, 0x99, 0x99];
const CSBK_MASK: u16 = 0xA5A5;
const DATA_HEADER_MASK: u16 = 0xCCCC;

/// Full link control: 72 bits of addressing plus 24 of Reed-Solomon parity.
const LC_BITS: usize = 72;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dmr".to_owned(),
    name: "DMR".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

/// A sync pattern and what matching it tells the decoder.
struct Sync {
    bits: u64,
    voice: bool,
    /// Direct-mode patterns name their timeslot; the repeater and radio ones do not, because
    /// there the slot lives in the CACH.
    slot: Option<u8>,
}

/// ETSI TS 102 361-1 §9.1.1, as the 48 bits each occupies.
const SYNCS: [Sync; 8] = [
    Sync {
        bits: 0x755F_D7DF_75F7,
        voice: true,
        slot: None,
    },
    Sync {
        bits: 0xDFF5_7D75_DF5D,
        voice: false,
        slot: None,
    },
    Sync {
        bits: 0x7F7D_5DD5_7DFD,
        voice: true,
        slot: None,
    },
    Sync {
        bits: 0xD5D7_F77F_D757,
        voice: false,
        slot: None,
    },
    Sync {
        bits: 0x5D57_7F77_57FF,
        voice: true,
        slot: Some(1),
    },
    Sync {
        bits: 0xF7FD_D5DD_FD55,
        voice: false,
        slot: Some(1),
    },
    Sync {
        bits: 0x7DFF_D5F5_5D5F,
        voice: true,
        slot: Some(2),
    },
    Sync {
        bits: 0xD755_7F5F_F7F5,
        voice: false,
        slot: Some(2),
    },
];

pub struct DmrChannel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&DmrParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dmr(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dmr channel got {} params",
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

impl ChannelRx for DmrChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = *params(&settings)?;
        Ok(Self {
            demod: c4fm_demod(&c4fm_params(ctx.input_rate, BAUD, DEVIATION_HZ, RRC_ALPHA)),
            symbols: Vec::new(),
            decoder: Decoder::new(p),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.decoder.params = *params(&settings)?;
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

/// What the decoder is waiting for.
#[derive(Clone, Copy)]
enum Pending {
    /// Nothing; hunting for a sync.
    None,
    /// A burst whose sync has been seen, waiting for its trailing payload.
    Burst { voice: bool, slot: Option<u8> },
    /// A voice burst located by counting from the superframe's burst A.
    VoiceFollower { slot: Option<u8> },
}

struct Decoder {
    params: DmrParams,
    window: SymbolWindow,
    pending: Pending,
    /// Symbols still to arrive before the pending burst is complete.
    countdown: usize,
    /// Voice bursts of the current superframe still expected (B to F).
    followers: u8,
    bits: Vec<bool>,
    bytes: Vec<u8>,
    /// Embedded link control fragments collected across bursts B to E.
    embedded: Vec<bool>,
    /// Bit errors repaired in the frames that built the embedded link control.
    embedded_errors: u32,
}

impl Decoder {
    fn new(params: DmrParams) -> Self {
        Self {
            params,
            window: SymbolWindow::new(BURST_SYMBOLS),
            pending: Pending::None,
            countdown: 0,
            followers: 0,
            bits: Vec::with_capacity(BURST_BITS),
            bytes: Vec::with_capacity(BURST_BITS / 8),
            embedded: Vec::with_capacity(Bptc128::CODED_BITS),
            embedded_errors: 0,
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.pending = Pending::None;
        self.countdown = 0;
        self.followers = 0;
        self.embedded.clear();
        self.embedded_errors = 0;
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown > 0 {
                return;
            }
            match std::mem::replace(&mut self.pending, Pending::None) {
                Pending::None => {}
                Pending::Burst { voice, slot } => self.burst(voice, slot, out),
                Pending::VoiceFollower { slot } => self.voice_burst(slot, out),
            }
            return;
        }
        self.hunt();
    }

    /// Look for a sync pattern ending at the symbol just pushed.
    fn hunt(&mut self) {
        for sync in &SYNCS {
            if self.window.sync_distance(sync.bits, SYNC_BITS) <= SYNC_TOLERANCE {
                self.window.anchor(sync.bits, SYNC_BITS);
                self.pending = Pending::Burst {
                    voice: sync.voice,
                    slot: sync.slot,
                };
                self.countdown = TRAILING_SYMBOLS;
                // A sync means the superframe restarted; whatever fragments were being
                // collected belong to a call that has ended.
                self.embedded.clear();
                self.embedded_errors = 0;
                return;
            }
        }
    }

    /// A burst whose last symbol has just arrived.
    fn burst(&mut self, voice: bool, slot: Option<u8>, out: &mut ChannelOutputs) {
        if voice {
            // Burst A of a voice superframe: no signalling of its own, but it anchors the
            // five that follow, which carry the embedded link control.
            self.followers = 5;
            self.schedule_follower(slot);
            return;
        }
        self.followers = 0;
        self.window.bits(0, BURST_SYMBOLS, &mut self.bits);
        let Some(frame) = self.data_burst(slot) else {
            return;
        };
        self.emit(frame, out);
    }

    fn schedule_follower(&mut self, slot: Option<u8>) {
        if self.followers == 0 {
            return;
        }
        self.followers -= 1;
        self.pending = Pending::VoiceFollower { slot };
        self.countdown = SUPERFRAME_STRIDE;
    }

    /// A voice burst B to F, located by counting rather than by a sync.
    fn voice_burst(&mut self, slot: Option<u8>, out: &mut ChannelOutputs) {
        self.window.bits(0, BURST_SYMBOLS, &mut self.bits);
        if let Some(frame) = self.embedded_signalling(slot) {
            self.emit(frame, out);
        }
        self.schedule_follower(slot);
    }

    fn emit(&self, frame: DvFrame, out: &mut ChannelOutputs) {
        if frame
            .slot
            .is_none_or(|slot| self.params.slots.accepts(slot))
        {
            out.events.push(DecoderEvent::Dv(frame));
        }
    }

    /// Slot type and BPTC payload of a data burst.
    fn data_burst(&mut self, slot: Option<u8>) -> Option<DvFrame> {
        let slot_type = u64::from(bits_to_u32(&self.bits, 98, 10)) << 10
            | u64::from(bits_to_u32(&self.bits, 156, 10));
        let (info, slot_errors) = CyclicCode::GOLAY_20_8.decode(slot_type)?;
        let colour = (info >> 4) as u16 & 0x0F;
        let data_type = info as u8 & 0x0F;

        let mut coded = [false; Bptc196::CODED_BITS];
        for (slot, &bit) in coded
            .iter_mut()
            .zip(self.bits[0..98].iter().chain(&self.bits[166..264]))
        {
            *slot = bit;
        }
        let (payload, payload_errors) = Bptc196::decode(&coded)?;
        let errors = slot_errors + payload_errors;

        let mut frame = match data_type {
            DT_VOICE_LC_HEADER => self.link_control(&payload, VOICE_LC_HEADER_MASK)?,
            DT_TERMINATOR_WITH_LC => {
                let mut frame = self.link_control(&payload, TERMINATOR_LC_MASK)?;
                frame.kind = DvFrameKind::Terminator;
                frame
            }
            DT_CSBK => self.csbk(&payload)?,
            DT_DATA_HEADER => {
                let mut frame = self.checked_block(&payload, DATA_HEADER_MASK)?;
                frame.kind = DvFrameKind::Data;
                frame
            }
            // A privacy indicator header says the payload that follows is encrypted, and
            // nothing else this decoder can read — its own fields are encrypted too.
            DT_PI_HEADER => {
                let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Header);
                frame.encrypted = Some(true);
                frame
            }
            // Rate 1/2, 3/4 and full-rate data blocks, MBC continuations and idle: framed
            // correctly, but nothing that names a call.
            _ => return None,
        };
        frame.slot = slot;
        frame.color_code = Some(colour);
        frame.errors_corrected = errors;
        Some(frame)
    }

    /// A 96-bit block whose last 16 bits are a mask-XORed CRC-16 over the rest.
    fn checked_block(&mut self, payload: &[bool; 96], mask: u16) -> Option<DvFrame> {
        pack_bytes(&payload[..80], &mut self.bytes);
        let expected = dmr_crc16(&self.bytes) ^ mask;
        let found = bits_to_u32(payload, 80, 16) as u16;
        (expected == found).then(|| DvFrame::new(DvMode::Dmr, DvFrameKind::Control))
    }

    /// A full link control: 72 bits of addressing under Reed-Solomon(12,9) parity, masked by
    /// frame type so a header cannot be read as a terminator.
    fn link_control(&mut self, payload: &[bool; 96], mask: [u8; 3]) -> Option<DvFrame> {
        pack_bytes(payload, &mut self.bytes);
        let (lc, parity) = self.bytes.split_at(LC_BITS / 8);
        let mut received = [parity[0], parity[1], parity[2]];
        for (byte, m) in received.iter_mut().zip(mask) {
            *byte ^= m;
        }
        if rs129_parity(lc) != received {
            return None;
        }
        Some(decode_lc(payload))
    }

    /// A control signalling block: opcode, feature set id, and the two addresses most of the
    /// opcodes carry in the same place.
    fn csbk(&mut self, payload: &[bool; 96]) -> Option<DvFrame> {
        let mut frame = self.checked_block(payload, CSBK_MASK)?;
        let opcode = bits_to_u32(payload, 2, 6) as u8;
        frame.opcode = Some(csbk_opcode_name(opcode).to_owned());
        // Preamble, and the call setup/teardown opcodes, all carry destination then source in
        // the last 48 bits of the block. The ones that do not are named but not read.
        if matches!(
            opcode,
            0x03 | 0x04 | 0x05 | 0x26 | 0x27 | 0x28 | 0x38 | 0x3D
        ) {
            frame.destination = Some(bits_to_u32(payload, 32, 24));
            frame.source = Some(bits_to_u32(payload, 56, 24));
        }
        Some(frame)
    }

    /// The embedded signalling field of a voice burst: colour code and the link control
    /// fragment index, and — once four fragments have arrived — the link control itself.
    fn embedded_signalling(&mut self, slot: Option<u8>) -> Option<DvFrame> {
        let emb = u64::from(bits_to_u32(&self.bits, 108, 8)) << 8
            | u64::from(bits_to_u32(&self.bits, 148, 8));
        let (info, errors) = CyclicCode::QR_16_7.decode(emb)?;
        let colour = (info >> 3) as u16 & 0x0F;
        let encrypted = info >> 2 & 1 == 1;
        let lcss = info & 0b11;

        // 00 is a single-fragment reverse-channel word this decoder has no use for; the other
        // three are the quarters of an embedded link control, in order.
        match lcss {
            0b01 => {
                self.embedded.clear();
                self.embedded_errors = 0;
            }
            // A continuation or last fragment with nothing before it belongs to a superframe
            // this receiver did not hear the start of.
            0b10 | 0b11 if !self.embedded.is_empty() => {}
            _ => return None,
        }
        self.embedded.extend(self.bits[116..148].iter().copied());
        self.embedded_errors += errors;

        if lcss != 0b10 {
            return None;
        }
        let coded: [bool; Bptc128::CODED_BITS] = self.embedded.as_slice().try_into().ok()?;
        let (data, bptc_errors) = Bptc128::decode(&coded)?;
        self.embedded.clear();

        // The 77 bits are the 72-bit link control with a five-bit checksum threaded through
        // column 10 of rows 2 to 6.
        let mut lc = [false; LC_BITS];
        let mut checksum = 0u32;
        let mut written = 0;
        for (i, &bit) in data.iter().enumerate() {
            if i >= 22 && i % 11 == 10 {
                checksum = checksum << 1 | u32::from(bit);
            } else {
                lc[written] = bit;
                written += 1;
            }
        }
        pack_bytes(&lc, &mut self.bytes);
        let sum = self.bytes.iter().map(|&b| u32::from(b)).sum::<u32>() % 31;
        if sum != checksum {
            return None;
        }
        let mut frame = decode_lc(&lc);
        frame.kind = DvFrameKind::Voice;
        frame.slot = slot;
        frame.color_code = Some(colour);
        frame.encrypted = Some(encrypted);
        frame.errors_corrected = self.embedded_errors + bptc_errors;
        Some(frame)
    }
}

/// The addressing a full link control carries, whichever path it arrived by (ETSI §9.1.6).
fn decode_lc(lc: &[bool]) -> DvFrame {
    let flco = bits_to_u32(lc, 2, 6);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Header);
    // FLCO 0 is a group call, 3 a call to one radio; the rest are talker alias and GPS
    // blocks, which carry no addresses of their own.
    match flco {
        0 | 3 => {
            frame.group_call = Some(flco == 0);
            frame.destination = Some(bits_to_u32(lc, 24, 24));
            frame.source = Some(bits_to_u32(lc, 48, 24));
            // Service options bit 6 is the encryption flag.
            frame.encrypted = Some(lc[17]);
        }
        _ => frame.opcode = Some(format!("FLCO {flco}")),
    }
    frame
}

fn csbk_opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x00 => "BS outbound activation",
        0x03 => "unit-to-unit voice service request",
        0x04 => "unit-to-unit voice service answer",
        0x05 => "channel timing",
        0x06 => "negative acknowledge response",
        0x07 => "call alert",
        0x08 => "call alert acknowledge",
        0x24 => "ALOHA",
        0x26 => "unit registration request",
        0x27 => "unit registration response",
        0x28 => "group voice channel grant",
        0x2A => "private voice channel grant",
        0x30 => "protect",
        0x38 => "preamble",
        0x3D => "talkgroup voice channel grant",
        _ => "CSBK",
    }
}

/// DMR's CRC-16 (ETSI TS 102 361-1 §B.3.10): the un-reflected CCITT register, initialised to
/// zero and inverted at the end, sent high byte first. Not the reflected X-25 variant the HDLC
/// modes use, and the difference is not detectable by inspection — only by a frame that fails.
fn dmr_crc16(data: &[u8]) -> u16 {
    !crc16_msb(0x1021, 0, data)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::DmrSlots;

    use super::*;
    use crate::{dv::testutil::decode, testgen::dv::dmr as tx, testutil::settings};

    fn channel(slots: DmrSlots) -> DmrChannel {
        DmrChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Dmr(DmrParams { slots })),
        )
        .expect("dmr channel")
    }

    /// A direct-mode call's repeated header, one voice superframe, and its terminator, keyed
    /// the way a TDMA radio keys: 132 symbols on the air in every 288, the rest dead.
    ///
    /// The embedded link control is asserted too — the late-entry path, four consecutive
    /// bursts of fragments with no burst-level code of their own. That is the part the
    /// sync-anchored level and centre estimates exist for: bursts B to E carry no sync, so
    /// they are sliced by what the syncs before them measured.
    #[test]
    fn decodes_a_call_from_header_to_terminator() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);

        let header = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Header)
            .expect("voice LC header");
        assert_eq!(header.mode, DvMode::Dmr);
        assert_eq!(header.slot, Some(1));
        assert_eq!(header.color_code, Some(u16::from(call.color_code)));
        assert_eq!(header.group_call, Some(true));
        assert_eq!(header.destination, Some(call.destination));
        assert_eq!(header.source, Some(call.source));

        // Every header a radio sends carries the same link control, so a decoder that framed
        // them all found them all. The bound is what the residual above costs: a handful of
        // bits the product code absorbs, not the tens a mis-framed burst would need.
        let headers: Vec<&DvFrame> = frames
            .iter()
            .filter(|f| f.kind == DvFrameKind::Header)
            .collect();
        assert_eq!(headers.len(), 3, "not every repeated header decoded");
        for header in headers {
            assert!(
                header.errors_corrected <= 4,
                "header needed {} corrections: {header:?}",
                header.errors_corrected
            );
        }

        let voice = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Voice)
            .expect("late entry: no embedded link control survived the superframe");
        assert_eq!(voice.destination, Some(call.destination));
        assert_eq!(voice.source, Some(call.source));
        assert_eq!(voice.color_code, Some(u16::from(call.color_code)));

        let terminator = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Terminator)
            .expect("terminator with link control");
        assert_eq!(terminator.destination, Some(call.destination));
        assert_eq!(terminator.source, Some(call.source));
        assert_eq!(terminator.color_code, Some(u16::from(call.color_code)));
    }

    /// Off the air, which is the only place the front end meets a real TDMA transmitter: a
    /// direct-mode call on PMR446 channel 1, recorded with an RTL-SDR at 2.048 Msps and
    /// down-converted to the channel rate (fixtures/dmr_call_48k). The radio keys off for half
    /// of every 60 ms frame, so between bursts the receiver hears nothing but its own noise —
    /// and the clock, centre and level estimates have to arrive at each burst holding what the
    /// transmitter taught them rather than what the dead time did.
    ///
    /// The late-entry path is what this proves: the addressing here is recovered from the voice
    /// superframes alone, four bursts of embedded link control at a time, with no header in
    /// them at all.
    #[test]
    fn decodes_a_recorded_call() {
        const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/dmr_call_48k.sigmf-data");
        let iq: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                Complex::new(
                    f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
                )
            })
            .collect();
        let mut chan = channel(DmrSlots::Both);
        let mut filter = channel_filter();
        let mut out = ChannelOutputs::default();
        let mut filtered = Vec::new();
        let mut frames = Vec::new();
        for block in iq.chunks(997) {
            filter.process(block, &mut filtered);
            out.reset();
            chan.process(&filtered, &mut out);
            for event in out.events.drain(..) {
                let DecoderEvent::Dv(frame) = event else {
                    panic!("unexpected event")
                };
                frames.push(frame);
            }
        }

        let calls: Vec<&DvFrame> = frames
            .iter()
            .filter(|f| f.kind == DvFrameKind::Header || f.kind == DvFrameKind::Voice)
            .collect();
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Header),
            "no voice LC header: {frames:?}"
        );
        assert!(
            frames
                .iter()
                .filter(|f| f.kind == DvFrameKind::Voice)
                .count()
                >= 3,
            "late entry recovered fewer than three superframes: {frames:?}"
        );
        for frame in calls {
            assert_eq!(frame.color_code, Some(1));
            assert_eq!(frame.group_call, Some(true));
            assert_eq!(frame.source, Some(12_345_678));
            assert_eq!(frame.destination, Some(12_345_678));
        }
    }

    #[test]
    fn decodes_a_private_call_header() {
        let call = tx::Call {
            group: false,
            destination: 2_621_002,
            ..tx::Call::default()
        };
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let header = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Header)
            .expect("voice LC header");
        assert_eq!(header.group_call, Some(false));
        assert_eq!(header.destination, Some(call.destination));
    }

    #[test]
    fn decodes_a_csbk() {
        let iq = tx::csbk(3, 0x38, 505, 2_621_001, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let csbk = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Control)
            .expect("csbk");
        assert_eq!(csbk.color_code, Some(3));
        assert_eq!(csbk.opcode.as_deref(), Some("preamble"));
        assert_eq!(csbk.destination, Some(505));
        assert_eq!(csbk.source, Some(2_621_001));
    }

    /// The slot filter drops what the operator asked not to see, and the generator transmits
    /// slot 1 only.
    #[test]
    fn the_slot_filter_selects_what_is_reported() {
        let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
        assert!(!decode(&mut channel(DmrSlots::One), &iq).is_empty());
        assert!(decode(&mut channel(DmrSlots::Two), &iq).is_empty());
    }

    /// Noise is not a call. Nothing may reach the log from a channel carrying only noise, at
    /// any level — the product code and the Reed-Solomon parity exist to make that true.
    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(9, 0.5, 400_000);
        assert!(decode(&mut channel(DmrSlots::Both), &noise).is_empty());
    }

    /// A retune abandons the transmitter the decoder was following: whatever it had collected
    /// describes a call on the frequency it just left.
    #[test]
    fn retuning_forgets_the_call_in_progress() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let mut chan = channel(DmrSlots::Both);
        let mut out = ChannelOutputs::default();
        // Half a transmission, then a retune, then the rest: the embedded link control that
        // spans the cut must not be assembled out of both halves.
        chan.process(&iq[..iq.len() / 2], &mut out);
        chan.retuned();
        out.reset();
        chan.process(&iq[iq.len() / 2..], &mut out);
        let frames: Vec<&DvFrame> = out
            .events
            .iter()
            .map(|event| {
                let DecoderEvent::Dv(frame) = event else {
                    panic!("unexpected event")
                };
                frame
            })
            .collect();
        assert!(
            frames.iter().all(|f| f.kind != DvFrameKind::Voice),
            "an embedded link control was assembled across the retune: {frames:?}"
        );
    }
}
