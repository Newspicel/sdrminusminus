use num_complex::Complex;
use sdrmm_wire::{
    ChannelParams, DecoderEvent, NfmParams, RadiosondeFrame, RadiosondeParams, SondeType,
};

use super::{
    dfm::{self, DfmFrames},
    fields::{Motion, ecef_to_geodetic, ecef_velocity_to_enu, gps_time},
    imet::{self, ImetPackets},
    m10,
    meteomodem::{Accept, MeteomodemFrames},
    rs41::{self, Rs41Frames},
    *,
};
use crate::{
    synth::{
        self,
        radiosonde::{
            Flight, RATE, dfm_payload, flight, imet_packets, m10_frame, m20_frame, rs41_frame,
            transmission,
        },
    },
    testutil::{add_awgn, complex_noise, settings},
};

const BLOCKS: [usize; 7] = [4_096, 1, 997, 65, 12_288, 7, 2_048];
const BOTH: Accept = Accept {
    m10: true,
    m20: true,
};

fn channel(sonde: Option<SondeType>) -> RadiosondeChannel {
    RadiosondeChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Radiosonde(RadiosondeParams { sonde })),
    )
    .expect("builds")
}

fn run(
    chan: &mut RadiosondeChannel,
    iq: &[Complex<f32>],
    blocks: &[usize],
) -> Vec<RadiosondeFrame> {
    let mut out = ChannelOutputs::default();
    let mut frames = Vec::new();
    let mut at = 0;
    for len in blocks.iter().cycle() {
        if at >= iq.len() {
            break;
        }
        let end = (at + len).min(iq.len());
        out.reset();
        chan.process(&iq[at..end], &mut out);
        for event in out.events.drain(..) {
            match event {
                DecoderEvent::Radiosonde(frame) => frames.push(frame),
                other => panic!("unexpected event {other:?}"),
            }
        }
        at = end;
    }
    frames
}

fn decode(sonde: Option<SondeType>, iq: &[Complex<f32>]) -> Vec<RadiosondeFrame> {
    run(&mut channel(sonde), iq, &BLOCKS)
}

fn index_of(truth: &Flight, frame: &RadiosondeFrame) -> u32 {
    let wrap = match truth.sonde {
        SondeType::Dfm | SondeType::M10 | SondeType::M20 => 0x100,
        SondeType::Rs41 | SondeType::Imet4 => 0x1_0000,
    };
    let number = frame.frame.expect("frame number");
    (number + wrap - truth.first_frame % wrap) % wrap
}

fn close(label: &str, got: Option<f64>, want: f64, tolerance: f64) {
    let got = got.unwrap_or_else(|| panic!("{label} missing"));
    assert!(
        (got - want).abs() <= tolerance,
        "{label}: got {got}, want {want} ± {tolerance}"
    );
}

fn check_fix(truth: &Flight, frame: &RadiosondeFrame, position_tolerance: f64, alt_tolerance: f64) {
    assert_eq!(frame.sonde, truth.sonde);
    assert_eq!(frame.serial, truth.serial);
    let fix = truth.fix(index_of(truth, frame));
    close("lat", frame.lat, fix.lat, position_tolerance);
    close("lon", frame.lon, fix.lon, position_tolerance);
    close("alt", frame.altitude_m, fix.alt, alt_tolerance);
    if truth.sonde == SondeType::Imet4 {
        let time = frame.time.as_deref().expect("time");
        assert_eq!(&time[10..], &fix.time[10..], "time of day");
    } else {
        assert_eq!(frame.time.as_deref(), Some(fix.time.as_str()), "time");
    }
}

fn check_motion(truth: &Flight, frame: &RadiosondeFrame, tolerance: f64) {
    close("speed", frame.speed_ms, truth.speed_ms(), tolerance);
    close("heading", frame.heading_deg, truth.heading_deg(), 1.0);
    close("climb", frame.climb_ms, truth.up_ms, tolerance);
}

fn temperature(frame: &RadiosondeFrame) -> Option<f64> {
    frame.temperature_c.map(f64::from)
}

fn humidity(frame: &RadiosondeFrame) -> Option<f64> {
    frame.humidity_pct.map(f64::from)
}

