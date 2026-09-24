use num_complex::Complex;
use sdrmm_wire::{
    ChannelParams, DataLinkMessage, DecoderEvent, InmarsatAeroParams,
};
use serde_json::Value;

use super::{
    InmarsatAeroChannel,
    acars_block,
    decoder::{AeroChannelDecoder, AeroEvent, INPUT_RATE},
    frame::FrameEncoder,
    framer::Framer,
    modulate::modulate,
    oqpsk::{self, HrFramer, hr_frame_bits, modulate_oqpsk},
    su,
};
use crate::{
    ChannelCtx, ChannelOutputs, ChannelRx,
    datalink::{self, Quality},
    testutil::settings,
};

const ADSC_TEXT: &str =
    "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5";
const CHUNKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

pub(super) struct Noise(pub u64);

impl Noise {
    pub(super) fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as f32 / u64::MAX as f32) * 2.0 - 1.0
    }

    pub(super) fn add(&mut self, iq: &mut [Complex<f32>], amplitude: f32) {
        for sample in iq {
            *sample += Complex::new(self.next() * amplitude, self.next() * amplitude);
        }
    }
}

pub(super) fn acars_user() -> Vec<u8> {
    let mut user = vec![0xFF, 0xFF];
    user.extend(acars_block::build(
        '2', "VT-ANB", None, "B6", 'A', None, None, ADSC_TEXT, false,
    ));
    user
}

fn frames_bits(rate: u32, units: &[Vec<u8>], frames: usize) -> Vec<u8> {
    let mut encoder = FrameEncoder::new(rate);
    let mut bits: Vec<u8> = (0..160).map(|index| (index % 2) as u8).collect();
    let mut padded = units.to_vec();
    while padded.len() < frames * 6 || !padded.len().is_multiple_of(6) {
        padded.push(su::fill_su());
    }
    for (index, chunk) in padded.chunks(6).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flatten().copied().collect();
        bits.extend(encoder.encode(&bytes, index as u8));
    }
    bits.extend((0..64).map(|index| (index % 2) as u8));
    bits
}

pub(super) fn p_channel_bits(rate: u32) -> Vec<u8> {
    frames_bits(rate, &su::build_isu_chain(0xA1B2C3, 0x44, 1, 7, &acars_user()), 1)
}

fn control_bits(su10: Vec<u8>) -> Vec<u8> {
    frames_bits(600, &[su::su_with_crc(su10)], 2)
}

pub(super) fn high_rate_bits() -> Vec<u8> {
    let mut units = su::build_isu_chain(0xA1B2C3, 0x44, 1, 7, &acars_user());
    while !units.len().is_multiple_of(26) {
        units.push(su::fill_su());
    }
    let mut encoder = FrameEncoder::new(oqpsk::BIT_RATE);
    let mut idle = 0x1234_5678_9abc_def0u64;
    let mut bits: Vec<u8> = (0..18_000)
        .map(|_| {
            idle = idle
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((idle >> 33) & 1) as u8
        })
        .collect();
    for (index, chunk) in units.chunks(26).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flatten().copied().collect();
        bits.extend(hr_frame_bits(&mut encoder, &bytes, index as u8));
    }
    bits.extend((0..400).map(|index| (index % 2) as u8));
    bits
}

fn msk(bits: &[u8], rate: u32, cfo: f64, noise: f32, seed: u64) -> Vec<Complex<f32>> {
    let mut iq = modulate(bits, f64::from(rate), INPUT_RATE, cfo, 0.5);
    Noise(seed).add(&mut iq, noise);
    iq
}

fn decoder_events(iq: &[Complex<f32>]) -> Vec<AeroEvent> {
    let mut decoder = AeroChannelDecoder::new();
    let mut events = Vec::new();
    for chunk in iq.chunks(4096) {
        decoder.process(chunk, &mut events);
    }
    events
}

fn chunked(iq: &[Complex<f32>]) -> impl Iterator<Item = &[Complex<f32>]> {
    let mut position = 0;
    CHUNKS.iter().cycle().map_while(move |&length| {
        if position >= iq.len() {
            return None;
        }
        let end = (position + length).min(iq.len());
        let chunk = &iq[position..end];
        position = end;
        Some(chunk)
    })
}

pub(super) fn ours(iq: &[Complex<f32>]) -> Vec<DataLinkMessage> {
    let mut channel = InmarsatAeroChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE,
        },
        settings(ChannelParams::InmarsatAero(InmarsatAeroParams::default())),
    )
    .expect("channel");
    let mut out = ChannelOutputs::default();
    let mut messages = Vec::new();
    for chunk in chunked(iq) {
        out.reset();
        channel.process(chunk, &mut out);
        messages.extend(out.events.drain(..).filter_map(|event| match event {
            DecoderEvent::InmarsatAero(message) => Some(message),
            _ => None,
        }));
    }
    messages
}

