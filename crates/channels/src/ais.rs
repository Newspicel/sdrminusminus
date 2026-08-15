use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, HdlcDeframer, NrziDecoder, bits_be, design_lowpass, hdlc_fcs_ok, reverse_byte,
};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, Mapping, TIMING_BW_BURST},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    AisMessage, AisParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

const BAUD: f64 = 9_600.0;
const DEVIATION_HZ: f64 = 2_400.0;
const BT: f64 = 0.4;
const PULSE_SPAN: usize = 5;
const MATCHED_SPAN: usize = 3;

const MIN_FRAME_BYTES: usize = 13;
const MAX_FRAME_BYTES: usize = 128;

const SOG_UNAVAILABLE: u64 = 1_023;
const COG_INVALID: u64 = 3_600;
const HEADING_UNAVAILABLE: u16 = 511;
const COORD_UNITS_PER_DEGREE: f64 = 600_000.0;

const MAX_PAYLOAD_CHARS: usize = 60;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ais".to_owned(),
    name: "AIS".to_owned(),
    bandwidth_hz: 25_000.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("ais".to_owned()),
    ..ChannelDescriptor::default()
});

const CARRIER_TAU_SAMPLES: f64 = 200.0;

pub struct AisChannelRx {
    letter: char,
    demod: CpmDemod,
    slicer: Mapping,
    nrzi: NrziDecoder,
    deframer: HdlcDeframer,
    carrier_acc: Complex<f32>,
    carrier_alpha: f32,
    carrier_prev: Complex<f32>,
    carrier_phase: f64,
    mixed: Vec<Complex<f32>>,
    soft: Vec<f32>,
    msg: Vec<u8>,
    seq: u8,
}

fn cpm_params(sps: f64) -> CpmParams {
    CpmParams::from_deviation(
        Mapping::natural(2),
        DEVIATION_HZ,
        BAUD,
        pulse::gaussian_freq(sps, BT, PULSE_SPAN, Norm::Area),
        sps,
    )
}

fn receive_filter(sps: f64) -> Vec<f32> {
    let mut taps = pulse::gaussian(sps, BT, MATCHED_SPAN, Norm::Area);
    taps.resize(CHANNEL_TAPS / 2 + taps.len(), 0.0);
    taps
}

fn params(settings: &ChannelSettings) -> Result<&AisParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ais(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "ais channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(_p: &AisParams) -> Result<(), ChannelError> {
    Ok(())
}