#[test]
fn rs41_round_trip_recovers_position_time_and_ptu() {
    let truth = flight(SondeType::Rs41);
    let frames = decode(
        Some(SondeType::Rs41),
        &transmission(SondeType::Rs41, 6, RATE),
    );
    assert!(frames.len() >= 5, "decoded {} frames", frames.len());
    for frame in &frames {
        check_fix(&truth, frame, 1e-6, 0.05);
        check_motion(&truth, frame, 0.02);
        assert_eq!(frame.satellites, Some(truth.satellites));
        close(
            "battery",
            frame.battery_v.map(f64::from),
            truth.battery_v,
            1e-3,
        );
        assert_eq!(frame.errors_corrected, 0);
    }
    let last = frames.last().expect("frames");
    close("temperature", temperature(last), truth.temperature_c, 0.01);
    close("humidity", humidity(last), truth.humidity_pct, 0.1);
    assert_eq!(
        last.pressure_hpa, None,
        "pressure needs subframes 0x21..0x2A"
    );
}

fn rs41_bytes(index: u32) -> [u8; rs41::EXTENDED_LEN] {
    let mut frame = [0u8; rs41::EXTENDED_LEN];
    frame[..rs41::STANDARD_LEN].copy_from_slice(&rs41_frame(index));
    frame
}

#[test]
fn rs41_full_calibration_adds_pressure() {
    let truth = flight(SondeType::Rs41);
    let mut frames = Rs41Frames::new();
    let mut last = None;
    for index in 0..60 {
        let mut bytes = rs41_bytes(index);
        last = frames.decode(&mut bytes, rs41::STANDARD_LEN);
    }
    let last = last.expect("decoded");
    close(
        "pressure",
        last.pressure_hpa.map(f64::from),
        truth.pressure_hpa,
        0.05,
    );
    close("temperature", temperature(&last), truth.temperature_c, 0.01);
    assert_eq!(frames.rejected(), 0);
}

#[test]
fn rs41_reed_solomon_repairs_byte_errors_and_reports_them() {
    let truth = flight(SondeType::Rs41);
    let mut bytes = rs41_bytes(4);
    for at in [9, 40, 57, 60, 0x70, 0x115, 0x116, 300] {
        bytes[at] ^= 0x5A;
    }
    let frame = Rs41Frames::new()
        .decode(&mut bytes, rs41::STANDARD_LEN)
        .expect("corrected frame");
    assert_eq!(frame.errors_corrected, 8);
    check_fix(&truth, &frame, 1e-6, 0.05);
}

#[test]
fn rs41_uncorrectable_status_block_emits_nothing_and_counts() {
    let mut frames = Rs41Frames::new();
    let mut bytes = rs41_bytes(4);
    for at in (0x3B..0x3B + 26).step_by(2) {
        bytes[at] ^= 0xFF;
    }
    assert!(frames.decode(&mut bytes, rs41::STANDARD_LEN).is_none());
    assert_eq!(frames.rejected(), 1);
}

#[test]
fn rs41_header_dewhitens_to_the_documented_bytes() {
    let mut header = rs41::HEADER;
    rs41::whiten(&mut header);
    assert_eq!(header, [0x86, 0x35, 0xF4, 0x40, 0x93, 0xDF, 0x1A, 0x60]);
}

#[test]
fn dfm_round_trip_assembles_serial_and_decodes_gps() {
    let truth = flight(SondeType::Dfm);
    let frames = decode(Some(SondeType::Dfm), &transmission(SondeType::Dfm, 7, RATE));
    assert!(frames.len() >= 3, "decoded {} frames", frames.len());
    for frame in &frames {
        check_fix(&truth, frame, 1e-7, 0.01);
        check_motion(&truth, frame, 0.01);
        assert_eq!(frame.satellites, Some(truth.satellites));
        close("temperature", temperature(frame), truth.temperature_c, 0.02);
        close(
            "battery",
            frame.battery_v.map(f64::from),
            truth.battery_v,
            1e-3,
        );
    }
}

#[test]
fn dfm_hamming_corrects_one_bit_and_rejects_two() {
    for nibble in 0..16u8 {
        let code = dfm::hamming_encode(nibble);
        assert_eq!(dfm::hamming_decode(code), Some((nibble, 0)));
        for flip in 0..8 {
            let mut damaged = code;
            damaged[flip] = !damaged[flip];
            assert_eq!(dfm::hamming_decode(damaged), Some((nibble, 1)));
        }
    }
    let mut two = dfm::hamming_encode(0x9);
    two[0] = !two[0];
    two[5] = !two[5];
    assert!(dfm::hamming_decode(two).is_none_or(|(nibble, _)| nibble != 0x9));
}

