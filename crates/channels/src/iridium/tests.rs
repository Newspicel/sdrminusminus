use num_complex::Complex;
use sdrmm_wire::{ChannelParams, DecoderEvent, IridiumParams};
use serde_json::Value;

use super::acars;
use super::decode::{Reassembly, decode_bits};
use super::encode::{
    bits_of_str, da_burst_bits, encode_lcw, ims_bits, ira_bits, ira_payload, pager_blocks,
};
use super::frame::{ACCESS_DL, symbol_reverse};
use super::ira::IridiumFrame;
use super::modulate::modulate;
use super::receiver::ChannelDecoder;
use super::{CHANNEL_RATE, IridiumChannel};
use crate::testutil::{run_events, settings};
use crate::{ChannelCtx, ChannelRx};

const OFF_AIR_RA: &str = "0011000000110000111100111111100001001010010011010011101101101100001001101011100001110011001100110000000111100010010011010011101011110101110100010010011010000111000101000111100110001000111111111111111111111111111111111111111111111111111111111111111110010111";
const OFF_AIR_IDA: &str = "0011000000110000111100110011000110001101111001011111011101001001100100101110100110001000000101000000000101001100010000001100000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000110000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010001000010010001100000000000000110010001000000010001000110000";
const OFF_AIR_IBC: &str = "0011000000110000111100110000000000111110010011111101110011011100000110111111011110111011011100111101101101010011100011101110010000001010101110111000001000000011001011100100101011011011100011101100111110100010111100000110111100101110011000101101101110001110110011101110001011110010111011";
const OFF_AIR_IBC_EXPIRY: &str = "001100000011000011110011000000000011111001001111110111001101110000011011111101111011101101110000110101110011100110010000000000000000000000000000000110000000000110000011001110100100100110001010110011101100000111110010111011011000001100111010010010011000101011001110110000011111001011101100";
const OFF_AIR_U3: &str = "001100000011000011110011001100011000000101100001110100010111110100010000101100110110000000000101000000010000011011001100100000100100010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const ITL_ORACLE: &str = "001100000011000011110011110000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000101001110001100110011010110111010011001011011101010101011110011101101111110101111111111100111011111000101110101111110001000001011001100100110010110100011110001001010111000100001000000011010100011001000010000011101111110010110011110111111010100001010101000000010010100010101110010011011010010010101111001001000010111110111010000010000010000000100011100100001100100101111100000101110001011010110001001011111011001001011010111010001100001001110110110011010100100000100000000111001101001100111001001111110011010011111111010001011110011011010101000111000111100010110011111000101011011000101100000010101000111100010101101000011011111001001010000001111000101001110011001101001111111010110000101010001110100000110000011001100100001000110101100011101110110010000010101110110100";
const ORACLE_RA: &str = "001100000011000011110011111010100110001000001000100110000011110111011000101110101011000001010001001001100010001010011101000110011011111011000010001011111001001101110111100100010000001100000010010110110010000100111111100000000000111110001100110011001111111111111111111111111111111111111111111111111111111111111111";
const ORACLE_IMS: &str = "00110000001100001111001100110011111100110011001111110011111001111011000111110011010001111001100011100101001000001100100011100111001000011101000000001000000000110010000000100010000000101101001111001100110110001101110010110100001100000100000101111000101000100100010101100000010001001001100100101101010110001000101010000110101110111101100001101011100110011101011001100100101110000100001011110000010000000011000000000001011010100000001100000000";
const ORACLE_DA: &str = "0011000000110000111100111100110011001100100000000001001100000010001100101110110001100111001001000000000100010100000001000100000000010010011110001010000001101000101111001001001010010000111111010000010100110001001011001000001010111101110111110101100001101101011010010011100001001011001111101001100011100110010101100101011011010101001000111000001100010001101111101010010110101100110000";

type Snapshot = (String, Value, Option<Value>);

fn canonical(raw: &str) -> Vec<u8> {
    symbol_reverse(&bits_of_str(raw))
}

fn snapshot(frame: &IridiumFrame) -> Snapshot {
    (
        frame.kind.to_owned(),
        frame.details.clone(),
        frame
            .acars
            .as_ref()
            .and_then(|a| serde_json::to_value(a).ok()),
    )
}

