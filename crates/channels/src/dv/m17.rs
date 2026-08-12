//! M17 decoder (M17 specification, 2024): C4FM at 4800 symbols per second, root-raised-cosine
//! shaped at 0.5, in 9 kHz.
//!
//! The one open mode of the seven, and the only one whose call setup is fully readable: a link
//! setup frame carries both callsigns in the clear, base-40 encoded into 48 bits each, under a
//! convolutional code and a CRC. This decodes them.
//!
//! A frame is a 16-bit sync burst and 368 payload bits. Which sync arrived says what the frame
//! is — link setup, stream, packet, or the end-of-transmission marker. The link setup payload
//! is undone in the order the transmitter applied it: derandomise with the specification's
//! 46-byte sequence, deinterleave by the quadratic permutation, restore the bits the puncturing
//! pattern removed, then Viterbi-decode and check the CRC.
//!
//! Stream frames also rebuild the link setup from their six Golay-protected LICH fragments for
//! late entry, undo P2 puncturing, and decode either two Codec2 3200 voice blocks or the Codec2
//! 1600 half of a voice+data block. Encrypted stream types are reported and emitted as silence.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    CyclicCode, Viterbi5, crc16_msb,
    fec::conv::{ERASURE, Soft},
};
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    M17Params,
};

use super::{INPUT_RATE_HZ, SymbolWindow, c4fm_demod, c4fm_params, vocoder::Codec2Decoder};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const BAUD: f64 = 4_800.0;
/// M17 deviates ±2400 Hz on its outer symbols and shapes with α = 0.5.
const DEVIATION_HZ: f64 = 2_400.0;
const RRC_ALPHA: f64 = 0.5;
const BANDWIDTH_HZ: f64 = 9_000.0;

/// The four sync bursts, 16 bits each (M17 spec §2.5).
const SYNC_LSF: u64 = 0x55F7;
const SYNC_STREAM: u64 = 0xFF5D;
const SYNC_PACKET: u64 = 0x75FF;
const SYNC_EOT: u64 = 0x555D;
const SYNC_BITS: u32 = 16;
/// Two bit errors in a 16-bit burst; the four bursts are far enough apart for that.
const SYNC_TOLERANCE: u32 = 2;

/// Payload of every frame: 184 symbols, 368 bits.
const PAYLOAD_SYMBOLS: usize = 184;
const PAYLOAD_BITS: usize = 368;
/// The link setup frame before puncturing: 240 information bits plus four flush bits, at rate
/// 1/2.
const LSF_CODED_BITS: usize = 488;
const LSF_BITS: usize = 240;
const LSF_BYTES: usize = LSF_BITS / 8;
const STREAM_CODED_BITS: usize = 296;
const STREAM_BITS: usize = 144;

/// P1, the puncturing pattern a link setup frame is thinned with: 46 of every 61 coded bits are
/// transmitted, which is exactly 368 of 488.
const PUNCTURE_1: [bool; 61] = {
    let mut pattern = [true; 61];
    let mut i = 2;
    while i < 61 {
        pattern[i] = false;
        i += 4;
    }
    pattern
};

/// The randomiser: 368 bits of the specification's fixed sequence, XORed over the payload so a
/// long run of one symbol never reaches the air.
const RANDOMIZER: [u8; 46] = [
    0xD6, 0xB5, 0xE2, 0x30, 0x82, 0xFF, 0x84, 0x62, 0xBA, 0x4E, 0x96, 0x90, 0xD8, 0x98, 0xDD, 0x5D,
    0x0C, 0xC8, 0x52, 0x43, 0x91, 0x1D, 0xF8, 0x6E, 0x68, 0x2F, 0x35, 0xDA, 0x14, 0xEA, 0xCD, 0x76,
    0x19, 0x8D, 0xD5, 0x80, 0xD1, 0x33, 0x87, 0x13, 0x57, 0x18, 0x2D, 0x29, 0x78, 0xC3,
];

/// M17's CRC: polynomial 0x5935 with an all-ones register.
const CRC_POLY: u16 = 0x5935;
const CRC_INIT: u16 = 0xFFFF;