#[test]
fn dfm_damaged_codewords_are_counted_and_corrected() {
    let mut frames = DfmFrames::new();
    let mut emitted = Vec::new();
    for index in 0..40 {
        let mut bits: [bool; dfm::PAYLOAD_BITS] = dfm_payload(index).try_into().expect("bits");
        bits[3] = !bits[3];
        if index == 10 {
            bits[60] = !bits[60];
            bits[60 + 13] = !bits[60 + 13];
        }
        emitted.extend(frames.decode(&bits));
    }
    assert!(frames.rejected() >= 1);
    assert!(!emitted.is_empty());
    assert!(emitted.iter().all(|frame| frame.errors_corrected >= 5));
}

#[test]
fn dfm_float24_scales_by_its_exponent() {
    assert_eq!(dfm::float24(0x2D_6D80), 0xD_6D80 as f64 / 4.0);
}

#[test]
fn m10_round_trip_recovers_trimble_fields() {
    let truth = flight(SondeType::M10);
    let frames = decode(Some(SondeType::M10), &transmission(SondeType::M10, 4, RATE));
    assert!(frames.len() >= 3, "decoded {} frames", frames.len());
    for frame in &frames {
        check_fix(&truth, frame, 1e-6, 0.002);
        check_motion(&truth, frame, 0.01);
        assert_eq!(frame.satellites, Some(truth.satellites));
        close("temperature", temperature(frame), truth.temperature_c, 0.1);
        close("humidity", humidity(frame), truth.humidity_pct, 0.1);
        close(
            "battery",
            frame.battery_v.map(f64::from),
            truth.battery_v,
            0.01,
        );
    }
}

#[test]
fn m20_round_trip_recovers_fields() {
    let truth = flight(SondeType::M20);
    let frames = decode(Some(SondeType::M20), &transmission(SondeType::M20, 4, RATE));
    assert!(frames.len() >= 3, "decoded {} frames", frames.len());
    for frame in &frames {
        check_fix(&truth, frame, 1e-6, 0.01);
        check_motion(&truth, frame, 0.01);
        close("temperature", temperature(frame), truth.temperature_c, 0.1);
        close("humidity", humidity(frame), truth.humidity_pct, 1.0);
        close(
            "battery",
            frame.battery_v.map(f64::from),
            truth.battery_v,
            0.02,
        );
    }
}

#[test]
fn meteomodem_checksum_failures_emit_nothing_and_count() {
    let mut frames = MeteomodemFrames::new();
    let good = m10_frame(2);
    assert!(frames.decode(&good, BOTH).is_some());
    for index in [5, 40, 90] {
        let mut bad = good.clone();
        bad[index] ^= 0x10;
        assert!(frames.decode(&bad, BOTH).is_none());
    }
    let mut bad20 = m20_frame(2);
    bad20[0x1D] ^= 0x01;
    assert!(frames.decode(&bad20, BOTH).is_none());
    assert_eq!(frames.rejected(), 4);
}

#[test]
fn m10_gtop_frames_use_decimal_coordinates_and_civil_time() {
    let mut frame = m10_frame(0);
    frame[1] = m10::TYPE_GTOP;
    frame[m10::GTOP_LAT..m10::GTOP_LAT + 4].copy_from_slice(&47_123_456i32.to_be_bytes());
    frame[m10::GTOP_LON..m10::GTOP_LON + 4].copy_from_slice(&(-3_456_789i32).to_be_bytes());
    frame[m10::GTOP_ALT..m10::GTOP_ALT + 3].copy_from_slice(&[0x01, 0xE2, 0x40]);
    frame[m10::GTOP_VEL..m10::GTOP_VEL + 6].copy_from_slice(&[0x00, 0x64, 0xFF, 0x38, 0x01, 0xF4]);
    frame[m10::GTOP_TIME..m10::GTOP_TIME + 3].copy_from_slice(&123_456u32.to_be_bytes()[1..]);
    frame[m10::GTOP_DATE..m10::GTOP_DATE + 3].copy_from_slice(&11_026u32.to_be_bytes()[1..]);
    let decoded = m10::decode(&frame).expect("decodes");
    close("lat", decoded.lat, 47.123_456, 1e-9);
    close("lon", decoded.lon, -3.456_789, 1e-9);
    close("alt", decoded.altitude_m, 1_234.56, 1e-9);
    close("climb", decoded.climb_ms, 5.0, 1e-9);
    close("speed", decoded.speed_ms, 1.0f64.hypot(2.0), 1e-9);
    assert_eq!(decoded.time.as_deref(), Some("2026-10-01T12:34:56Z"));
}

