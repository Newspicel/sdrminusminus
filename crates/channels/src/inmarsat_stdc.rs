use std::{ops::RangeInclusive, sync::LazyLock};

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, InmarsatStdcParams,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    datalink::{self, Quality},
};

mod demod;
mod fields;
mod frame;
mod packet;

#[cfg(test)]
mod equivalence;
#[cfg(test)]
mod modulate;
#[cfg(test)]
mod sensitivity;

use demod::{BpskDemod, RATE};
use frame::{
    CODED_SYMBOLS, FRAME_SYMBOLS, FrameDecoder, UW_ACQUIRE_MATCH, UW_TRACK_MATCH, uw_ber_ppt,
    uw_score,
};
use packet::{PacketParser, StdcPacket, details_with_uw_ber};

const HALF_BANDWIDTH: f64 = 2_000.0;
const MID_FRAME_FLIP_MIN_GAIN: u32 = 24;
const RELOCK_AFTER_SYMBOLS: u32 = 2 * FRAME_SYMBOLS as u32;
const LOOKAHEAD: usize = 2;
const MAX_FEC_CORRECTED: u32 = CODED_SYMBOLS as u32 * 3 / 10;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "inmarsat_stdc".to_owned(),
    name: "Inmarsat STD-C / EGC".to_owned(),
    summary: "Maritime text and safety messages over Inmarsat".to_owned(),
    family: DecoderFamily::Marine,
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("inmarsat_stdc".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Body<'a> {
    StdC {
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<&'a str>,
        details: &'a Value,
    },
}

#[derive(Clone, Copy)]
struct Alignment {
    start: usize,
    invert: bool,
    matches: u32,
}

fn alignment_at(symbols: &[f32], start: usize) -> Alignment {
    let (normal, inverted) = uw_score(&symbols[start..start + FRAME_SYMBOLS]);
    Alignment {
        start,
        invert: inverted > normal,
        matches: normal.max(inverted),
    }
}

fn best_alignment(symbols: &[f32], starts: RangeInclusive<usize>) -> Alignment {
    starts
        .map(|start| alignment_at(symbols, start))
        .reduce(|best, candidate| {
            if candidate.matches > best.matches {
                candidate
            } else {
                best
            }
        })
        .unwrap_or_else(|| alignment_at(symbols, 0))
}

pub struct StdcDecoder {
    demod: BpskDemod,
    frames: FrameDecoder,
    parser: PacketParser,
    symbols: Vec<f32>,
    flipped: Vec<f32>,
    start: usize,
    expected: Option<usize>,
    since_lock: u32,
}

