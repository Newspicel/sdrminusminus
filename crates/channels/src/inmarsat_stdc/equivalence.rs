use num_complex::Complex;
use sdrmm_wire::DataLinkMessage;

use super::{RATE, StdcDecoder, frame, packet, tests::offair_channel_iq, to_datalink};
use crate::{
    datalink::{self, Quality},
    testutil::add_awgn,
};

const SYMBOL_RATE: f64 = 1_200.0;

fn xng_datalink(packet: &xng_mode_stdc::packet::StdcPacket) -> DataLinkMessage {
    let converted = xng_mode_stdc::to_message(
        packet,
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
    let mut decoder = xng_mode_stdc::StdcChannelDecoder::new(RATE, 0.0).expect("xng decoder");
    iq.chunks(chunk)
        .flat_map(|piece| decoder.process(piece))
        .map(|packet| xng_datalink(&packet))
        .collect()
}

pub(super) fn run_ours(iq: &[Complex<f32>], chunk: usize) -> Vec<DataLinkMessage> {
    let mut decoder = StdcDecoder::new();
    let mut packets = Vec::new();
    for piece in iq.chunks(chunk) {
        decoder.process(piece, &mut packets);
    }
    packets.iter().map(to_datalink).collect()
}

fn egc_packet(text: &[u8], sequence: u16) -> Vec<u8> {
    let mut body = vec![0xB0, 0u8, 0x31, (1 << 5) | 1];
    body.extend(sequence.to_be_bytes());
    body.push(1);
    body.push(0);
    body.extend([0x12, 0x34, 0x56, 0x78]);
    body.extend(text);
    body[1] = body.len() as u8;
    packet::build_packet(&body)
}

pub(super) fn frame_payload(text: &[u8], sequence: u16) -> Vec<u8> {
    let mut payload = packet::build_packet(&[0x7D, 1, 0x03, 0xE8, 0, 0, 1, 0x10, 0, 0, 0, 0]);
    payload.extend(egc_packet(text, sequence));
    payload.resize(639, 0);
    payload
}

pub(super) fn transmission(
    frames: usize,
    offset_hz: f64,
    sigma: f32,
    seed: u64,
) -> Vec<Complex<f32>> {
    let mut symbols: Vec<u8> = (0..4_000).map(|index| (index % 2) as u8).collect();
    for index in 0..frames {
        let text = format!("SECURITE NAVAREA XII {index:03} BUOY ADRIFT");
        symbols.extend(frame::encode_frame(&frame_payload(
            text.as_bytes(),
            index as u16,
        )));
    }
    let mut iq = super::modulate::modulate(&symbols, SYMBOL_RATE, RATE, offset_hz, 0.5);
    add_awgn(&mut iq, sigma, seed);
    let mut filtered = Vec::with_capacity(iq.len());
    super::channel_filter().process(&iq, &mut filtered);
    filtered
}

#[test]
fn modulators_agree() {
    let symbols = frame::encode_frame(&frame_payload(b"TEST", 1));
    let ours = super::modulate::modulate(&symbols[..2_000], SYMBOL_RATE, RATE, 230.0, 0.5);
    let theirs =
        xng_mode_stdc::modulate::modulate(&symbols[..2_000], SYMBOL_RATE, RATE, 230.0, 0.5);
    assert_eq!(ours, theirs);
    assert_eq!(
        symbols,
        xng_mode_stdc::frame::encode_frame(&frame_payload(b"TEST", 1))
    );
}

#[test]
fn frame_layer_matches_xng() {
    let symbols = frame::encode_frame(&frame_payload(b"SECURITE TEST", 4));
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let soft: Vec<f32> = symbols
        .iter()
        .map(|&bit| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state >> 40) as f32 / (1u64 << 24) as f32 * 3.0 - 1.5;
            if bit == 1 { 1.0 + noise } else { -1.0 + noise }
        })
        .collect();
    for invert in [false, true] {
        let (bytes, stats) = frame::FrameDecoder::new().decode(&soft, invert);
        let (theirs, their_stats) =
            xng_mode_stdc::frame::FrameDecoder::new().decode_with_stats(&soft, invert);
        assert_eq!(bytes, theirs);
        assert_eq!(stats.fec_corrected, their_stats.fec_corrected);
    }
}

#[test]
fn packet_layer_matches_xng() {
    let payload = frame_payload(b"SECURITE TEST", 9);
    let mut ours = Vec::new();
    packet::PacketParser::new().parse_frame(&payload, &mut ours);
    let theirs = xng_mode_stdc::packet::PacketParser::new().parse_frame(&payload);
    assert_eq!(ours.len(), theirs.len());
    for (a, b) in ours.iter().zip(&theirs) {
        assert_eq!(
            (a.name, &a.text, &a.details, &a.raw),
            (b.name, &b.text, &b.details, &b.raw)
        );
    }
}

fn essence(message: &DataLinkMessage) -> serde_json::Value {
    let mut details = message.details.clone();
    if let Some(inner) = details
        .get_mut("details")
        .and_then(serde_json::Value::as_object_mut)
    {
        inner.remove("uw_ber_ppt");
    }
    serde_json::json!([message.message_type, message.text, message.raw, details])
}

fn assert_superset(iq: &[Complex<f32>], chunk: usize) -> usize {
    let ours: Vec<_> = run_ours(iq, chunk).iter().map(essence).collect();
    let theirs: Vec<_> = run_xng(iq, chunk).iter().map(essence).collect();
    for packet in &theirs {
        assert!(ours.contains(packet), "missing {packet}");
    }
    theirs.len()
}

#[test]
fn keeps_every_xng_packet_on_the_offair_recording() {
    let iq = offair_channel_iq();
    for chunk in [4_096, 777] {
        assert!(assert_superset(&iq, chunk) >= 5);
    }
}

#[test]
fn keeps_every_xng_packet_on_noisy_synthetic_frames() {
    let mut compared = 0;
    for (seed, sigma, offset) in [(1u64, 0.2f32, 230.0), (2, 0.8, -310.0), (3, 1.2, 90.0)] {
        compared += assert_superset(&transmission(3, offset, sigma, seed), 8_192);
    }
    assert!(compared >= 6, "{compared}");
}
