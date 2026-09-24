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
mod message;
mod symbol;

#[cfg(test)]
mod equivalence;
#[cfg(test)]
mod modulate;

use demod::{FskDemod, RATE, find_phasing};
use message::{DscMessage, Format};
use symbol::{LEADING_DX_PHASING, RX_DELAY, SYMBOL_BITS};

const HALF_BANDWIDTH: f64 = 250.0;
const MAX_BITS_WINDOW: usize = 4_096;
const MIN_FRAME_BITS: usize = 460;
const MAX_FRAME_BITS: usize = 2 * SYMBOL_BITS * (LEADING_DX_PHASING + 25 + RX_DELAY + 1);

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

pub struct DscDecoder {
    demod: FskDemod,
    bits: Vec<u8>,
    scanned: usize,
}

impl DscDecoder {
    pub fn new() -> Self {
        Self {
            demod: FskDemod::new(),
            bits: Vec::with_capacity(2 * MAX_BITS_WINDOW),
            scanned: 0,
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<DscMessage>) {
        self.demod.process(iq, &mut self.bits);
        while let Some(offset) = find_phasing(&self.bits[self.scanned..]) {
            let start = self.scanned + offset;
            let available = self.bits.len() - start;
            if available < MIN_FRAME_BITS {
                break;
            }
            let message = decode_from_bits(&self.bits[start..]);
            if message.format != Format::Unknown {
                if !message.is_complete() && available < MAX_FRAME_BITS {
                    break;
                }
                out.push(message);
            }
            self.scanned = start + SYMBOL_BITS;
            if self.scanned >= self.bits.len() {
                break;
            }
        }
        if self.bits.len() > MAX_BITS_WINDOW {
            let consumed = self.scanned.min(self.bits.len());
            self.bits.drain(..consumed);
            self.scanned = 0;
        }
    }
}

pub fn decode_from_bits(bits: &[u8]) -> DscMessage {
    let chars = symbol::decode_bitstream(bits);
    let symbols = symbol::deinterleave_dx_rx(&chars, LEADING_DX_PHASING, RX_DELAY);
    message::decode(&symbols)
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