#[test]
fn imet_round_trip_recovers_ptu_and_position() {
    let truth = flight(SondeType::Imet4);
    let frames = decode(
        Some(SondeType::Imet4),
        &transmission(SondeType::Imet4, 4, RATE),
    );
    assert!(frames.len() >= 3, "decoded {} frames", frames.len());
    for frame in &frames {
        check_fix(&truth, frame, 1e-5, 0.6);
        assert_eq!(frame.satellites, Some(truth.satellites));
        close(
            "temperature",
            temperature(frame),
            truth.temperature_c,
            0.006,
        );
        close("humidity", humidity(frame), truth.humidity_pct, 0.006);
        close(
            "pressure",
            frame.pressure_hpa.map(f64::from),
            truth.pressure_hpa,
            0.006,
        );
        close(
            "battery",
            frame.battery_v.map(f64::from),
            truth.battery_v,
            0.01,
        );
    }
}

#[test]
fn imet_extended_gps_carries_velocity() {
    let mut packets = ImetPackets::new();
    let mut gps = vec![imet::SOH, imet::PACKET_EXTENDED_GPS];
    gps.extend_from_slice(&40.5f32.to_le_bytes());
    gps.extend_from_slice(&(-105.25f32).to_le_bytes());
    gps.extend_from_slice(&6_000u16.to_le_bytes());
    gps.push(9);
    for velocity in [3.0f32, 4.0, -2.5] {
        gps.extend_from_slice(&velocity.to_le_bytes());
    }
    gps.extend_from_slice(&[1, 2, 3]);
    let crc = imet::crc(&gps);
    gps.extend_from_slice(&crc.to_be_bytes());
    assert_eq!(
        Some(gps.len()),
        imet::packet_len(imet::PACKET_EXTENDED_GPS, None)
    );
    assert!(packets.packet(&gps).is_none());
    let ptu = &imet_packets(0)[18..];
    let frame = packets.packet(ptu).expect("frame");
    close("alt", frame.altitude_m, 1_000.0, 1e-9);
    close("speed", frame.speed_ms, 5.0, 1e-6);
    close("heading", frame.heading_deg, 36.869_897_6, 1e-5);
    close("climb", frame.climb_ms, -2.5, 1e-9);
    assert!(
        frame
            .time
            .as_deref()
            .is_some_and(|t| t.ends_with("T01:02:03Z"))
    );
}

#[test]
fn imet_crc_failure_emits_nothing_and_counts() {
    let mut iq = transmission(SondeType::Imet4, 3, RATE);
    let mut chan = channel(Some(SondeType::Imet4));
    let clean = run(&mut chan, &iq, &BLOCKS);
    assert!(!clean.is_empty());
    let mut damaged_chan = channel(Some(SondeType::Imet4));
    let bit_samples = 40;
    let start = (0.1 * RATE) as usize + 25 * 10 * bit_samples;
    for (n, sample) in iq[start..start + 3 * bit_samples].iter_mut().enumerate() {
        let phase = std::f64::consts::TAU * imet::SPACE_HZ * n as f64 / RATE;
        *sample = Complex::from_polar(1.0, phase.sin() as f32);
    }
    let damaged = run(&mut damaged_chan, &iq, &BLOCKS);
    assert_eq!(damaged.len() + 1, clean.len());
    assert!(damaged_chan.rejected(SondeType::Imet4) >= 1);
    assert!(clean.iter().all(|frame| frame.rejected == 0));
    let last = damaged.last().expect("frames after the damage");
    assert_eq!(last.rejected, damaged_chan.rejected(SondeType::Imet4));
}

fn mixed_signal() -> Vec<Complex<f32>> {
    let mut iq = Vec::new();
    for (sonde, frames) in [
        (SondeType::Rs41, 2),
        (SondeType::Dfm, 5),
        (SondeType::M10, 2),
        (SondeType::M20, 2),
        (SondeType::Imet4, 3),
    ] {
        iq.extend(transmission(sonde, frames, RATE));
        iq.extend(synth::silence(4_800));
    }
    iq
}

