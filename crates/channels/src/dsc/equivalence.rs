use num_complex::Complex;
use sdrmm_wire::DataLinkMessage;

use super::{DscDecoder, RATE, modulate, to_datalink};
use crate::{
    datalink::{self, Quality},
    testutil::add_awgn,
};

pub(super) const CALLS: &[&[i32]] = &[
    &[
        112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
    ],
    &[
        120, 120, 32, 51, 42, 0, 0, 108, 0, 23, 71, 0, 0, 118, 126, 4, 10, 10, 4, 39, 30, 122, 54,
        122, 122,
    ],
    &[
        120, 120, 0, 25, 70, 0, 0, 108, 23, 20, 19, 71, 50, 109, 126, 55, 5, 85, 30, 1, 34, 117,
        18, 117, 117,
    ],
    &[
        116, 116, 108, 0, 23, 71, 0, 0, 109, 126, 4, 12, 50, 4, 12, 50, 127, 36, 127, 127,
    ],
    &[
        102, 102, 4, 40, 3, 5, 8, 108, 0, 22, 75, 40, 0, 109, 126, 2, 18, 20, 2, 18, 20, 127, 49,
        127, 127,
    ],
];

const CHUNKS: [usize; 5] = [512, 1, 4_096, 77, 1_000];

fn xng_datalink(message: &xng_mode_dsc::DscMessage) -> DataLinkMessage {
    let converted = xng_mode_dsc::to_message(
        message,
        0,
        0.0,
        xng_types::Provenance {
            station: xng_types::StationIdentity::new("SDR--"),
            app: xng_types::AppInfo {
                name: "SDR--".to_owned(),
                version: String::new(),
            },
            sdr: None,
            channel: None,
        },
    );
    datalink::message(
        &converted.body,
        Quality {
            crc_ok: converted.decode.crc_ok,
            fec_corrected: converted.decode.fec_corrected,
            snr_db: converted.signal.snr_db,
            frequency_error_hz: converted.signal.freq_skew_hz,
        },
        converted.raw.as_deref(),
    )
}

pub(super) fn run_xng(iq: &[Complex<f32>], chunk: usize) -> Vec<DataLinkMessage> {
    let mut decoder = xng_mode_dsc::DscChannelDecoder::new(RATE, 0.0).expect("xng decoder");
    iq.chunks(chunk)
        .flat_map(|piece| decoder.process(piece))
        .map(|message| xng_datalink(&message))
        .collect()
}

pub(super) fn run_ours(iq: &[Complex<f32>], chunk: usize) -> Vec<DataLinkMessage> {
    let mut decoder = DscDecoder::new();
    let mut messages = Vec::new();
    for piece in iq.chunks(chunk) {
        decoder.process(piece, &mut messages);
    }
    messages.iter().map(to_datalink).collect()
}

pub(super) fn transmission(
    calls: &[&[i32]],
    realistic: bool,
    sigma: f32,
    seed: u64,
) -> Vec<Complex<f32>> {
    offset_transmission(calls, realistic, 0.0, sigma, seed)
}

pub(super) fn offset_transmission(
    calls: &[&[i32]],
    realistic: bool,
    offset_hz: f64,
    sigma: f32,
    seed: u64,
) -> Vec<Complex<f32>> {
    let mut iq = vec![Complex::new(0.0, 0.0); 1_000];
    for call in calls {
        if realistic {
            iq.extend(modulate::m493_call_iq(call, RATE, 0.0, 1.0));
        } else {
            iq.extend(xng_mode_dsc::modulate::call_iq(call, RATE, 0.0, 1.0));
        }
        iq.extend(vec![Complex::new(0.0, 0.0); 3_000]);
    }
    crate::testgen::shift(&mut iq, offset_hz, RATE);
    add_awgn(&mut iq, sigma, seed);
    let mut filtered = Vec::with_capacity(iq.len());
    super::channel_filter().process(&iq, &mut filtered);
    filtered
}

#[test]
fn modulators_agree() {
    for call in CALLS {
        assert_eq!(
            modulate::call_iq(call, RATE, 0.0, 0.7),
            xng_mode_dsc::modulate::call_iq(call, RATE, 0.0, 0.7)
        );
    }
}

#[test]
fn symbol_layer_matches_xng() {
    for call in CALLS {
        let mut symbols = call.to_vec();
        for erased in [None, Some(3), Some(9), Some(call.len() - 2)] {
            if let Some(index) = erased {
                symbols[index] = -1;
            }
            let ours = serde_json::to_value(super::message::decode(&symbols)).ok();
            let theirs = serde_json::to_value(xng_mode_dsc::decode(&symbols)).ok();
            assert_eq!(ours, theirs, "{symbols:?}");
        }
    }
}

fn essence(message: &DataLinkMessage) -> serde_json::Value {
    let mut details = message.details.clone();
    if let Some(inner) = details
        .get_mut("details")
        .and_then(serde_json::Value::as_object_mut)
    {
        inner.remove("symbols");
    }
    serde_json::json!([
        message.message_type,
        message.station,
        message.crc_ok,
        details
    ])
}

#[test]
fn every_valid_xng_decode_is_also_ours() {
    let mut compared = 0;
    for (seed, sigma) in [(1u64, 0.0f32), (2, 1.0), (3, 1.4), (4, 1.8), (5, 2.2)] {
        for (realistic, call) in [false, true]
            .into_iter()
            .flat_map(|r| CALLS.iter().map(move |c| (r, c)))
        {
            let iq = transmission(&[call], realistic, sigma, seed);
            for chunk in CHUNKS {
                let ours: Vec<_> = run_ours(&iq, chunk).iter().map(essence).collect();
                for theirs in run_xng(&iq, chunk).iter().filter(|m| m.crc_ok) {
                    assert!(
                        ours.contains(&essence(theirs)),
                        "sigma {sigma} chunk {chunk}: {theirs:?}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 80, "{compared}");
}

#[test]
fn back_to_back_calls_all_decode_once() {
    for realistic in [false, true] {
        let iq = transmission(CALLS, realistic, 0.3, 21);
        for chunk in CHUNKS {
            let ours = run_ours(&iq, chunk);
            assert_eq!(ours.len(), CALLS.len(), "{ours:?}");
            assert!(ours.iter().all(|message| message.crc_ok));
            assert_eq!(run_xng(&iq, chunk).len(), 1);
        }
    }
}
