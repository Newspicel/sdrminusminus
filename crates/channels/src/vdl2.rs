mod atn;
mod avlc;
mod cpdlc;
mod cpdlc_tables;
mod decoder;
mod demod;
mod header;
mod interleave;
#[cfg(test)]
mod modulate;
mod scramble;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, Vdl2Params,
};
use serde::Serialize;
use serde_json::{Value, json};

use self::avlc::{AvlcFrame, Control, Payload};
use self::decoder::{Vdl2Decoder, Vdl2Frame};
use crate::datalink::{self, Quality};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const RATE: f64 = 100_000.0;
const HALF_BANDWIDTH: f64 = 8_500.0;
const INFO_HEX_LIMIT: usize = 64;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "vdl2".to_owned(),
    name: "VDL Mode 2".to_owned(),
    summary: "Aircraft datalink on VHF".to_owned(),
    family: DecoderFamily::Aviation,
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("vdl2".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct Vdl2Channel {
    decoder: Vdl2Decoder,
    frames: Vec<Vdl2Frame>,
}

fn params(settings: &ChannelSettings) -> Result<&Vdl2Params, ChannelError> {
    match &settings.params {
        ChannelParams::Vdl2(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "VDL2 channel got {} params",
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

impl ChannelRx for Vdl2Channel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            decoder: Vdl2Decoder::new(ctx.input_rate),
            frames: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.frames.clear();
        self.decoder.process(iq, &mut self.frames);
        out.events.extend(
            self.frames
                .iter()
                .map(|frame| DecoderEvent::Vdl2(message(frame))),
        );
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Body<'a, C: Serialize> {
    Acars(&'a C),
    Vdl2 { kind: String, details: Value },
}

fn message(frame: &Vdl2Frame) -> DataLinkMessage {
    let quality = Quality {
        crc_ok: frame.acars.as_ref().is_none_or(|block| block.crc_ok),
        fec_corrected: u32::try_from(frame.rs_corrected).ok(),
        snr_db: Some(frame.snr_db),
        frequency_error_hz: Some(frame.freq_skew_hz),
    };
    let raw = Some(frame.avlc.raw.as_slice());
    match &frame.acars {
        Some(block) => datalink::message(&Body::Acars(&block.core), quality, raw),
        None => {
            let (kind, details) = avlc_body(&frame.avlc, frame.atn.as_ref());
            datalink::message(&Body::<()>::Vdl2 { kind, details }, quality, raw)
        }
    }
}

fn frame_kind(frame: &AvlcFrame) -> String {
    match (&frame.control, &frame.payload) {
        (Control::Unnumbered { kind: "XID", .. }, _) => "xid".to_owned(),
        (Control::Unnumbered { kind, .. } | Control::Supervisory { kind, .. }, _) => {
            format!("avlc-{}", kind.to_lowercase())
        }
        (Control::Info { .. }, Payload::Atn { .. }) => "atn".to_owned(),
        (Control::Info { .. }, _) => "avlc-i".to_owned(),
    }
}

fn avlc_body(frame: &AvlcFrame, atn: Option<&Value>) -> (String, Value) {
    let mut details = json!({
        "dst": frame.dst,
        "src": frame.src,
        "control": frame.control,
    });
    if let Payload::Atn { ipi } = frame.payload {
        details["protocol"] = json!(match ipi {
            0x81 => "CLNP",
            0x82 => "ES-IS",
            _ => "IDRP",
        });
    }
    if let Some(atn) = atn {
        details["atn"] = atn.clone();
    }
    if let Control::Unnumbered { kind, .. } = frame.control {
        if kind == "XID"
            && let Some(params) = avlc::parse_xid(&frame.info)
        {
            details["params"] = json!(params);
        }
        if kind == "FRMR"
            && let Some(frmr) = avlc::parse_frmr(&frame.info)
        {
            details["frmr"] = json!(frmr);
        }
    }
    if !frame.info.is_empty() {
        let shown = &frame.info[..frame.info.len().min(INFO_HEX_LIMIT)];
        details["info_hex"] = json!(datalink::hex(shown));
        details["info_len"] = json!(frame.info.len());
    }
    (frame_kind(frame), details)
}

#[cfg(test)]
mod tests;