impl StdcDecoder {
    pub fn new() -> Self {
        Self {
            demod: BpskDemod::new(RATE),
            frames: FrameDecoder::new(),
            parser: PacketParser::new(),
            symbols: Vec::with_capacity(3 * FRAME_SYMBOLS),
            flipped: vec![0.0; FRAME_SYMBOLS],
            start: 0,
            expected: None,
            since_lock: 0,
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<StdcPacket>) {
        self.demod.process(iq, &mut self.symbols);
        while self.symbols.len() >= self.start + FRAME_SYMBOLS + LOOKAHEAD {
            if self.try_frame(out) {
                self.expected = Some(self.start);
                self.demod.locked = true;
                self.since_lock = 0;
            } else {
                self.start += 1;
                self.since_lock += 1;
                if self.since_lock > RELOCK_AFTER_SYMBOLS {
                    self.demod.locked = false;
                }
            }
        }
        let consumed = self.start.saturating_sub(LOOKAHEAD);
        self.symbols.drain(..consumed);
        self.start -= consumed;
        self.expected = self.expected.and_then(|at| at.checked_sub(consumed));
    }

    fn locate(&mut self) -> Option<Alignment> {
        let start = self.start;
        if self.expected == Some(start) {
            let tracked = best_alignment(
                &self.symbols,
                start.saturating_sub(LOOKAHEAD)..=start + LOOKAHEAD,
            );
            if tracked.matches >= UW_TRACK_MATCH {
                return Some(tracked);
            }
            self.expected = None;
        }
        if alignment_at(&self.symbols, start).matches < UW_ACQUIRE_MATCH {
            return None;
        }
        Some(best_alignment(&self.symbols, start..=start + LOOKAHEAD))
    }

    fn try_frame(&mut self, out: &mut Vec<StdcPacket>) -> bool {
        if let Some(alignment) = self.locate() {
            let soft = &self.symbols[alignment.start..alignment.start + FRAME_SYMBOLS];
            let (bytes, stats) = self.frames.decode(soft, alignment.invert);
            self.emit(
                &bytes,
                stats.fec_corrected,
                uw_ber_ppt(alignment.matches),
                out,
            );
            self.start = alignment.start + FRAME_SYMBOLS;
            return true;
        }
        let soft = &self.symbols[self.start..self.start + FRAME_SYMBOLS];
        let Some(flip) = frame::detect_polarity_flip(soft, MID_FRAME_FLIP_MIN_GAIN)
            .filter(|flip| flip.uw_score >= UW_ACQUIRE_MATCH)
        else {
            return false;
        };
        self.flipped.copy_from_slice(soft);
        frame::apply_polarity_flip(&mut self.flipped, &flip);
        let (bytes, stats) = self.frames.decode(&self.flipped, false);
        self.emit(&bytes, stats.fec_corrected, uw_ber_ppt(flip.uw_score), out);
        self.start += FRAME_SYMBOLS;
        true
    }

    fn emit(
        &mut self,
        bytes: &[u8],
        fec_corrected: u32,
        uw_ber_ppt: u32,
        out: &mut Vec<StdcPacket>,
    ) {
        let first = out.len();
        let rejected = if fec_corrected > MAX_FEC_CORRECTED {
            None
        } else {
            Some(self.parser.parse_frame(bytes, out))
        };
        if rejected != Some(0) {
            out.push(StdcPacket::damaged_frame(rejected));
        }
        for packet in &mut out[first..] {
            packet.fec_corrected = Some(fec_corrected);
            packet.uw_ber_ppt = Some(uw_ber_ppt);
            details_with_uw_ber(&mut packet.details, uw_ber_ppt);
        }
    }
}

pub fn to_datalink(packet: &StdcPacket) -> DataLinkMessage {
    datalink::message(
        &Body::StdC {
            name: packet.name,
            text: packet.text.as_deref(),
            details: &packet.details,
        },
        Quality {
            crc_ok: packet.checksum_ok,
            fec_corrected: packet.fec_corrected,
            ..Quality::default()
        },
        Some(&packet.raw),
    )
}

pub struct InmarsatStdcChannel {
    decoder: StdcDecoder,
    packets: Vec<StdcPacket>,
}

fn params(settings: &ChannelSettings) -> Result<&InmarsatStdcParams, ChannelError> {
    match &settings.params {
        ChannelParams::InmarsatStdc(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "inmarsat STD-C channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-HALF_BANDWIDTH, HALF_BANDWIDTH)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    datalink::channel_filter(RATE, HALF_BANDWIDTH)
}

impl ChannelRx for InmarsatStdcChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            decoder: StdcDecoder::new(),
            packets: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.decoder.process(iq, &mut self.packets);
        out.events.extend(
            self.packets
                .drain(..)
                .map(|packet| DecoderEvent::InmarsatStdc(to_datalink(&packet))),
        );
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::Ddc;

    use super::*;
    use crate::testutil::{run_events, settings};

    pub(super) fn bulletin_board_frame() -> Vec<u8> {
        let mut payload = packet::build_packet(&[0x7D, 1, 0x03, 0xE8, 0, 0, 1, 0x10, 0, 0, 0, 0]);
        payload.resize(639, 0);
        frame::encode_frame(&payload)
    }

    pub(super) fn offair_channel_iq() -> Vec<Complex<f32>> {
        const FIXTURE: &[u8] = include_bytes!("../../../fixtures/inmarsat_stdc_egc_24k.sigmf-data");
        let capture: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| {
                Complex::new(
                    f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0,
                    f32::from(i16::from_le_bytes([s[2], s[3]])) / 32_768.0,
                )
            })
            .collect();
        let mut ddc = Ddc::new(24_000.0, RATE, 216.0).expect("ddc");
        let mut iq = Vec::new();
        ddc.process(&capture, &mut iq);
        iq
    }

    #[test]
    fn decodes_a_bulletin_board_frame() {
        let symbols = bulletin_board_frame();
        let mut bits: Vec<u8> = (0..4_000).map(|index| (index % 2) as u8).collect();
        bits.extend(&symbols);
        bits.extend(&symbols);
        let iq = modulate::modulate(&bits, 1_200.0, RATE, 0.0, 0.5);
        let mut channel = InmarsatStdcChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::InmarsatStdc(InmarsatStdcParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        let message = events
            .iter()
            .find_map(|event| match event {
                DecoderEvent::InmarsatStdc(message) if message.crc_ok => Some(message),
                _ => None,
            })
            .expect("bulletin board");
        assert_eq!(message.message_type, "bulletin-board");
        assert_eq!(message.details["type"], "std_c");
        assert_eq!(message.details["details"]["frame_number"], 1000);
        assert!(message.details["details"]["uw_ber_ppt"].is_number());
        assert_eq!(message.fec_corrected, Some(0));
    }

    #[test]
    fn a_packet_with_a_bad_checksum_surfaces() {
        let mut payload = packet::build_packet(&[0x7D, 1, 0x03, 0xE8, 0, 0, 1, 0x10, 0, 0, 0, 0]);
        let last = payload.len() - 1;
        payload[last] ^= 0x5A;
        payload.resize(639, 0);
        let frame = frame::encode_frame(&payload);
        let mut bits: Vec<u8> = (0..4_000).map(|index| (index % 2) as u8).collect();
        bits.extend(&frame);
        bits.extend(&frame);
        let iq = modulate::modulate(&bits, 1_200.0, RATE, 0.0, 0.5);
        let mut decoder = StdcDecoder::new();
        let mut packets = Vec::new();
        decoder.process(&iq, &mut packets);
        let damaged = packets.first().expect("damaged frame");
        assert_eq!(damaged.name, "damaged-frame");
        assert!(!damaged.checksum_ok);
        assert_eq!(damaged.details["rejected_packets"], 1);
        assert!(packets.iter().all(|packet| !packet.checksum_ok));
    }

    #[test]
    fn decodes_the_offair_recording() {
        let iq = offair_channel_iq();
        let mut decoder = StdcDecoder::new();
        let mut packets = Vec::new();
        for chunk in iq.chunks(4_096) {
            decoder.process(chunk, &mut packets);
        }
        assert!(packets.len() >= 5, "{}", packets.len());
        let board = packets
            .iter()
            .find(|p| p.name == "bulletin-board")
            .expect("bulletin board");
        assert_eq!(board.details["frame_number"], 5987);
        assert_eq!(board.details["utc_time"], "14:22:07");
        assert_eq!(board.details["channel_type_name"], "NCS");
        assert_eq!(board.details["sat_les"]["les"], 144);
        assert_eq!(board.details["status"]["operational"], true);
        let announcement = packets
            .iter()
            .find(|p| p.name == "announcement")
            .expect("announcement");
        assert_eq!(
            announcement.details["sat_les"]["les_name"],
            "Vizada-Telenor, Norway"
        );
        let signalling = packets
            .iter()
            .find(|p| p.name == "signalling-channel")
            .expect("signalling channel");
        assert_eq!(signalling.details["uplink_mhz"], 1636.64);
        assert_eq!(
            signalling.details["tdm_slots"].as_array().map(Vec::len),
            Some(28)
        );
    }
}