pub(super) fn reference(iq: &[Complex<f32>]) -> Vec<DataLinkMessage> {
    let mut decoder = xng_mode_aero::AeroChannelDecoder::new(INPUT_RATE, 0.0).expect("xng");
    let source = xng_types::Provenance {
        station: xng_types::StationIdentity::new("SDR--"),
        app: xng_types::AppInfo {
            name: "SDR--".to_owned(),
            version: String::new(),
        },
        sdr: None,
        channel: None,
    };
    let mut messages = Vec::new();
    for chunk in chunked(iq) {
        for event in decoder.process(chunk) {
            let message = xng_mode_aero::to_message(&event, 0, 0.0, source.clone());
            messages.push(datalink::message(
                &message.body,
                Quality {
                    crc_ok: message.decode.crc_ok,
                    fec_corrected: message.decode.fec_corrected,
                    snr_db: message.signal.snr_db,
                    frequency_error_hz: message.signal.freq_skew_hz,
                },
                message.raw.as_deref(),
            ));
        }
    }
    messages
}

fn assert_equivalent(iq: &[Complex<f32>]) -> Vec<DataLinkMessage> {
    let ours = ours(iq);
    assert_eq!(ours, reference(iq));
    ours
}

fn su_event<'a>(events: &'a [AeroEvent], su_type: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| {
            event
                .su_event
                .as_ref()
                .filter(|value| value["su_type"] == su_type)
        })
        .unwrap_or_else(|| panic!("{su_type} decoded"))
}

fn addressed(type_byte: u8, aes: [u8; 3], ges: u8) -> Vec<u8> {
    let mut su10 = vec![0u8; 10];
    su10[0] = type_byte;
    su10[1..4].copy_from_slice(&aes);
    su10[4] = ges;
    su10
}

#[test]
fn decodes_acars_at_600_and_1200_bps() {
    for (rate, cfo) in [(600u32, 30.0), (1200, -45.0)] {
        let events = decoder_events(&msk(&p_channel_bits(rate), rate, cfo, 0.02, 7));
        let event = events
            .iter()
            .find(|event| event.acars.is_some())
            .expect("ACARS event");
        assert_eq!(event.bit_rate, rate);
        assert_eq!(event.user.aes_id, "A1B2C3");
        let block = event.acars.as_ref().expect("block");
        assert!(block.crc_ok);
        assert_eq!(block.core.tail.as_deref(), Some("VT-ANB"));
        assert_eq!(block.core.label, "B6");
    }
}

#[test]
fn channel_reports_acars_as_a_datalink_message() {
    let messages = ours(&msk(&p_channel_bits(600), 600, 0.0, 0.0, 1));
    let message = messages
        .iter()
        .find(|message| message.message_type == "acars")
        .expect("ACARS message");
    assert!(message.crc_ok);
    assert_eq!(message.station.as_deref(), Some("VT-ANB"));
    assert_eq!(message.text.as_deref(), Some(ADSC_TEXT));
    assert_eq!(message.fec_corrected, Some(0));
}

#[test]
fn control_units_decode_end_to_end() {
    let events = decoder_events(&msk(
        &control_bits(addressed(0x11, [0xC0, 0xFF, 0xEE], 0x05)),
        600,
        30.0,
        0.02,
        3,
    ));
    let log_on = su_event(&events, "log-control");
    assert_eq!(log_on["event"], "log-on-confirm");
    assert_eq!(log_on["aes_id"], "C0FFEE");
    let mut announcement = addressed(0x21, [0xA1, 0xB2, 0xC3], 0x44);
    announcement[6..10].copy_from_slice(&[0x0F, 0xA0, 0x07, 0xD0]);
    let events = decoder_events(&msk(&control_bits(announcement), 600, 30.0, 0.02, 3));
    let value = su_event(&events, "call-announcement");
    assert_eq!(value["receive_mhz"], 4000.0 * 0.0025 + 1510.0);
    assert_eq!(value["transmit_mhz"], 2000.0 * 0.0025 + 1611.5);
}