#[test]
fn auto_mode_identifies_every_sonde() {
    let frames = decode(None, &mixed_signal());
    for sonde in SondeType::ALL {
        let truth = flight(sonde);
        let found: Vec<_> = frames.iter().filter(|f| f.sonde == sonde).collect();
        assert!(!found.is_empty(), "{} not decoded", sonde.label());
        assert!(found.iter().all(|f| f.serial == truth.serial));
    }
}

#[test]
fn fixed_mode_ignores_other_sondes() {
    let iq = mixed_signal();
    for sonde in SondeType::ALL {
        let frames = decode(Some(sonde), &iq);
        assert!(!frames.is_empty(), "{} not decoded", sonde.label());
        assert!(
            frames.iter().all(|f| f.sonde == sonde),
            "{} leaked others",
            sonde.label()
        );
    }
}

#[test]
fn frequency_offset_and_noise_still_decode() {
    for (sonde, frames, offset_hz, sigma) in [
        (SondeType::Rs41, 4, 3_000.0, 0.25),
        (SondeType::Dfm, 6, -2_500.0, 0.25),
        (SondeType::M10, 3, 2_000.0, 0.2),
        (SondeType::M20, 3, -3_000.0, 0.2),
        (SondeType::Imet4, 3, 3_500.0, 0.25),
    ] {
        let mut iq = transmission(sonde, frames, RATE);
        synth::shift(&mut iq, offset_hz, RATE);
        add_awgn(&mut iq, sigma, 7);
        let decoded = decode(Some(sonde), &iq);
        assert!(
            !decoded.is_empty(),
            "{} lost at {offset_hz} Hz",
            sonde.label()
        );
        let truth = flight(sonde);
        assert!(decoded.iter().all(|f| f.serial == truth.serial));
    }
}

#[test]
fn ragged_block_splits_decode_identically() {
    let iq = mixed_signal();
    let whole = run(&mut channel(None), &iq, &[iq.len()]);
    let ragged = run(&mut channel(None), &iq, &[1, 3, 4_097, 333, 50_000]);
    assert!(!whole.is_empty());
    assert_eq!(whole, ragged);
}

#[test]
fn pure_noise_emits_nothing() {
    let noise = complex_noise(99, 0.7, (RATE * 6.0) as usize);
    assert!(decode(None, &noise).is_empty());
}

#[test]
fn wrong_params_and_rate_are_rejected() {
    let wrong = RadiosondeChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Nfm(NfmParams::default())),
    );
    assert!(matches!(wrong, Err(ChannelError::InvalidSettings(_))));
    let rate = RadiosondeChannel::new(
        ChannelCtx {
            input_rate: 96_000.0,
        },
        settings(ChannelParams::Radiosonde(RadiosondeParams::default())),
    );
    assert!(rate.is_err());
    let mut chan = channel(None);
    assert!(
        chan.apply(settings(ChannelParams::Nfm(NfmParams::default())))
            .is_err()
    );
}

#[test]
fn ecef_converts_to_geodetic_and_local_velocity() {
    let (lat, lon, alt) = (48.0f64, 11.0f64, 5_000.0);
    let e2 = 1.0 - (6_356_752.314_245_18f64 / 6_378_137.0).powi(2);
    let (phi, lam) = (lat.to_radians(), lon.to_radians());
    let n = 6_378_137.0 / (1.0 - e2 * phi.sin().powi(2)).sqrt();
    let x = (n + alt) * phi.cos() * lam.cos();
    let y = (n + alt) * phi.cos() * lam.sin();
    let z = (n * (1.0 - e2) + alt) * phi.sin();
    let geodetic = ecef_to_geodetic(x, y, z);
    close("lat", Some(geodetic.lat), lat, 1e-8);
    close("lon", Some(geodetic.lon), lon, 1e-12);
    close("alt", Some(geodetic.alt), alt, 1e-3);
    let up = [phi.cos() * lam.cos(), phi.cos() * lam.sin(), phi.sin()];
    let [east, north, climb] = ecef_velocity_to_enu(geodetic, up.map(|c| c * 4.0));
    close("east", Some(east), 0.0, 1e-6);
    close("north", Some(north), 0.0, 1e-6);
    close("up", Some(climb), 4.0, 1e-6);
    let motion = Motion::from_enu(-1.0, 0.0, 0.0);
    close("heading", Some(motion.heading_deg), 270.0, 1e-9);
}

#[test]
fn gps_week_and_tow_become_utc() {
    assert_eq!(
        gps_time(2_386, 304_514, 18).as_deref(),
        Some("2025-10-01T12:34:56Z")
    );
}