fn xng_snapshot(frame: &xng_mode_iridium::ira::IridiumFrame) -> Snapshot {
    (
        frame.kind.to_owned(),
        frame.details.clone(),
        frame
            .acars
            .as_ref()
            .and_then(|a| serde_json::to_value(a).ok()),
    )
}

fn decode_burst(bits: &[u8]) -> Vec<IridiumFrame> {
    let mut out = Vec::new();
    Reassembly::new().handle(bits, &[], 0.0, 0.0, &mut out);
    out
}

fn single(bits: &[u8]) -> IridiumFrame {
    let mut frames = decode_burst(bits);
    assert_eq!(frames.len(), 1, "one frame expected");
    frames.remove(0)
}

fn ours(iq: &[Complex<f32>]) -> Vec<Snapshot> {
    let mut decoder = ChannelDecoder::new();
    let mut frames = Vec::new();
    for chunk in iq.chunks(65_536) {
        decoder.process(chunk, &mut frames);
    }
    frames.iter().map(snapshot).collect()
}

fn xng(iq: &[Complex<f32>]) -> Vec<Snapshot> {
    let Ok(mut decoder) = xng_mode_iridium::IridiumChannelDecoder::new(CHANNEL_RATE, 0.0) else {
        return Vec::new();
    };
    iq.chunks(65_536)
        .flat_map(|chunk| decoder.process(chunk))
        .map(|frame| xng_snapshot(&frame))
        .collect()
}

pub(super) struct Gaussian(pub(super) u64);

impl Gaussian {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub(super) fn sample(&mut self, sigma: f32) -> Complex<f32> {
        let radius = (-2.0 * self.unit().ln()).sqrt();
        let angle = std::f64::consts::TAU * self.unit();
        Complex::new(
            (radius * angle.cos()) as f32 * sigma,
            (radius * angle.sin()) as f32 * sigma,
        )
    }
}

pub(super) fn add_noise(iq: &mut [Complex<f32>], sigma: f32, seed: u64) {
    let mut noise = Gaussian(seed);
    for sample in iq {
        *sample += noise.sample(sigma);
    }
}

pub(super) fn place(bursts: &[Vec<Complex<f32>>], gap: usize) -> Vec<Complex<f32>> {
    let mut iq = vec![Complex::default(); gap];
    for burst in bursts {
        iq.extend_from_slice(burst);
        iq.extend(std::iter::repeat_n(Complex::default(), gap));
    }
    iq.extend(std::iter::repeat_n(Complex::default(), 40_000));
    iq
}

fn sync_burst_bits(sync_bytes: &[u8; 39]) -> Vec<u8> {
    let mut bits = ACCESS_DL.to_vec();
    bits.extend(encode_lcw(7, 0, 0));
    for &byte in sync_bytes {
        bits.extend((0..8).rev().map(|k| (byte >> k) & 1));
    }
    bits
}

fn acars_l2(tail: &str, flight: &str) -> Vec<u8> {
    let block = acars::build(
        '2',
        tail,
        None,
        "Q0",
        '5',
        Some("M01A"),
        Some(flight),
        "",
        false,
    );
    let mut l2 = vec![0x06, 0x00];
    let mut prehdr = vec![0u8; 29];
    prehdr[0] = 0x20;
    prehdr[15] = 1;
    l2.extend_from_slice(&prehdr);
    l2.extend_from_slice(&block);
    l2
}

pub(super) fn da_fragments(l2: &[u8]) -> Vec<Vec<u8>> {
    let chunks: Vec<&[u8]> = l2.chunks(20).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut payload = [0u8; 20];
            payload[..chunk.len()].copy_from_slice(chunk);
            da_burst_bits(
                i + 1 < chunks.len(),
                (i % 8) as u8,
                chunk.len() as u8,
                &payload,
            )
        })
        .collect()
}

fn gr_reference_burst() -> Vec<Complex<f32>> {
    const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/iridium_prbs15_250k.sigmf-data");
    FIXTURE
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| {
            Complex::new(
                f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                f32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            )
        })
        .collect()
}