#[test]
fn satellite_and_frame_header_reach_the_message() {
    let mut su10 = vec![0u8; 10];
    su10[0] = 0x0C;
    su10[2] = 0x29;
    su10[3] = 0x40;
    su10[5] = 200;
    su10[6] = 0x01;
    su10[7] = 0x23;
    let messages = ours(&msk(&control_bits(su10), 600, 30.0, 0.02, 5));
    let details = &messages
        .iter()
        .find(|message| message.message_type == "satellite-id")
        .expect("satellite-id message")
        .details["details"];
    assert_eq!(details["resolved_satellite"]["satellite_id"], 20);
    assert_eq!(details["resolved_satellite"]["region"], "AOR-W");
    assert_eq!(details["beam"], "global");
    assert_eq!(details["channel"], "p-channel");
    assert_eq!(details["line_bit_rate"], 600);
    assert_eq!(details["frame_header"]["format_id"], 1);
    assert_eq!(details["superframe_lock"]["carrier_state"], "searching");
}

#[test]
fn high_rate_framing_at_bit_level() {
    let bits = high_rate_bits();
    for inverted_rail in [false, true] {
        let mut framer = HrFramer::new();
        let mut users = Vec::new();
        for (index, &bit) in bits.iter().enumerate() {
            let bit = bit ^ u8::from(inverted_rail && index % 2 == 0);
            framer.push(if bit == 1 { 1.0 } else { -1.0 }, bit, &mut users);
        }
        let user = users.first().expect("user data reassembles");
        let block = su::parse_acars(&user.data).expect("ACARS parses");
        assert!(block.crc_ok);
        assert_eq!(block.core.tail.as_deref(), Some("VT-ANB"));
    }
}

#[test]
fn low_rate_framer_tolerates_two_uw_errors() {
    let mut bits = p_channel_bits(1200);
    bits[160] ^= 1;
    bits[170] ^= 1;
    let mut framer = Framer::new(1200);
    let mut users = Vec::new();
    for &bit in &bits {
        framer.push(if bit == 1 { 1.0 } else { -1.0 }, bit, &mut users);
    }
    assert_eq!(users.len(), 1);
}

#[test]
fn decodes_acars_at_10500_bps() {
    let mut iq = modulate_oqpsk(&high_rate_bits(), oqpsk::CHANNEL_RATE_HR, 120.0, 0.5);
    Noise(0xaa55_1234_9999_0001).add(&mut iq, 0.02);
    let events = decoder_events(&iq);
    let event = events
        .iter()
        .find(|event| event.acars.is_some())
        .expect("ACARS at 10.5k");
    assert_eq!(event.bit_rate, 10_500);
    assert!(event.acars.as_ref().is_some_and(|block| block.crc_ok));
}

#[test]
fn matches_xng_on_low_rate_acars() {
    for (rate, cfo) in [(600u32, 30.0), (1200, -45.0)] {
        let messages = assert_equivalent(&msk(&p_channel_bits(rate), rate, cfo, 0.02, 11));
        assert!(messages.iter().any(|message| message.message_type == "acars"));
    }
}

#[test]
fn matches_xng_on_control_units() {
    for type_byte in [0x0C, 0x11, 0x21, 0x28, 0x40, 0x51, 0x05, 0x74] {
        let mut su10 = addressed(type_byte, [0x12, 0x34, 0x56], 0x2A);
        su10[5..10].copy_from_slice(&[200, 0x81, 0x23, 0x10, 0x01]);
        let messages = assert_equivalent(&msk(&control_bits(su10), 600, 30.0, 0.02, 13));
        assert!(!messages.is_empty(), "type 0x{type_byte:02X}");
    }
}

#[test]
fn matches_xng_on_high_rate_acars() {
    let mut iq = modulate_oqpsk(&high_rate_bits(), oqpsk::CHANNEL_RATE_HR, 120.0, 0.5);
    Noise(0xaa55_1234_9999_0001).add(&mut iq, 0.02);
    let messages = assert_equivalent(&iq);
    assert!(messages.iter().any(|message| message.message_type == "acars"));
}

pub(super) fn offair() -> Vec<Complex<f32>> {
    const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/inmarsat_aero_offair_48k.sigmf-data");
    FIXTURE
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[i0, i1, q0, q1]| {
            Complex::new(
                f32::from(i16::from_le_bytes([i0, i1])) / 32768.0,
                f32::from(i16::from_le_bytes([q0, q1])) / 32768.0,
            )
        })
        .collect()
}

#[test]
fn decodes_the_recorded_600_bps_channel() {
    let messages = assert_equivalent(&offair());
    let message = messages
        .iter()
        .find(|message| message.message_type == "acars")
        .expect("off air ACARS");
    assert!(message.crc_ok);
    assert_eq!(message.station.as_deref(), Some("HL8217"));
}

#[test]
fn matches_xng_on_noise() {
    let mut iq = vec![Complex::new(0.0, 0.0); 96_000];
    Noise(99).add(&mut iq, 0.3);
    assert_equivalent(&iq);
}