/// The interleaver, as the quadratic permutation the specification defines rather than the
/// table it prints: bit `i` of the interleaved payload is bit `(45·i + 92·i²) mod 368`.
fn interleave_index(i: usize) -> usize {
    (45 * i + 92 * i * i) % PAYLOAD_BITS
}

/// Callsign alphabet: index 0 is the space that pads a short callsign (M17 spec §2.3.1).
const CALLSIGN_ALPHABET: &[u8; 40] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-/.";

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "m17".to_owned(),
    name: "M17".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct M17Channel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&M17Params, ChannelError> {
    match &settings.params {
        ChannelParams::M17(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "m17 channel got {} params",
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

impl ChannelRx for M17Channel {
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

/// What the sync burst said the frame is.
#[derive(Clone, Copy, PartialEq)]
enum FrameType {
    LinkSetup,
    Stream,
    Packet,
}

struct Decoder {
    window: SymbolWindow,
    viterbi: Viterbi5,
    pending: Option<FrameType>,
    countdown: usize,
    soft: Vec<Soft>,
    coded: Vec<Soft>,
    info: Vec<bool>,
    /// A stream carries its link setup once and then repeats it in fragments; reporting the
    /// same call every 40 ms would drown the log.
    in_stream: bool,
    stream_type: Option<u16>,
    /// Six late-entry LICH chunks rebuild the LSF for a receiver joining mid-call.
    late_lsf: [bool; LSF_BITS],
    late_chunks: u8,
    stream_coded: Vec<Soft>,
    stream_info: Vec<bool>,
    vocoder: Codec2Decoder,
}

impl Decoder {
    fn new() -> Self {
        Self {
            window: SymbolWindow::new(PAYLOAD_SYMBOLS),
            viterbi: Viterbi5::new(),
            pending: None,
            countdown: 0,
            soft: Vec::with_capacity(PAYLOAD_BITS),
            coded: Vec::with_capacity(LSF_CODED_BITS),
            info: Vec::with_capacity(LSF_BITS + 4),
            in_stream: false,
            stream_type: None,
            late_lsf: [false; LSF_BITS],
            late_chunks: 0,
            stream_coded: Vec::with_capacity(STREAM_CODED_BITS),
            stream_info: Vec::with_capacity(STREAM_BITS + 4),
            vocoder: Codec2Decoder::new(),
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.pending = None;
        self.countdown = 0;
        self.in_stream = false;
        self.stream_type = None;
        self.late_chunks = 0;
        self.vocoder.reset();
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown == 0
                && let Some(kind) = self.pending.take()
                && let Some(frame) = self.payload(kind, out)
            {
                out.events.push(DecoderEvent::Dv(frame));
            }
            return;
        }
        if self.pending.is_some() {
            return;
        }
        // The end-of-transmission marker is a sync burst with no payload behind it, so the
        // only thing vouching for it is the call it ends: without one it is indistinguishable
        // from the sixteen bits of noise that match it several times a second.
        if self.in_stream && self.window.sync_distance(SYNC_EOT, SYNC_BITS) <= SYNC_TOLERANCE {
            self.window.anchor(SYNC_EOT, SYNC_BITS);
            self.in_stream = false;
            self.stream_type = None;
            self.late_chunks = 0;
            out.events.push(DecoderEvent::Dv(DvFrame::new(
                DvMode::M17,
                DvFrameKind::Terminator,
            )));
            return;
        }
        for (sync, kind) in [
            (SYNC_LSF, FrameType::LinkSetup),
            (SYNC_STREAM, FrameType::Stream),
            (SYNC_PACKET, FrameType::Packet),
        ] {
            if self.window.sync_distance(sync, SYNC_BITS) <= SYNC_TOLERANCE {
                self.window.anchor(sync, SYNC_BITS);
                self.pending = Some(kind);
                self.countdown = PAYLOAD_SYMBOLS;
                return;
            }
        }
    }

    fn payload(&mut self, kind: FrameType, out: &mut ChannelOutputs) -> Option<DvFrame> {
        match kind {
            FrameType::LinkSetup => {
                let frame = self.link_setup();
                // A frame that failed its CRC says nothing — including nothing about whether
                // the call this decoder was following has ended. Sixteen bits of sync match
                // noise often enough that treating a failure as an end would lose the call.
                self.in_stream |= frame.is_some();
                frame
            }
            // A stream frame carries both a late-entry LSF fragment and Codec2 audio.
            FrameType::Stream => self.stream(out),
            FrameType::Packet => self
                .in_stream
                .then(|| DvFrame::new(DvMode::M17, DvFrameKind::Data)),
        }
    }

    /// Undo the transmitter's payload chain and read the link setup out of it.
    fn link_setup(&mut self) -> Option<DvFrame> {
        self.window.soft_bits(0, PAYLOAD_SYMBOLS, &mut self.soft);
        // Derandomise: the sequence flips bits, which for a soft value is a sign change.
        for (i, value) in self.soft.iter_mut().enumerate() {
            if RANDOMIZER[i / 8] >> (7 - i % 8) & 1 == 1 {
                *value = -*value;
            }
        }
        // Deinterleave, then put an erasure back wherever the puncturing pattern removed a
        // coded bit — a position the decoder must weigh as evidence for neither branch.
        let mut deinterleaved = [ERASURE; PAYLOAD_BITS];
        for (i, &value) in self.soft.iter().enumerate() {
            deinterleaved[interleave_index(i)] = value;
        }
        self.coded.clear();
        let mut read = 0;
        for i in 0..LSF_CODED_BITS {
            if PUNCTURE_1[i % PUNCTURE_1.len()] {
                self.coded.push(deinterleaved[read]);
                read += 1;
            } else {
                self.coded.push(ERASURE);
            }
        }
        self.info.clear();
        self.viterbi.decode(&self.coded, &mut self.info);

        let mut lsf = [0u8; LSF_BYTES];
        for (i, byte) in lsf.iter_mut().enumerate() {
            *byte = self.info[i * 8..i * 8 + 8]
                .iter()
                .fold(0u8, |acc, &b| acc << 1 | u8::from(b));
        }
        // The CRC covers the whole frame including its own two bytes, so a good frame leaves a
        // zero register.
        if crc16_msb(CRC_POLY, CRC_INIT, &lsf) != 0 {
            return None;
        }

        self.stream_type = Some(u16::from_be_bytes([lsf[12], lsf[13]]));
        frame_from_lsf(&lsf)
    }

    fn stream(&mut self, out: &mut ChannelOutputs) -> Option<DvFrame> {
        self.window.soft_bits(0, PAYLOAD_SYMBOLS, &mut self.soft);
        for (i, value) in self.soft.iter_mut().enumerate() {
            if RANDOMIZER[i / 8] >> (7 - i % 8) & 1 == 1 {
                *value = -*value;
            }
        }
        let mut deinterleaved = [ERASURE; PAYLOAD_BITS];
        for (i, &value) in self.soft.iter().enumerate() {
            deinterleaved[interleave_index(i)] = value;
        }

        let late = self.late_entry(&deinterleaved[..96]);

        // Stream contents are a 296-bit rate-1/2 code punctured by deleting every twelfth
        // coded bit. Restore those positions as erasures before Viterbi decoding.
        self.stream_coded.clear();
        let mut read = 96;
        for i in 0..STREAM_CODED_BITS {
            if i % 12 == 11 {
                self.stream_coded.push(ERASURE);
            } else {
                self.stream_coded.push(deinterleaved[read]);
                read += 1;
            }
        }
        self.stream_info.clear();
        self.viterbi
            .decode(&self.stream_coded, &mut self.stream_info);
        if self.stream_info.len() < STREAM_BITS {
            return late;
        }
        let mut payload = [0u8; 16];
        for (i, &bit) in self.stream_info[16..STREAM_BITS].iter().enumerate() {
            payload[i / 8] |= u8::from(bit) << (7 - i % 8);
        }
        if let Some(stream_type) = self.stream_type {
            let encrypted = stream_type >> 3 & 0b11 != 0;
            match stream_type >> 1 & 0b11 {
                0b10 => self.vocoder.decode_3200(&payload, encrypted, out),
                0b11 => self.vocoder.decode_1600(&payload, encrypted, out),
                _ => {}
            }
        }
        late
    }

    fn late_entry(&mut self, coded: &[Soft]) -> Option<DvFrame> {
        let mut decoded = [false; 48];
        for block in 0..4 {
            let word = coded[block * 24..(block + 1) * 24]
                .iter()
                .fold(0u64, |acc, &bit| acc << 1 | u64::from(bit > 0));
            let (info, _) = CyclicCode::GOLAY_24_12.decode(word)?;
            for bit in 0..12 {
                decoded[block * 12 + bit] = info >> (11 - bit) & 1 == 1;
            }
        }
        let chunk = decoded[40..43]
            .iter()
            .fold(0u8, |acc, &bit| acc << 1 | u8::from(bit));
        if chunk >= 6 {
            return None;
        }
        self.late_lsf[usize::from(chunk) * 40..usize::from(chunk + 1) * 40]
            .copy_from_slice(&decoded[..40]);
        self.late_chunks |= 1 << chunk;
        if self.late_chunks != 0x3F {
            return None;
        }
        self.late_chunks = 0;
        let mut lsf = [0u8; LSF_BYTES];
        for (i, &bit) in self.late_lsf.iter().enumerate() {
            lsf[i / 8] |= u8::from(bit) << (7 - i % 8);
        }
        if crc16_msb(CRC_POLY, CRC_INIT, &lsf) != 0 {
            return None;
        }
        self.stream_type = Some(u16::from_be_bytes([lsf[12], lsf[13]]));
        let joined_late = !self.in_stream;
        self.in_stream = true;
        if joined_late {
            frame_from_lsf(&lsf)
        } else {
            None
        }
    }
}

fn frame_from_lsf(lsf: &[u8; LSF_BYTES]) -> Option<DvFrame> {
    let mut frame = DvFrame::new(DvMode::M17, DvFrameKind::Header);
    frame.destination_call = callsign(u64::from_be_bytes([
        0, 0, lsf[0], lsf[1], lsf[2], lsf[3], lsf[4], lsf[5],
    ]));
    frame.source_call = callsign(u64::from_be_bytes([
        0, 0, lsf[6], lsf[7], lsf[8], lsf[9], lsf[10], lsf[11],
    ]));
    let stream_type = u16::from_be_bytes([lsf[12], lsf[13]]);
    frame.group_call = Some(frame.destination_call.as_deref() == Some("ALL"));
    frame.encrypted = Some(stream_type >> 3 & 0b11 != 0);
    frame.opcode = Some(
        if stream_type & 1 == 1 {
            "stream"
        } else {
            "packet"
        }
        .to_owned(),
    );
    Some(frame)
}

/// Base-40 callsign decoding (M17 spec §2.3.1). `0xFFFFFFFFFF` is the broadcast address, which
/// every radio answers to and which no base-40 value can collide with.
fn callsign(address: u64) -> Option<String> {
    if address == 0 {
        return None;
    }
    if address == 0xFFFF_FFFF_FFFF {
        return Some("ALL".to_owned());
    }
    if address > 40u64.pow(9) {
        return None;
    }
    let mut value = address;
    let mut call = Vec::new();
    while value > 0 {
        call.push(CALLSIGN_ALPHABET[(value % 40) as usize]);
        value /= 40;
    }
    String::from_utf8(call).ok().map(|s| s.trim().to_owned())
}

/// Base-40 encoding of a callsign: the inverse of [`callsign`], which is what makes the
/// round-trip test below a test of the decoding and not of a shared table.
#[cfg(test)]
fn encode_callsign(call: &str) -> u64 {
    if call == "ALL" {
        return 0xFFFF_FFFF_FFFF;
    }
    call.bytes()
        .rev()
        .filter_map(|c| CALLSIGN_ALPHABET.iter().position(|&a| a == c))
        .fold(0u64, |acc, index| acc * 40 + index as u64)
}

#[cfg(test)]
mod tests {
    use codec2::{Codec2, Codec2Mode};

    use super::*;
    use crate::{
        dv::testutil::{assert_tone_audio, decode, decode_with_audio},
        testgen::dv::m17 as tx,
        testutil::settings,
    };

    fn channel() -> M17Channel {
        M17Channel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::M17(M17Params::default())),
        )
        .expect("m17 channel")
    }

    /// The mode that can say who is talking without a vocoder, saying it.
    #[test]
    fn decodes_the_callsigns_of_a_link_setup_frame() {
        let iq = tx::transmission("ALL", "DL1ABC", INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let lsf = frames.first().expect("a decoded frame");
        assert_eq!(lsf.mode, DvMode::M17);
        assert_eq!(lsf.kind, DvFrameKind::Header);
        assert_eq!(lsf.source_call.as_deref(), Some("DL1ABC"));
        assert_eq!(lsf.destination_call.as_deref(), Some("ALL"));
        assert_eq!(lsf.group_call, Some(true));
        assert_eq!(lsf.encrypted, Some(false));
        assert_eq!(lsf.opcode.as_deref(), Some("stream"));
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Terminator),
            "no end of transmission: {frames:?}"
        );
    }

    #[test]
    fn decodes_codec2_stream_audio() {
        let mut encoder = Codec2::new(Codec2Mode::MODE_3200);
        let mut voice = [[0u8; 16]; 8];
        for (radio_frame, payload) in voice.iter_mut().enumerate() {
            for codec_frame in 0..2 {
                let frame = radio_frame * 2 + codec_frame;
                let pcm: [i16; 160] = std::array::from_fn(|i| {
                    let sample = frame * 160 + i;
                    (12_000.0 * (std::f64::consts::TAU * 440.0 * sample as f64 / 8_000.0).sin())
                        as i16
                });
                encoder.encode(&mut payload[codec_frame * 8..codec_frame * 8 + 8], &pcm);
            }
        }
        let iq = tx::transmission_with_voice("ALL", "DL1ABC", &voice, INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        // The front end acquires the stream cadence on the first two short sync bursts; the
        // remaining six frames prove twelve independently framed Codec2 blocks end to end.
        assert_tone_audio(&audio, 12);
    }

    #[test]
    fn decodes_codec2_voice_and_data_audio() {
        let mut encoder = Codec2::new(Codec2Mode::MODE_1600);
        let mut payloads = [[0u8; 16]; 8];
        for (frame, payload) in payloads.iter_mut().enumerate() {
            let pcm: [i16; 320] = std::array::from_fn(|i| {
                let sample = frame * 320 + i;
                (12_000.0 * (std::f64::consts::TAU * 440.0 * sample as f64 / 8_000.0).sin()) as i16
            });
            encoder.encode(&mut payload[..8], &pcm);
            payload[8..].fill(frame as u8);
        }
        let iq = tx::transmission_with_voice_data("ALL", "DL1ABC", &payloads, INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert_tone_audio(&audio, 16);
    }

    #[test]
    fn joins_a_stream_from_lich_late_entry() {
        let iq = tx::late_entry("ALL", "DL1ABC", INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);
        let header = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Header)
            .expect("late-entry link setup");
        assert_eq!(header.source_call.as_deref(), Some("DL1ABC"));
        assert_eq!(header.destination_call.as_deref(), Some("ALL"));
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.kind == DvFrameKind::Header)
                .count(),
            1
        );
    }

    #[test]
    fn decodes_a_call_between_two_stations() {
        let iq = tx::transmission("DL9XYZ", "M0ABC", INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);
        let lsf = frames.first().expect("a decoded frame");
        assert_eq!(lsf.destination_call.as_deref(), Some("DL9XYZ"));
        assert_eq!(lsf.source_call.as_deref(), Some("M0ABC"));
        assert_eq!(lsf.group_call, Some(false));
    }

    #[test]
    fn callsigns_round_trip_through_base_40() {
        for call in ["DL1ABC", "M0ABC", "SP5WWP", "N0CALL", "ALL"] {
            assert_eq!(callsign(encode_callsign(call)).as_deref(), Some(call));
        }
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(45, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
