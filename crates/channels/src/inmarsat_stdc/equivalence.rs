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

fn run_xng(iq: &[Complex<f32>], chunk: usize) -> Vec<DataLinkMessage> {
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
fn demod_symbols_match_xng() {
    let iq = transmission(2, 180.0, 0.3, 11);
    let mut ours = Vec::new();
    super::demod::BpskDemod::new(RATE).process(&iq, &mut ours);
    let mut theirs = Vec::new();
    xng_mode_stdc::demod::BpskDemod::new(RATE).process(&iq, &mut theirs);
    assert_eq!(ours, theirs);
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

#[test]
fn matches_xng_on_the_offair_recording() {
    let iq = offair_channel_iq();
    for chunk in [4_096, 777] {
        let ours = run_ours(&iq, chunk);
        assert!(ours.len() >= 5);
        assert_eq!(ours, run_xng(&iq, chunk));
    }
}

#[test]
fn matches_xng_on_noisy_synthetic_frames() {
    let mut compared = 0;
    for (seed, sigma, offset) in [(1u64, 0.2f32, 230.0), (2, 0.8, -310.0), (3, 1.2, 90.0)] {
        let iq = transmission(3, offset, sigma, seed);
        let ours = run_ours(&iq, 8_192);
        assert_eq!(ours, run_xng(&iq, 8_192), "sigma {sigma}");
        compared += ours.len();
    }
    assert!(compared >= 6, "{compared}");
}
