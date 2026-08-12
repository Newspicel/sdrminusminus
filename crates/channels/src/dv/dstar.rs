//! D-Star decoder: GMSK at 4800 bit/s in 6.25 kHz — the one two-level mode of the seven, so it
//! demodulates the way AIS does rather than through the four-level front end.
//!
//! A transmission opens with a header the receiver almost never gets to read: it is sent once,
//! convolutionally coded, interleaved and scrambled, and a receiver that joins late has missed
//! it. D-Star's answer is to send it again — every voice frame carries three bytes of slow
//! data, and the header is repeated through that channel over twenty frames. This decoder reads
//! the slow data, which means it recovers the callsigns of a call it joined in the middle, and
//! the text message that rides in the same channel.
//!
//! Slow data is scrambled with the fixed sequence 0x70 0x4F 0x93 and framed as a type nibble
//! and a length, in pairs of frames (six bytes at a time). The reassembled header is checked
//! against its own CRC-16 before any callsign reaches the log.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{BitSync, DcBlocker, FmDemod, RealDecimator, crc16_x25, hamming_distance};
use sdrmm_modem::pulse::{self, Norm};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DstarParams, DvFrame,
    DvFrameKind, DvMode,
};

use super::INPUT_RATE_HZ;
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const BAUD: f64 = 4_800.0;
/// GMSK at ±1200 Hz with a 0.5 bandwidth-time product, which is what an ICOM radio transmits.
const DEVIATION_HZ: f64 = 1_200.0;
const BT: f64 = 0.5;
const MATCHED_SPAN: usize = 3;
const BANDWIDTH_HZ: f64 = 6_250.0;

/// The 24-bit frame sync a transmission repeats every 21 voice frames: 0x552D16, sent low bit
/// first like every other byte in this mode.
const SYNC: u32 = 0x0055_2D16;
const SYNC_BITS: u32 = 24;
const SYNC_TOLERANCE: u32 = 2;

/// A voice frame: 72 bits of vocoder payload and 24 bits of data.
const FRAME_BITS: usize = 96;
const DATA_BITS: usize = 24;
/// Frames between one sync and the next.
const FRAMES_PER_SUPERFRAME: usize = 21;

/// The slow-data scrambling sequence, applied byte by byte.
const SCRAMBLER: [u8; 3] = [0x70, 0x4F, 0x93];

/// Slow-data packet types, in the high nibble of the first byte.
const TYPE_TEXT: u8 = 0x4;
const TYPE_HEADER: u8 = 0x5;

/// The header a transmission names itself with: flags, four callsigns, a suffix and a CRC.
const HEADER_BYTES: usize = 41;
const CALLSIGN_LEN: usize = 8;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dstar".to_owned(),
    name: "D-STAR".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct DstarChannel {
    demod: FmDemod,
    matched: RealDecimator,
    dc: DcBlocker,
    sync: BitSync,
    decoder: Decoder,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&DstarParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dstar(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dstar channel got {} params",
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

impl ChannelRx for DstarChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let sps = ctx.input_rate / BAUD;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, DEVIATION_HZ),
            // Area norm: the discriminator's level estimate relies on the filter's unit DC
            // gain, and the taps are `design_gaussian`'s output bit for bit.
            matched: RealDecimator::new(&pulse::gaussian(sps, BT, MATCHED_SPAN, Norm::Area), 1),
            dc: DcBlocker::new(),
            sync: BitSync::new(ctx.input_rate, BAUD),
            decoder: Decoder::new(),
            demod_buf: Vec::new(),
            filtered: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings)?;
        Ok(())
    }

    fn retuned(&mut self) {
        self.sync.reset();
        self.dc = DcBlocker::new();
        self.decoder.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        self.dc.process(&mut self.demod_buf);
        self.matched.process(&self.demod_buf, &mut self.filtered);
        for &sample in &self.filtered {
            if let Some(bit) = self.sync.push(sample) {
                self.decoder.push(bit, out);
            }
        }
    }
}