fn traffic_bursts() -> Vec<Vec<u8>> {
    let mut bursts = vec![
        ira_bits(&ira_payload(42, 13, [-1200, 800, 1500], &[0xDEADBEEF])),
        canonical(OFF_AIR_RA),
        ims_bits(&pager_blocks(1_234_567, "CALL OPS +14155550100")),
        canonical(ITL_ORACLE),
        canonical(OFF_AIR_IBC),
        canonical(OFF_AIR_IBC_EXPIRY),
        canonical(OFF_AIR_IDA),
        canonical(OFF_AIR_U3),
        sync_burst_bits(&[0xAA; 39]),
    ];
    bursts.extend(da_fragments(&acars_l2("N321AB", "UA1234")));
    bursts
}

fn modulated_stream(bursts: &[Vec<u8>], sigma: f32, seed: u64) -> Vec<Complex<f32>> {
    let iq: Vec<Vec<Complex<f32>>> = bursts
        .iter()
        .enumerate()
        .map(|(i, bits)| {
            let cfo = (i as f64 * 1_370.0) % 7_000.0 - 3_500.0;
            modulate(bits, 64, CHANNEL_RATE, cfo, 0.5)
        })
        .collect();
    let mut stream = place(&iq, 12_000);
    add_noise(&mut stream, sigma, seed);
    stream
}

#[test]
fn decodes_a_remodulated_off_air_ring_alert() {
    let iq = place(
        &[modulate(&canonical(OFF_AIR_RA), 64, CHANNEL_RATE, 0.0, 0.5)],
        4_000,
    );
    let mut channel = IridiumChannel::new(
        ChannelCtx {
            input_rate: CHANNEL_RATE,
        },
        settings(ChannelParams::Iridium(IridiumParams::default())),
    )
    .expect("channel");
    let events = run_events(&mut channel, &iq);
    assert!(events.iter().any(|event| matches!(
        event,
        DecoderEvent::Iridium(message) if message.message_type == "ring-alert"
            && message.crc_ok
            && message.frequency_error_hz.is_some_and(|hz| hz.abs() < 200.0)
    )));
}

#[test]
fn a_failed_data_crc_is_reported() {
    let frame = IridiumFrame::new(
        "ida",
        serde_json::json!({ "crc_ok": false, "bch_corrected": 2 }),
    );
    let message = super::message(&frame);
    assert!(!message.crc_ok);
    assert_eq!(message.fec_corrected, Some(2));
}

#[test]
fn demodulates_the_gr_iridium_reference_burst() {
    let mut iq = gr_reference_burst();
    iq.extend(std::iter::repeat_n(Complex::default(), 40_000));
    let mut decoder = ChannelDecoder::new();
    let mut bursts = Vec::new();
    decoder.demodulate(&iq, &mut bursts);
    assert!(!bursts.is_empty());
    let bits = symbol_reverse(&bursts[0].bits);
    assert_eq!(&bits[..24], &ACCESS_DL[..]);
    let payload = &bits[24..];
    assert!(payload.len() >= 300);
    let violations = (15..payload.len())
        .filter(|&i| payload[i] != (payload[i - 15] ^ payload[i - 14]))
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn ira_rf_loopback() {
    let bits = ira_bits(&ira_payload(
        99,
        7,
        [500, -900, 1300],
        &[0x12345678, 0x0BADCAFE],
    ));
    let mut iq = place(&[modulate(&bits, 64, CHANNEL_RATE, 1_500.0, 0.5)], 4_000);
    add_noise(&mut iq, 0.006, 7);
    let frames = ours(&iq);
    let (kind, details, _) = frames.first().expect("burst decodes");
    assert_eq!(kind, "ring-alert");
    assert_eq!(details["sat"], 99);
    assert_eq!(details["beam"], 7);
    assert_eq!(details["pages"][0]["tmsi"], "12345678");
    assert_eq!(details["pages"][1]["tmsi"], "0badcafe");
}

#[test]
fn oracle_validated_ring_alert() {
    let f = decode_bits(&bits_of_str(ORACLE_RA)).expect("decodes");
    assert_eq!(f.kind, "ring-alert");
    let d = &f.details;
    assert_eq!(d["sat"], 75);
    assert_eq!(d["beam"], 21);
    assert_eq!(d["ra_interval"], 33);
    assert_eq!(d["timeslot"], 0);
    assert_eq!(d["epi"], 1);
    assert_eq!(d["bc_sub_band"], 22);
    assert!((d["lat"].as_f64().unwrap_or_default() - 48.14).abs() < 0.01);
    assert!((d["lon"].as_f64().unwrap_or_default() - 169.44).abs() < 0.01);
    assert!((d["alt_km"].as_f64().unwrap_or_default() - 2248.7).abs() < 0.5);
    assert_eq!(d["pages"][0]["tmsi"], "cafed00d");
    assert_eq!(d["pages"][1]["tmsi"], "00c0ffee");
    assert_eq!(d["pages"][0]["msc_id"], 7);
}

#[test]
fn ims_pager_decodes_and_completes() {
    let frames = decode_burst(&ims_bits(&pager_blocks(1_234_567, "CALL OPS +14155550100")));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].kind, "msg-complete");
    assert_eq!(frames[0].details["text"], "CALL OPS +14155550100");
    assert_eq!(frames[1].kind, "msg");
    assert_eq!(
        frames[1].details.pointer("/body/ric"),
        Some(&Value::from(1_234_567))
    );
}