pub(crate) fn occupied_band(_p: &AisParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &AisParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

impl ChannelRx for AisChannelRx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        let sps = ctx.input_rate / BAUD;
        let cpm = cpm_params(sps);
        Ok(Self {
            letter: p.ais_channel.letter(),
            demod: CpmDemod::new(&cpm, &receive_filter(sps), TIMING_BW_BURST),
            slicer: cpm.mapping().clone(),
            nrzi: NrziDecoder::new(),
            deframer: HdlcDeframer::new(MIN_FRAME_BYTES, MAX_FRAME_BYTES),
            carrier_acc: Complex::new(0.0, 0.0),
            carrier_alpha: (1.0 / CARRIER_TAU_SAMPLES) as f32,
            carrier_prev: Complex::new(0.0, 0.0),
            carrier_phase: 0.0,
            mixed: Vec::new(),
            soft: Vec::new(),
            msg: Vec::with_capacity(MAX_FRAME_BYTES),
            seq: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.letter = p.ais_channel.letter();
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.mixed.clear();
        for &sample in iq {
            let rotation = sample * self.carrier_prev.conj();
            self.carrier_prev = sample;
            self.carrier_acc += self.carrier_alpha * (rotation - self.carrier_acc);
            self.carrier_phase -= f64::from(self.carrier_acc.arg());
            self.carrier_phase = self.carrier_phase.rem_euclid(std::f64::consts::TAU);
            self.mixed
                .push(sample * Complex::from_polar(1.0, self.carrier_phase as f32));
        }
        self.soft.clear();
        self.demod.process(&self.mixed, &mut self.soft);
        for &symbol in &self.soft {
            let level = self.slicer.slice(symbol) == 1;
            let Some(frame) = self.deframer.push(self.nrzi.decode(level)) else {
                continue;
            };
            if !hdlc_fcs_ok(&frame) {
                continue;
            }
            let Some((payload, _fcs)) = frame.split_last_chunk::<2>() else {
                continue;
            };
            self.msg.clear();
            self.msg.extend(payload.iter().copied().map(reverse_byte));
            let bits = self.msg.len() * 8;
            let fill = (6 - bits % 6) % 6;
            let nmea = sentences(&armour(&self.msg, bits), fill, self.letter, &mut self.seq);
            if let Some(message) = decode(&self.msg, bits, self.letter, nmea) {
                out.events.push(DecoderEvent::Ais(message));
            }
        }
    }
}

struct PositionLayout {
    sog: usize,
    lon: usize,
    lat: usize,
    cog: usize,
    heading: usize,
}

const CLASS_A_POSITION: PositionLayout = PositionLayout {
    sog: 50,
    lon: 61,
    lat: 89,
    cog: 116,
    heading: 128,
};

const CLASS_B_POSITION: PositionLayout = PositionLayout {
    sog: 46,
    lon: 57,
    lat: 85,
    cog: 112,
    heading: 124,
};

const HEADER_BITS: usize = 38;

fn decode(msg: &[u8], bits: usize, letter: char, nmea: String) -> Option<AisMessage> {
    if bits < HEADER_BITS {
        return None;
    }
    let mut out = AisMessage {
        mmsi: bits_be(msg, 8, 30) as u32,
        msg_type: bits_be(msg, 0, 6) as u8,
        ais_channel: letter,
        nmea,
        ..AisMessage::default()
    };
    match out.msg_type {
        1..=3 if bits >= 168 => {
            out.nav_status = Some(bits_be(msg, 38, 4) as u8);
            read_position(msg, &CLASS_A_POSITION, &mut out);
        }
        18 if bits >= 168 => read_position(msg, &CLASS_B_POSITION, &mut out),
        5 if bits >= 424 => {
            out.call_sign = text(msg, 70, 7);
            out.name = text(msg, 112, 20);
            out.destination = text(msg, 302, 20);
        }
        24 if bits >= 160 => {
            if bits_be(msg, 38, 2) == 0 {
                out.name = text(msg, 40, 20);
            } else {
                out.call_sign = text(msg, 90, 7);
            }
        }
        _ => {}
    }
    Some(out)
}

fn read_position(msg: &[u8], layout: &PositionLayout, out: &mut AisMessage) {
    let sog = bits_be(msg, layout.sog, 10);
    out.sog_kt = (sog != SOG_UNAVAILABLE).then(|| sog as f64 / 10.0);
    out.lon = coord(signed(msg, layout.lon, 28), 180.0);
    out.lat = coord(signed(msg, layout.lat, 27), 90.0);
    let cog = bits_be(msg, layout.cog, 12);
    out.cog_deg = (cog < COG_INVALID).then(|| cog as f64 / 10.0);
    let heading = bits_be(msg, layout.heading, 9) as u16;
    out.heading_deg = (heading != HEADING_UNAVAILABLE).then_some(heading);
}

fn signed(msg: &[u8], offset: usize, len: usize) -> i64 {
    let raw = bits_be(msg, offset, len);
    if raw >> (len - 1) & 1 == 1 {
        raw as i64 - (1i64 << len)
    } else {
        raw as i64
    }
}

fn coord(raw: i64, limit_deg: f64) -> Option<f64> {
    let deg = raw as f64 / COORD_UNITS_PER_DEGREE;
    (deg.abs() <= limit_deg).then_some(deg)
}

fn text(msg: &[u8], offset: usize, chars: usize) -> Option<String> {
    let mut s = String::with_capacity(chars);
    for i in 0..chars {
        let v = bits_be(msg, offset + i * 6, 6) as u8;
        s.push(char::from(if v < 32 { v + 64 } else { v }));
    }
    let trimmed = s.trim_end_matches(['@', ' ']);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn armour(msg: &[u8], bits: usize) -> String {
    (0..bits.div_ceil(6))
        .map(|group| {
            let offset = group * 6;
            let take = 6.min(bits - offset);
            let value = (bits_be(msg, offset, take) << (6 - take)) as u8;
            let c = value + 48;
            char::from(if c > 87 { c + 8 } else { c })
        })
        .collect()
}

fn sentences(payload: &str, fill: usize, letter: char, seq: &mut u8) -> String {
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(MAX_PAYLOAD_CHARS).collect();
    let total = chunks.len();
    let id = if total > 1 {
        let id = *seq;
        *seq = (*seq + 1) % 10;
        id.to_string()
    } else {
        String::new()
    };

    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let text: String = chunk.iter().copied().map(char::from).collect();
        let pad = if i + 1 == total { fill } else { 0 };
        let body = format!("AIVDM,{total},{},{id},{letter},{text},{pad}", i + 1);
        let checksum = body.bytes().fold(0u8, |acc, b| acc ^ b);
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("!{body}*{checksum:02X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AisChannel, AmParams};

    use super::*;
    use crate::{
        testgen::{
            add_noise,
            ais::{
                PositionReport, burst, class_b_payload, corrupted_burst, position_payload,
                static_data_payloads, static_payload,
            },
            shift, silence,
        },
        testutil::settings,
    };

    const RATE: f64 = 48_000.0;

    fn ais_settings(channel: AisChannel) -> ChannelSettings {
        settings(ChannelParams::Ais(AisParams {
            ais_channel: channel,
        }))
    }

    fn channel(ais_channel: AisChannel) -> AisChannelRx {
        AisChannelRx::new(ChannelCtx { input_rate: RATE }, ais_settings(ais_channel)).unwrap()
    }

    fn report() -> PositionReport {
        PositionReport {
            mmsi: 244_670_316,
            lat: 52.372_5,
            lon: 4.893_2,
            sog_kt: 12.4,
            cog_deg: 187.3,
            heading_deg: 188,
            nav_status: 3,
        }
    }

    fn run_blocks(
        chan: &mut AisChannelRx,
        iq: &[Complex<f32>],
        sizes: &[usize],
    ) -> Vec<AisMessage> {
        let mut out = ChannelOutputs::default();
        let mut msgs = Vec::new();
        let mut pos = 0;
        for len in sizes.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "ais must not produce audio");
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Ais(m) => msgs.push(m),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        msgs
    }

    fn run(iq: &[Complex<f32>]) -> Vec<AisMessage> {
        run_blocks(
            &mut channel(AisChannel::A),
            iq,
            &[997, 1, 4_096, 65, 2_048, 7, 1_024],
        )
    }

    const LEAD_IN: usize = 2_400;

    const FLOOR: f32 = 0.01;

    fn framed(burst_iq: Vec<Complex<f32>>) -> Vec<Complex<f32>> {
        let mut iq = silence(LEAD_IN);
        iq.extend(burst_iq);
        iq.extend(silence(480));
        add_noise(&mut iq, 0x1157, FLOOR);
        iq
    }

    fn raw(payload: &[bool]) -> Vec<Complex<f32>> {
        framed(burst(payload, RATE))
    }

    fn select(iq: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut filter = crate::channel_filter(&ChannelParams::Ais(AisParams::default())).unwrap();
        let mut out = Vec::new();
        filter.process(iq, &mut out);
        out
    }

    fn transmission(payload: &[bool]) -> Vec<Complex<f32>> {
        select(&raw(payload))
    }

    fn only(msgs: Vec<AisMessage>) -> AisMessage {
        assert_eq!(msgs.len(), 1, "expected exactly one message: {msgs:?}");
        msgs.into_iter().next().unwrap()
    }

    #[test]
    fn acquires_from_every_burst_phase_at_every_dial_error() {
        for offset_hz in [0.0, -400.0, 400.0, -1_200.0, 1_200.0] {
            for phase in 0..5usize {
                let mut iq = silence(LEAD_IN + phase);
                iq.extend(burst(&position_payload(&report()), RATE));
                iq.extend(silence(480));
                add_noise(&mut iq, 0x1157, FLOOR);
                shift(&mut iq, offset_hz, RATE);
                let msgs = run_blocks(&mut channel(AisChannel::A), &select(&iq), &[997]);
                assert_eq!(
                    msgs.len(),
                    1,
                    "no decode at burst phase {phase}/5, {offset_hz:+.0} Hz"
                );
                assert_eq!(
                    msgs[0].mmsi, 244_670_316,
                    "phase {phase}, {offset_hz:+.0} Hz"
                );
            }
        }
    }

    #[test]
    fn decodes_a_type_1_position_report() {
        let m = only(run(&transmission(&position_payload(&report()))));
        assert_eq!(m.msg_type, 1);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.ais_channel, 'A');
        assert_eq!(m.nav_status, Some(3));
        assert_eq!(m.sog_kt, Some(12.4));
        assert_eq!(m.cog_deg, Some(187.3));
        assert_eq!(m.heading_deg, Some(188));
        assert!((m.lat.unwrap() - 52.372_5).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 4.893_2).abs() < 1e-4, "lon {:?}", m.lon);
    }

    #[test]
    fn decodes_a_type_18_class_b_report() {
        let m = only(run(&transmission(&class_b_payload(&report()))));
        assert_eq!(m.msg_type, 18);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.sog_kt, Some(12.4));
        assert_eq!(m.cog_deg, Some(187.3));
        assert_eq!(m.heading_deg, Some(188));
        assert!((m.lat.unwrap() - 52.372_5).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 4.893_2).abs() < 1e-4, "lon {:?}", m.lon);
        assert_eq!(m.nav_status, None);
    }

    #[test]
    fn decodes_a_type_5_static_report_with_the_padding_trimmed() {
        let payload = static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM");
        let m = only(run(&transmission(&payload)));
        assert_eq!(m.msg_type, 5);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.name.as_deref(), Some("NAUTICA"));
        assert_eq!(m.call_sign.as_deref(), Some("PBRT"));
        assert_eq!(m.destination.as_deref(), Some("ROTTERDAM"));
    }

    #[test]
    fn decodes_both_parts_of_a_type_24_static_data_report() {
        let (part_a, part_b) = static_data_payloads(244_670_316, "SEA WOLF", "PBRT");
        let mut iq = raw(&part_a);
        iq.extend(raw(&part_b));
        let msgs = run(&select(&iq));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].msg_type, 24);
        assert_eq!(msgs[0].name.as_deref(), Some("SEA WOLF"));
        assert_eq!(msgs[0].call_sign, None);
        assert_eq!(msgs[1].call_sign.as_deref(), Some("PBRT"));
        assert_eq!(msgs[1].name, None);
    }

    #[test]
    fn an_undecoded_message_type_still_reports_its_sender_and_sentence() {
        let mut payload = position_payload(&report());
        payload[..6].copy_from_slice(&[false, false, false, true, false, false]);
        let m = only(run(&transmission(&payload)));
        assert_eq!(m.msg_type, 4);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(
            (m.lat, m.lon, m.sog_kt, m.nav_status),
            (None, None, None, None)
        );
        assert!(checksum_ok(&m.nmea), "{}", m.nmea);
    }

    #[test]
    fn unavailable_sentinels_decode_to_none() {
        let m = only(run(&transmission(&position_payload(&PositionReport {
            mmsi: 1,
            lat: 91.0,
            lon: 181.0,
            sog_kt: 102.3,
            cog_deg: 360.0,
            heading_deg: 511,
            nav_status: 15,
        }))));
        assert_eq!(m.lat, None);
        assert_eq!(m.lon, None);
        assert_eq!(m.sog_kt, None);
        assert_eq!(m.cog_deg, None);
        assert_eq!(m.heading_deg, None);
    }

    #[test]
    fn a_payload_with_a_long_run_of_ones_survives_bit_stuffing() {
        let payload = position_payload(&PositionReport {
            mmsi: 268_435_455,
            ..report()
        });
        let longest = payload
            .chunk_by(|a, b| a == b)
            .filter(|run| run[0])
            .map(<[bool]>::len)
            .max()
            .unwrap_or(0);
        assert!(
            longest >= 6,
            "fixture must force stuffing, longest run {longest}"
        );

        let m = only(run(&transmission(&payload)));
        assert_eq!(m.mmsi, 268_435_455);
        assert_eq!(m.heading_deg, Some(188));
    }

    #[test]
    fn a_corrupted_burst_emits_nothing() {
        let payload = position_payload(&report());
        let iq = framed(corrupted_burst(&payload, 100, RATE));
        assert!(run(&select(&iq)).is_empty());
        assert_eq!(only(run(&transmission(&payload))).mmsi, 244_670_316);
    }

    #[test]
    fn two_bursts_back_to_back_both_decode() {
        let first = position_payload(&report());
        let second = position_payload(&PositionReport {
            mmsi: 211_234_567,
            sog_kt: 0.0,
            ..report()
        });
        let mut iq = raw(&first);
        iq.extend(raw(&second));
        let msgs = run(&select(&iq));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].mmsi, 244_670_316);
        assert_eq!(msgs[1].mmsi, 211_234_567);
        assert_eq!(msgs[1].sog_kt, Some(0.0));
    }

    #[test]
    fn ragged_block_splits_give_identical_results() {
        let iq = transmission(&static_payload(1_234_567, "SEA WOLF", "ZZ9", "HAMBURG"));
        let whole = run_blocks(&mut channel(AisChannel::A), &iq, &[iq.len()]);
        let ragged = run_blocks(
            &mut channel(AisChannel::A),
            &iq,
            &[1, 3, 512, 17, 4_096, 63],
        );
        assert_eq!(whole.len(), 1);
        assert_eq!(whole, ragged);
    }

    fn checksum_ok(sentence: &str) -> bool {
        let Some((body, tail)) = sentence.strip_prefix('!').and_then(|s| s.split_once('*')) else {
            return false;
        };
        let want = body.bytes().fold(0u8, |acc, b| acc ^ b);
        tail == format!("{want:02X}")
    }

    #[test]
    fn a_single_sentence_carries_the_channel_letter_and_a_valid_checksum() {
        let mut chan = channel(AisChannel::B);
        let msgs = run_blocks(
            &mut chan,
            &transmission(&position_payload(&report())),
            &[1_000],
        );
        let m = only(msgs);
        assert_eq!(m.ais_channel, 'B');
        assert!(checksum_ok(&m.nmea), "{}", m.nmea);
        assert!(m.nmea.starts_with("!AIVDM,1,1,,B,"), "{}", m.nmea);
        assert!(m.nmea.contains(",0*"), "{}", m.nmea);
    }

    fn dearmour_char(c: char) -> u8 {
        let v = c as u8 - 48;
        if v > 40 { v - 8 } else { v }
    }

    #[test]
    fn a_type_5_payload_splits_into_two_valid_sentences() {
        let payload = static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM");
        let m = only(run(&transmission(&payload)));

        let parts: Vec<&str> = m.nmea.split('\n').collect();
        assert_eq!(parts.len(), 2, "{}", m.nmea);
        let mut bits = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            assert!(checksum_ok(part), "{part}");
            let fields: Vec<&str> = part.split([',', '*']).collect();
            assert_eq!(fields[0], "!AIVDM");
            assert_eq!(fields[1], "2");
            assert_eq!(fields[2], (i + 1).to_string());
            assert_eq!(fields[3], "0", "sequential id");
            assert_eq!(fields[4], "A");
            assert_eq!(fields[6], if i == 0 { "0" } else { "2" });
            for c in fields[5].chars() {
                let v = dearmour_char(c);
                bits.extend((0..6).rev().map(|k| v >> k & 1 == 1));
            }
        }
        assert_eq!(bits.len(), 426);
        assert_eq!(&bits[..424], payload.as_slice());
        assert_eq!(&bits[424..], [false, false]);
    }

    #[test]
    fn decodes_through_a_receiver_frequency_error() {
        for offset_hz in [-1_200.0, -400.0, 400.0, 1_200.0] {
            let mut iq = raw(&position_payload(&report()));
            shift(&mut iq, offset_hz, RATE);
            assert_eq!(only(run(&select(&iq))).mmsi, 244_670_316, "{offset_hz} Hz");
        }
    }

    #[test]
    fn decodes_through_noise() {
        for seed in 1u32..=4 {
            let mut iq = raw(&static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM"));
            add_noise(&mut iq, seed, 0.2);
            let m = only(run(&select(&iq)));
            assert_eq!(m.name.as_deref(), Some("NAUTICA"), "seed {seed}");
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AisChannel::A);
        let err = chan.apply(settings(ChannelParams::Am(AmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = AisChannelRx::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Am(AmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn apply_switches_the_reported_channel_letter() {
        let mut chan = channel(AisChannel::A);
        chan.apply(ais_settings(AisChannel::B)).unwrap();
        let msgs = run_blocks(
            &mut chan,
            &transmission(&position_payload(&report())),
            &[512],
        );
        assert_eq!(only(msgs).ais_channel, 'B');
    }

    #[test]
    fn decodes_the_committed_fixture() {
        const FIXTURE: &[u8] =
            include_bytes!("../../../fixtures/ais_position_pre_cpm_240k.sigmf-data");
        const FIXTURE_RATE: f64 = 240_000.0;
        let mut wide: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                Complex::new(
                    f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
                )
            })
            .collect();
        shift(&mut wide, -25_000.0, FIXTURE_RATE);
        let decim = (FIXTURE_RATE / RATE) as usize;
        let mut narrow = Vec::new();
        Decimator::new(
            &design_lowpass(CHANNEL_TAPS, DESCRIPTOR.bandwidth_hz / 2.0 / FIXTURE_RATE),
            decim,
        )
        .process(&wide, &mut narrow);
        let m = only(run(&select(&framed(narrow))));
        assert_eq!(m.msg_type, 1);
        assert_eq!(m.mmsi, 211_234_560);
        assert_eq!(m.nav_status, Some(0));
        assert_eq!(m.sog_kt, Some(12.3));
        assert_eq!(m.cog_deg, Some(178.4));
        assert_eq!(m.heading_deg, Some(179));
        assert!((m.lat.unwrap() - 53.5413).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 9.9846).abs() < 1e-4, "lon {:?}", m.lon);
    }

    #[test]
    fn a_240k_rendering_decodes_through_the_ddc_path() {
        const DEVICE_RATE: f64 = 240_000.0;
        let mut wide = burst(&position_payload(&report()), DEVICE_RATE);
        shift(&mut wide, 25_000.0, DEVICE_RATE);
        shift(&mut wide, -25_000.0, DEVICE_RATE);
        let mut narrow = Vec::new();
        Decimator::new(
            &design_lowpass(CHANNEL_TAPS, DESCRIPTOR.bandwidth_hz / 2.0 / DEVICE_RATE),
            (DEVICE_RATE / RATE) as usize,
        )
        .process(&wide, &mut narrow);
        assert_eq!(only(run(&select(&framed(narrow)))).mmsi, 244_670_316);
    }

    #[test]
    fn a_looped_replay_over_digital_silence_decodes() {
        let one_pass = {
            let mut iq = burst(&position_payload(&report()), RATE);
            let pad = RATE as usize - iq.len();
            iq.extend(silence(pad));
            iq
        };
        let mut iq = one_pass.clone();
        iq.extend(one_pass);
        let msgs = run(&select(&iq));
        assert!(!msgs.is_empty(), "no pass of the loop decoded");
        for m in msgs {
            assert_eq!(m.mmsi, 244_670_316);
        }
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AisChannelRx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            ais_settings(AisChannel::A),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
