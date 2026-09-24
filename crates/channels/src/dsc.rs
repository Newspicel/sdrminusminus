use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, DscParams,
};
use serde::Serialize;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    datalink::{self, Quality},
};

mod demod;
mod frame;
mod message;
mod symbol;

#[cfg(test)]
mod equivalence;
#[cfg(test)]
mod modulate;
#[cfg(test)]
mod sensitivity;

use demod::{FskDemod, RATE, find_phasing};
use frame::{Received, decode_at, frame_bits};
use message::{DscMessage, Format};
use symbol::{LEADING_DX_PHASING, RX_DELAY, SYMBOL_BITS};

const HALF_BANDWIDTH: f64 = 250.0;
const CHAR_PAIR_BITS: usize = 2 * SYMBOL_BITS;
const MAX_BITS_WINDOW: usize = 4_096;
const HISTORY_BITS: usize = CHAR_PAIR_BITS * LEADING_DX_PHASING;
const MIN_FRAME_BITS: usize = 460;
const MAX_FRAME_BITS: usize = CHAR_PAIR_BITS * (LEADING_DX_PHASING + 25 + RX_DELAY + 1);

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dsc".to_owned(),
    name: "Digital Selective Calling".to_owned(),
    summary: "Maritime distress and calling alerts".to_owned(),
    family: DecoderFamily::Marine,
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("dsc".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Body<'a> {
    Dsc {
        kind: &'static str,
        details: &'a DscMessage,
    },
}

enum Outcome {
    Emit(Box<DscMessage>, usize),
    Skip,
    Wait,
}

pub struct DscDecoder {
    demod: FskDemod,
    soft: Vec<f32>,
    hard: Vec<u8>,
    scanned: usize,
    waiting: Option<(usize, usize)>,
}