#[test]
fn oracle_validated_ims_vector() {
    let f = decode_bits(&bits_of_str(ORACLE_IMS)).expect("frame");
    assert_eq!(f.kind, "msg");
    let d = &f.details;
    assert_eq!(d["block"], 3);
    assert_eq!(d["group"], "1");
    assert_eq!(d["frame"], 9);
    assert_eq!(d["body"]["ric"], 1_234_567);
    assert_eq!(d["body"]["format"], 5);
    assert_eq!(d["body"]["seq"], 7);
    assert_eq!(d["body"]["text"], "CALL OPS +14155550100");
}

#[test]
fn itl_oracle_decodes_sat_plane_msg() {
    let f = decode_bits(&canonical(ITL_ORACLE)).expect("decodes");
    assert_eq!(f.kind, "itl");
    let d = &f.details;
    assert_eq!(d["type"], "time-location");
    assert_eq!(d["version"], 2);
    assert_eq!(d["plane"], 2);
    assert_eq!(d["sat"], "S09");
    assert_eq!(d["msg_type"], "M04");
}

#[test]
fn sync_byte_mismatch_matches_toolkit() {
    let clean = single(&sync_burst_bits(&[0xAA; 39]));
    assert_eq!(clean.kind, "sync");
    assert_eq!(clean.details["sync_errors"], 0);
    assert_eq!(clean.details["sync_idle"], true);
    let mut dirty = [0xAAu8; 39];
    dirty[0] = 0x00;
    dirty[10] = 0xFF;
    dirty[38] = 0x55;
    let f = single(&sync_burst_bits(&dirty));
    assert_eq!(f.details["sync_errors"], 3);
    assert_eq!(f.details["sync_idle"], false);
}

#[test]
fn da_roundtrip() {
    let payload: [u8; 20] = std::array::from_fn(|i| i as u8 * 7 + 3);
    let f = decode_burst(&da_burst_bits(true, 3, 20, &payload));
    assert_eq!(f[0].kind, "ida");
    assert_eq!(f[0].details["cont"], true);
    assert_eq!(f[0].details["ctr"], 3);
    assert_eq!(f[0].details["crc_ok"], true);
    assert_eq!(
        f[0].details["data_hex"],
        crate::datalink::hex(&payload).as_str()
    );
}

#[test]
fn da_decodes_with_lcw_bit_errors() {
    let payload: [u8; 20] = std::array::from_fn(|i| i as u8 * 5 + 1);
    let mut bits = da_burst_bits(false, 1, 12, &payload);
    bits[24 + 2] ^= 1;
    bits[24 + 40] ^= 1;
    let f = &decode_burst(&bits)[0];
    assert_eq!(f.kind, "ida");
    assert_eq!(f.details["ctr"], 1);
    assert_eq!(f.details["len"], 12);
    assert_eq!(f.details["crc_ok"], true);
}

#[test]
fn sbd_acars_end_to_end() {
    let mut reassembly = Reassembly::new();
    let mut frames = Vec::new();
    for (i, bits) in da_fragments(&acars_l2("N321AB", "UA1234"))
        .iter()
        .enumerate()
    {
        reassembly.handle(bits, &[], i as f64 * 0.09, 0.0, &mut frames);
    }
    let block = frames
        .iter()
        .find_map(|f| f.acars.as_ref())
        .expect("ACARS extracted");
    assert!(block.crc_ok);
    assert_eq!(block.core.tail.as_deref(), Some("N321AB"));
    assert_eq!(block.core.label, "Q0");
    assert_eq!(block.core.flight.as_deref(), Some("UA1234"));
}