struct Decoder {
    register: u32,
    /// Bits into the current voice frame, once a sync has been found.
    bit: usize,
    /// Frames since the last sync; the sync itself is frame 0 of a superframe.
    frame: usize,
    synced: bool,
    /// The 24 data bits of the frame being received.
    data: u32,
    /// Slow data of the current frame pair; a packet spans two frames.
    packet: Vec<u8>,
    /// Header bytes reassembled from the slow-data channel.
    header: Vec<u8>,
    text: [u8; 20],
    /// The call last reported, so a header repeated every second is logged once.
    reported: Option<String>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            register: 0,
            bit: 0,
            frame: 0,
            synced: false,
            data: 0,
            packet: Vec::with_capacity(6),
            header: Vec::with_capacity(HEADER_BYTES),
            text: [b' '; 20],
            reported: None,
        }
    }

    fn reset(&mut self) {
        self.register = 0;
        self.synced = false;
        self.bit = 0;
        self.frame = 0;
        self.packet.clear();
        self.header.clear();
        self.reported = None;
    }

    fn push(&mut self, bit: bool, out: &mut ChannelOutputs) {
        self.register = self.register << 1 | u32::from(bit);
        if !self.synced {
            let mask = (1u32 << SYNC_BITS) - 1;
            if hamming_distance(u64::from(self.register & mask), u64::from(SYNC & mask))
                <= SYNC_TOLERANCE
            {
                self.synced = true;
                self.bit = 0;
                self.frame = 1;
                self.packet.clear();
            }
            return;
        }
        self.bit += 1;
        if self.bit > FRAME_BITS - DATA_BITS {
            self.data = self.data << 1 | u32::from(bit);
        }
        if self.bit < FRAME_BITS {
            return;
        }
        self.bit = 0;
        let frame = self.frame;
        self.frame += 1;
        if self.frame >= FRAMES_PER_SUPERFRAME {
            // The next frame is a sync frame; hunt for it rather than assuming it.
            self.synced = false;
        }
        // The sync frame's data field is the sync itself, not slow data.
        if !frame.is_multiple_of(FRAMES_PER_SUPERFRAME) {
            self.slow_data(frame, out);
        }
    }

    /// Three descrambled bytes per frame, two frames per packet.
    fn slow_data(&mut self, frame: usize, out: &mut ChannelOutputs) {
        for (i, mask) in SCRAMBLER.into_iter().enumerate() {
            self.packet.push((self.data >> (16 - i * 8)) as u8 ^ mask);
        }
        // Packets start on odd frames, so a complete one is six bytes gathered from a pair.
        if frame.is_multiple_of(2) && self.packet.len() >= 6 {
            let packet: Vec<u8> = self.packet.drain(..6).collect();
            self.packet.clear();
            self.packet_complete(&packet, out);
        } else if self.packet.len() > 6 {
            self.packet.clear();
        }
    }

    fn packet_complete(&mut self, packet: &[u8], out: &mut ChannelOutputs) {
        let kind = packet[0] >> 4;
        let length = usize::from(packet[0] & 0x0F).min(packet.len() - 1);
        match kind {
            TYPE_HEADER => {
                // Header segments arrive in order and restart at the first one; anything else
                // is a segment from a transmission this receiver did not hear the start of.
                if self.header.len() + length > HEADER_BYTES {
                    self.header.clear();
                }
                self.header.extend_from_slice(&packet[1..=length]);
                if self.header.len() == HEADER_BYTES
                    && let Some(frame) = self.header_frame()
                {
                    out.events.push(DecoderEvent::Dv(frame));
                }
            }
            TYPE_TEXT => {
                // Four characters per packet, at the offset the low nibble names.
                let slot = usize::from(packet[0] & 0x03) * 5;
                for (i, &byte) in packet[1..5].iter().enumerate() {
                    if slot + i < self.text.len() {
                        self.text[slot + i] = byte;
                    }
                }
            }
            _ => {}
        }
    }

    /// The reassembled 41-byte header, once its CRC agrees.
    fn header_frame(&mut self) -> Option<DvFrame> {
        let header = std::mem::take(&mut self.header);
        let (body, crc) = header.split_at(HEADER_BYTES - 2);
        if crc16_x25(body) != u16::from_le_bytes([crc[0], crc[1]]) {
            return None;
        }
        let call = |offset: usize, len: usize| {
            let text = String::from_utf8_lossy(&header[offset..offset + len])
                .trim()
                .to_owned();
            (!text.is_empty()).then_some(text)
        };
        let source = call(27, CALLSIGN_LEN);
        // The same call, sent every second for the length of the transmission.
        if self.reported == source && source.is_some() {
            return None;
        }
        self.reported.clone_from(&source);

        let mut frame = DvFrame::new(DvMode::Dstar, DvFrameKind::Header);
        frame.destination_call = call(19, CALLSIGN_LEN);
        frame.source_call = source;
        frame.via = call(3, CALLSIGN_LEN);
        frame.group_call = Some(frame.destination_call.as_deref() == Some("CQCQCQ"));
        let text = String::from_utf8_lossy(&self.text).trim().to_owned();
        frame.text = (!text.is_empty()).then_some(text);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dv::testutil::decode, testgen::dv::dstar as tx, testutil::settings};

    fn channel() -> DstarChannel {
        DstarChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Dstar(DstarParams::default())),
        )
        .expect("dstar channel")
    }

    /// The late-entry path: no header transmission is heard at all, and the callsigns come out
    /// of the slow-data channel that carries the header again while the call runs.
    #[test]
    fn decodes_the_callsigns_from_the_slow_data_channel() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.mode, DvMode::Dstar);
        assert_eq!(header.kind, DvFrameKind::Header);
        assert_eq!(header.source_call.as_deref(), Some(call.mycall.as_str()));
        assert_eq!(
            header.destination_call.as_deref(),
            Some(call.urcall.as_str())
        );
        assert_eq!(header.via.as_deref(), Some(call.repeater.as_str()));
        assert_eq!(header.group_call, Some(true));
    }

    /// The header repeats for as long as the transmission lasts; the log gets one line.
    #[test]
    fn a_repeated_header_is_reported_once() {
        let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
        assert_eq!(decode(&mut channel(), &iq).len(), 1);
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(81, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }
}