impl DscDecoder {
    pub fn new() -> Self {
        Self {
            demod: FskDemod::new(),
            soft: Vec::with_capacity(2 * MAX_BITS_WINDOW),
            hard: Vec::with_capacity(2 * MAX_BITS_WINDOW),
            scanned: 0,
            waiting: None,
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<DscMessage>) {
        let before = self.soft.len();
        self.demod.process(iq, &mut self.soft);
        self.hard.extend(
            self.soft[before..]
                .iter()
                .map(|&soft| u8::from(soft >= 0.0)),
        );
        while let Some(offset) = self.hard.get(self.scanned..).and_then(find_phasing) {
            let found = self.scanned + offset;
            match self.outcome(found) {
                Outcome::Wait => break,
                Outcome::Skip => self.scanned = found + SYMBOL_BITS,
                Outcome::Emit(message, end) => {
                    out.push(*message);
                    self.scanned = end.max(found + SYMBOL_BITS);
                }
            }
            if self.scanned >= self.hard.len() {
                break;
            }
        }
        self.trim();
    }

    fn outcome(&mut self, found: usize) -> Outcome {
        let available = self.hard.len() - found;
        let unchanged = self
            .waiting
            .is_some_and(|(at, length)| at == found && self.hard.len() < length + SYMBOL_BITS);
        if available < MIN_FRAME_BITS || unchanged {
            return Outcome::Wait;
        }
        let received = Received {
            hard: &self.hard,
            soft: &self.soft,
        };
        if let Some((start, message)) = best_valid(&received, found) {
            self.waiting = None;
            let end = start + frame_bits(&message);
            return Outcome::Emit(Box::new(message), end);
        }
        let primary = decode_at(&received, found);
        if primary.format == Format::Unknown {
            return Outcome::Skip;
        }
        let settled =
            primary.is_complete() && available >= frame_bits(&primary) + CHAR_PAIR_BITS * RX_DELAY;
        if !settled && available < MAX_FRAME_BITS {
            self.waiting = Some((found, self.hard.len()));
            return Outcome::Wait;
        }
        self.waiting = None;
        let end = found + frame_bits(&primary);
        Outcome::Emit(Box::new(primary), end)
    }

    fn trim(&mut self) {
        if self.hard.len() <= MAX_BITS_WINDOW {
            return;
        }
        let drop = self.scanned.saturating_sub(HISTORY_BITS);
        self.hard.drain(..drop);
        self.soft.drain(..drop);
        self.scanned -= drop;
        self.waiting = None;
    }
}

fn best_valid(received: &Received, found: usize) -> Option<(usize, DscMessage)> {
    let mut best: Option<(usize, usize, DscMessage)> = None;
    for start in (0..LEADING_DX_PHASING).filter_map(|back| found.checked_sub(back * CHAR_PAIR_BITS))
    {
        let message = decode_at(received, start);
        if message.format == Format::Unknown || !message.is_complete() || !message.ecc_ok() {
            continue;
        }
        let score = received.phasing_score(start);
        if best
            .as_ref()
            .is_none_or(|(best_score, _, _)| score > *best_score)
        {
            best = Some((score, start, message));
        }
    }
    best.map(|(_, start, message)| (start, message))
}

pub fn to_datalink(message: &DscMessage) -> DataLinkMessage {
    let raw: Vec<u8> = message
        .symbols
        .iter()
        .map(|&symbol| symbol.clamp(0, 255) as u8)
        .collect();
    datalink::message(
        &Body::Dsc {
            kind: message.format.kind(),
            details: message,
        },
        Quality {
            crc_ok: message.ecc_ok(),
            ..Quality::default()
        },
        Some(&raw),
    )
}

pub struct DscChannel {
    decoder: DscDecoder,
    messages: Vec<DscMessage>,
}

fn params(settings: &ChannelSettings) -> Result<&DscParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dsc(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "dsc channel got {} params",
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

impl ChannelRx for DscChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            decoder: DscDecoder::new(),
            messages: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.decoder.process(iq, &mut self.messages);
        out.events.extend(
            self.messages
                .drain(..)
                .map(|message| DecoderEvent::Dsc(to_datalink(&message))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{run_events, settings};

    const DISTRESS: &[i32] = &[
        112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
    ];
    const INDIVIDUAL: &[i32] = &[
        120, 120, 32, 51, 42, 0, 0, 108, 0, 23, 71, 0, 0, 118, 126, 4, 10, 10, 4, 39, 30, 122, 54,
        122, 122,
    ];

    fn padded(symbols: &[i32]) -> Vec<Complex<f32>> {
        let mut iq = vec![Complex::new(0.0, 0.0); 400];
        iq.extend(modulate::call_iq(symbols, RATE, 0.0, 0.6));
        iq.extend(vec![Complex::new(0.0, 0.0); 400]);
        iq
    }

    fn decode_all(iq: &[Complex<f32>]) -> Vec<DscMessage> {
        let mut decoder = DscDecoder::new();
        let mut out = Vec::new();
        for chunk in iq.chunks(512) {
            decoder.process(chunk, &mut out);
        }
        out
    }

    #[test]
    fn decodes_a_distress_alert_fixture() {
        let symbols = [
            112, 112, 12, 34, 56, 78, 90, 100, 12, 34, 45, 67, 89, 12, 12, 34, 117, 88,
        ];
        let iq = modulate::call_iq(&symbols, RATE, 0.0, 0.8);
        let mut channel = DscChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Dsc(DscParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        let message = events
            .iter()
            .find_map(|event| match event {
                DecoderEvent::Dsc(message) => Some(message),
                _ => None,
            })
            .expect("DSC message");
        assert!(message.crc_ok);
        assert_eq!(message.message_type, "distress_alert");
        assert_eq!(message.station.as_deref(), Some("123456789"));
    }

    #[test]
    fn distress_alert_from_iq() {
        let out = decode_all(&padded(DISTRESS));
        let m = out
            .iter()
            .find(|m| m.format == Format::DistressAlert)
            .expect("distress alert");
        assert_eq!(m.from.as_deref(), Some("255805997"));
        assert_eq!(m.position.as_deref(), Some("45 26N 013 07E"));
        assert_eq!(m.time.as_deref(), Some("12:52"));
        assert_eq!(m.ecc, 52);
        assert!(m.ecc_ok());
    }

    #[test]
    fn individual_station_call_from_iq() {
        let out = decode_all(&padded(INDIVIDUAL));
        let m = out
            .iter()
            .find(|m| m.format == Format::IndividualStationCall)
            .expect("individual call");
        assert_eq!(m.to.as_deref(), Some("325142000"));
        assert_eq!(m.from.as_deref(), Some("002371000"));
        assert_eq!(m.frequency.as_deref(), Some("04101.0/04393.0"));
        assert!(m.ecc_ok());
    }

    #[test]
    fn datalink_shape() {
        let out = decode_all(&padded(DISTRESS));
        let m = out
            .iter()
            .find(|m| m.format == Format::DistressAlert)
            .expect("distress alert");
        let message = to_datalink(m);
        assert_eq!(message.message_type, "distress_alert");
        assert!(message.crc_ok);
        assert_eq!(message.station.as_deref(), Some("255805997"));
        assert_eq!(message.details["type"], "dsc");
        assert_eq!(message.details["details"]["position"], "45 26N 013 07E");
        assert!(message.raw.is_some());
    }

    #[test]
    fn a_frame_that_never_completes_does_not_stall_the_decoder() {
        let mut symbols = DISTRESS.to_vec();
        symbols.truncate(16);
        symbols.extend([5; 10]);
        let mut iq = padded(&symbols);
        iq.extend(padded(DISTRESS));
        let out = decode_all(&iq);
        assert!(
            out.iter()
                .any(|m| m.format == Format::DistressAlert && !m.ecc_ok())
        );
        assert!(
            out.iter()
                .any(|m| m.format == Format::DistressAlert && m.ecc_ok())
        );
    }
}