#[test]
fn reassembles_interleaved_channels() {
    let a = da_fragments(&acars_l2("N321AB", "UA1234"));
    let b = da_fragments(&acars_l2("N555CD", "DL9999"));
    let mut reassembly = Reassembly::new();
    let mut frames = Vec::new();
    for i in 0..a.len().max(b.len()) {
        let t = i as f64 * 0.1;
        if let Some(bits) = a.get(i) {
            reassembly.handle(bits, &[], t, 100_000.0, &mut frames);
        }
        if let Some(bits) = b.get(i) {
            reassembly.handle(bits, &[], t, 200_000.0, &mut frames);
        }
    }
    let flights: Vec<_> = frames
        .iter()
        .filter_map(|f| f.acars.as_ref()?.core.flight.clone())
        .collect();
    assert_eq!(flights, ["UA1234", "DL9999"]);
}

#[test]
fn oracle_validated_da_vector() {
    let f = &decode_burst(&bits_of_str(ORACLE_DA))[0];
    assert_eq!(f.kind, "ida");
    assert_eq!(f.details["cont"], false);
    assert_eq!(f.details["ctr"], 0);
    assert_eq!(f.details["len"], 20);
    assert_eq!(f.details["crc_ok"], true);
    assert_eq!(
        f.details["data_hex"],
        "05101b26313c47525d68737e89949faab5c0cbd6"
    );
}

#[test]
fn offair_ring_alert_matches_toolkit() {
    let f = decode_bits(&canonical(OFF_AIR_RA)).expect("decodes");
    let d = &f.details;
    assert_eq!(d["sat"], 44);
    assert_eq!(d["beam"], 25);
    assert_eq!(d["ra_interval"], 48);
    assert_eq!(d["bc_sub_band"], 23);
    assert!((d["lat"].as_f64().unwrap_or_default() - 40.11).abs() < 0.01);
    assert!((d["lon"].as_f64().unwrap_or_default() + 127.29).abs() < 0.01);
    assert_eq!(d["pages"][0]["tmsi"], "071ca54a");
}

#[test]
fn offair_ida_sbd_decodes_with_crc() {
    let f = &decode_burst(&canonical(OFF_AIR_IDA))[0];
    assert_eq!(f.kind, "ida");
    assert_eq!(f.details["crc_ok"], true);
    assert_eq!(f.details["ctr"], 0);
    assert_eq!(f.details["cont"], false);
}

#[test]
fn offair_ibc_matches_toolkit() {
    let f = decode_bits(&canonical(OFF_AIR_IBC)).expect("IBC decodes");
    assert_eq!(f.kind, "broadcast");
    let d = &f.details;
    assert_eq!(d["bc_type"], 0);
    assert_eq!(d["sat"], 13);
    assert_eq!(d["beam"], 15);
    assert_eq!(d["acq_classes"], 65535);
    assert_eq!(d["acq_sub_band"], 20);
    assert_eq!(d["acq_channels"], 2);
    assert_eq!(d["info_type"], 0);
    assert_eq!(d["max_uplink_pwr"], 20);
    assert!(d.get("block_trailer").is_none());
    let a = &d["assignments"];
    assert_eq!(a.as_array().map(Vec::len), Some(2));
    assert_eq!(a[0]["random_id"], 153);
    assert_eq!(a[0]["timeslot"], 4);
    assert_eq!(a[0]["uplink_sub_band"], 31);
    assert_eq!(a[0]["downlink_sub_band"], 22);
}

#[test]
fn offair_ibc_tmsi_expiry_time() {
    let f = decode_bits(&canonical(OFF_AIR_IBC_EXPIRY)).expect("IBC decodes");
    assert_eq!(f.details["info_type"], 2);
    assert_eq!(f.details["tmsi_expiry"], 32768);
    let unix = f.details["tmsi_expiry_unix"].as_f64().unwrap_or_default();
    let offset = 32768.0 * 0.09;
    let bases = [
        1_399_821_184.12f64,
        1_739_491_200.0 + offset,
        1_768_414_080.0 + offset,
    ];
    assert!(bases.iter().any(|&b| (unix - b).abs() < 1.0));
}

#[test]
fn offair_u3_lcw_handoff() {
    let f = single(&canonical(OFF_AIR_U3));
    assert_eq!(f.kind, "u3");
    assert_eq!(f.details["frame_ft"], 3);
    assert_eq!(f.details["lcw"]["type"], "hndof");
    let code = &f.details["lcw"]["code"];
    assert_eq!(code["code"], "handoff_cand");
    assert_eq!(code["cand_a"], 0x34c);
    assert_eq!(code["cand_b"], 0x120);
    assert_eq!(f.details["u3_type"], "IU3");
}

#[test]
fn idle_bursts_are_dropped() {
    let mut bits = ACCESS_DL.to_vec();
    bits.extend([0u8; 400]);
    assert!(decode_burst(&bits).is_empty());
}

#[test]
fn every_traffic_class_matches_xng_bit_for_bit() {
    for bits in traffic_bursts() {
        let mut expected: Vec<Snapshot> = Vec::new();
        if let Some(f) = xng_mode_iridium::decode_bits(&bits) {
            expected.push(xng_snapshot(&f));
        } else if let Some(f) = xng_mode_iridium::lcw_traffic_frame(&bits) {
            expected.push(xng_snapshot(&f));
        } else if let Some((da, _)) = xng_mode_iridium::decode_da_bits(&bits) {
            assert_eq!(decode_burst(&bits)[0].details["crc_ok"], da.crc_ok);
            continue;
        }
        let got: Vec<Snapshot> = decode_burst(&bits)
            .iter()
            .filter(|f| f.kind != "msg-complete")
            .map(snapshot)
            .collect();
        assert_eq!(got, expected);
    }
}

fn identity(frame: &Snapshot) -> (String, Value) {
    let (kind, details, acars) = frame;
    let fields: &[&str] = match kind.as_str() {
        "ring-alert" => &["sat", "beam", "x", "y", "z"],
        "broadcast" => &["bc_type", "sat", "beam"],
        "msg" => &["block", "frame", "group"],
        "itl" => &["version", "sat", "plane"],
        "ida" => &["data_hex", "crc_ok", "len"],
        "voice" | "ip-data" | "sync" | "u3" | "u6" | "lcw" => &["lcw"],
        _ => return (kind.clone(), serde_json::json!([details, acars])),
    };
    let picked: serde_json::Map<String, Value> = fields
        .iter()
        .map(|&field| (field.to_owned(), details[field].clone()))
        .collect();
    (kind.clone(), Value::Object(picked))
}

fn assert_superset(ours: &[Snapshot], xng: &[Snapshot]) {
    let mut remaining: Vec<(String, Value)> = ours.iter().map(identity).collect();
    for frame in xng.iter().map(identity) {
        let at = remaining.iter().position(|candidate| *candidate == frame);
        assert!(at.is_some(), "xng frame missing: {frame:?}");
        if let Some(at) = at {
            remaining.remove(at);
        }
    }
}

#[test]
fn channel_decoder_finds_every_xng_frame_in_modulated_traffic() {
    let iq = modulated_stream(&traffic_bursts(), 0.02, 11);
    let expected = xng(&iq);
    assert!(expected.len() >= 12, "xng decoded {}", expected.len());
    let got = ours(&iq);
    assert_superset(&got, &expected);
    assert!(got.iter().any(|f| f.0 == "itl" && f.1["sat"] == "S09"));
}

#[test]
fn channel_decoder_finds_every_xng_frame_in_weak_traffic() {
    let iq = modulated_stream(&traffic_bursts(), 0.06, 13);
    let expected = xng(&iq);
    let got = ours(&iq);
    assert_superset(&got, &expected);
    assert!(
        got.len() > expected.len(),
        "ours {} xng {}",
        got.len(),
        expected.len()
    );
}

#[test]
fn channel_decoder_matches_xng_on_the_reference_burst() {
    let mut iq = gr_reference_burst();
    iq.extend(std::iter::repeat_n(Complex::default(), 40_000));
    assert_eq!(ours(&iq), xng(&iq));
}

#[test]
fn channel_decoder_stays_silent_on_noise() {
    let mut iq = vec![Complex::default(); 2_000_000];
    add_noise(&mut iq, 0.05, 3);
    assert!(xng(&iq).is_empty());
    assert!(ours(&iq).is_empty());
}
