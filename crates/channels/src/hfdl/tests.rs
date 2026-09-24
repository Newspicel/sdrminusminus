use super::*;
use crate::testutil::{run_events, settings};
use fec::SETTINGS;
use modulate::{burst_symbols, modulate};
use pdu::build::{acars_mpdu, spdu};

struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as f32 / u64::MAX as f32) * 2.0 - 1.0
    }
}

fn burst_iq(payload: &[u8], setting: usize, cfo: f64) -> Vec<Complex<f32>> {
    let symbols = burst_symbols(payload, &SETTINGS[setting]);
    modulate(&symbols, RATE, SUBCARRIER_OFFSET_HZ + cfo, 0.5)
}

fn with_noise(mut iq: Vec<Complex<f32>>, level: f32, seed: u64) -> Vec<Complex<f32>> {
    let mut noise = Noise(seed);
    for sample in &mut iq {
        *sample += Complex::new(noise.next() * level, noise.next() * level);
    }
    iq
}

fn gap() -> Vec<Complex<f32>> {
    vec![Complex::default(); 3_000]
}

fn channel() -> HfdlChannel {
    HfdlChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Hfdl(HfdlParams::default())),
    )
    .expect("channel")
}

fn decode(iq: &[Complex<f32>]) -> Vec<HfdlEvent> {
    let mut receiver = Receiver::new(RATE);
    let mut events = Vec::new();
    for chunk in iq.chunks(8_192) {
        receiver.process(chunk, &mut events);
    }
    events
}

fn single(payload: &[u8], setting: usize, cfo: f64, level: f32) -> Vec<HfdlEvent> {
    let mut iq = gap();
    iq.extend(burst_iq(payload, setting, cfo));
    iq.extend(gap());
    decode(&with_noise(
        iq,
        level,
        0xd00d_f00d_0042_4242 + setting as u64,
    ))
}

#[test]
fn decodes_a_ground_station_squitter() {
    let mut iq = gap();
    iq.extend(burst_iq(&spdu(7, 1_234, 52), 0, 0.0));
    iq.extend(gap());
    let events = run_events(&mut channel(), &iq);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DecoderEvent::Hfdl(message) if message.crc_ok && message.message_type == "squitter"))
    );
}

#[test]
fn decodes_spdu_at_300bps() {
    let events = single(&spdu(7, 1234, 52), 0, 20.0, 0.01);
    let squitter = events
        .iter()
        .find(|e| e.kind == "squitter")
        .expect("squitter");
    assert_eq!(squitter.details["gs_id"], 7);
    assert_eq!(squitter.details["frame_index"], 1234);
    assert_eq!(squitter.details["systable_version"], 52);
}

#[test]
fn decodes_acars_at_every_rate() {
    for (setting, cfo) in [(1, -35.0), (2, 50.0), (3, 15.0)] {
        let events = single(&acars_mpdu(), setting, cfo, 0.01);
        let acars = events.iter().find_map(|e| e.acars.as_ref()).expect("acars");
        assert!(acars.crc_ok);
        assert_eq!(acars.core.tail.as_deref(), Some("N471XG"));
    }
}

#[test]
fn fec_corrected_is_zero_on_a_clean_burst() {
    let events = single(&acars_mpdu(), 1, 0.0, 0.0);
    let acars = events.iter().find(|e| e.kind == "acars").expect("acars");
    assert_eq!(acars.fec_corrected, Some(0));
}

#[test]
fn acars_message_carries_quality() {
    let mut iq = gap();
    iq.extend(burst_iq(&acars_mpdu(), 1, -35.0));
    iq.extend(gap());
    let events = run_events(&mut channel(), &with_noise(iq, 0.01, 7));
    let Some(DecoderEvent::Hfdl(message)) = events.first() else {
        panic!("no HFDL event");
    };
    assert_eq!(message.message_type, "acars");
    assert!(message.crc_ok);
    assert!(message.snr_db.is_some());
    assert!(
        message
            .frequency_error_hz
            .is_some_and(|hz| (hz + 35.0).abs() < 5.0)
    );
}

fn xng_decode(iq: &[Complex<f32>]) -> Vec<xng_mode_hfdl::pdu::HfdlEvent> {
    let mut decoder = xng_mode_hfdl::HfdlChannelDecoder::new(RATE, 0.0).expect("xng");
    iq.chunks(8_192)
        .flat_map(|chunk| decoder.process(chunk))
        .collect()
}

fn xng_message(event: &xng_mode_hfdl::pdu::HfdlEvent) -> DataLinkMessage {
    let (body, crc_ok) = match &event.acars {
        Some(block) => (
            xng_types::MessageBody::Acars(block.core.clone()),
            block.crc_ok,
        ),
        None => (
            xng_types::MessageBody::Hfdl {
                kind: event.kind.clone(),
                details: event.details.clone(),
            },
            true,
        ),
    };
    datalink::message(
        &body,
        Quality {
            crc_ok,
            fec_corrected: event.fec_corrected,
            snr_db: event.snr_db,
            frequency_error_hz: event.freq_skew_hz,
        },
        Some(&event.raw),
    )
}

fn mixed_traffic(level: f32) -> Vec<Complex<f32>> {
    let mut iq = gap();
    for (payload, setting, cfo) in [
        (spdu(4, 2_397, 52), 0, 12.0),
        (acars_mpdu(), 1, -30.0),
        (acars_mpdu(), 2, 40.0),
        (spdu(1, 2_399, 52), 4, -5.0),
        (acars_mpdu(), 3, 8.0),
        (acars_mpdu(), 5, 0.0),
    ] {
        iq.extend(burst_iq(&payload, setting, cfo));
        iq.extend(gap());
    }
    with_noise(iq, level, 0x5eed)
}

fn content(message: DataLinkMessage) -> DataLinkMessage {
    DataLinkMessage {
        fec_corrected: None,
        snr_db: None,
        frequency_error_hz: None,
        ..message
    }
}

fn is_subsequence(needles: &[DataLinkMessage], haystack: &[DataLinkMessage]) -> bool {
    let mut remaining = haystack.iter();
    needles
        .iter()
        .all(|needle| remaining.any(|candidate| candidate == needle))
}

#[test]
fn finds_everything_xng_finds_on_noisy_traffic() {
    let (mut total_ours, mut total_xng) = (0, 0);
    for level in [0.3, 0.9, 1.2, 1.5] {
        let iq = mixed_traffic(level);
        let ours: Vec<_> = decode(&iq).iter().map(|e| content(message(e))).collect();
        let theirs: Vec<_> = xng_decode(&iq)
            .iter()
            .map(|e| content(xng_message(e)))
            .collect();
        assert!(ours.iter().all(|m| m.crc_ok));
        assert!(is_subsequence(&theirs, &ours), "noise {level}");
        total_ours += ours.len();
        total_xng += theirs.len();
    }
    assert!(total_ours > total_xng, "ours {total_ours} xng {total_xng}");
}

#[test]
fn stays_silent_on_noise() {
    let mut noise = Noise(0xfeed);
    let iq: Vec<Complex<f32>> = (0..720_000)
        .map(|_| Complex::new(gaussian(&mut noise), gaussian(&mut noise)) * 0.3)
        .collect();
    assert!(decode(&iq).is_empty());
}

fn gaussian(noise: &mut Noise) -> f32 {
    (0..12).map(|_| noise.next() * 0.5).sum::<f32>()
}

fn faded(iq: &[Complex<f32>], seed: u64) -> Vec<Complex<f32>> {
    let delay = 12;
    let spread = 0.3 + (seed % 5) as f64 * 0.2;
    iq.iter()
        .enumerate()
        .map(|(n, &x)| {
            let t = n as f64 / RATE;
            let direct = (std::f64::consts::TAU * spread * t).cos() as f32;
            let late = (std::f64::consts::TAU * spread * 1.7 * t + 1.0).sin() as f32 * 0.7;
            let echo = n.checked_sub(delay).map_or(Complex::default(), |k| iq[k]);
            x * direct + echo * late
        })
        .collect()
}

fn trial(setting: usize, sigma: f32, seed: u64, fading: bool) -> Vec<Complex<f32>> {
    let payload = if SETTINGS[setting].bps == 300 {
        spdu(4, seed as u16, 52)
    } else {
        acars_mpdu()
    };
    let cfo = (seed % 7) as f64 * 13.0 - 39.0;
    let mut iq = gap();
    iq.extend(burst_iq(&payload, setting, cfo));
    iq.extend(gap());
    if fading {
        iq = faded(&iq, seed);
    }
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
    for sample in &mut iq {
        *sample += Complex::new(gaussian(&mut noise), gaussian(&mut noise)) * sigma;
    }
    iq
}

fn decoded(events: usize) -> usize {
    usize::from(events > 0)
}

#[test]
fn beats_xng_on_fading_downlinks() {
    let (mut ours, mut theirs) = (0, 0);
    for seed in 0..12 {
        let iq = trial(1, 0.25, seed, true);
        ours += decoded(decode(&iq).len());
        theirs += decoded(xng_decode(&iq).len());
    }
    assert!(ours >= theirs + 4 && ours >= 8, "ours {ours} xng {theirs}");
}

#[test]
#[ignore = "sensitivity sweep, run with --ignored --nocapture"]
fn sensitivity_sweep() {
    let (mut total_ours, mut total_xng) = (0, 0);
    for (fading, sigmas) in [
        (false, [0.3f32, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]),
        (true, [0.1f32, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4]),
    ] {
        for setting in [0, 1, 2, 3] {
            let mut line = String::new();
            for sigma in sigmas {
                let (mut ours, mut theirs) = (0, 0);
                for seed in 0..24 {
                    let iq = trial(setting, sigma, seed, fading);
                    ours += decoded(decode(&iq).len());
                    theirs += decoded(xng_decode(&iq).len());
                }
                total_ours += ours;
                total_xng += theirs;
                line.push_str(&format!(" {sigma}:{ours}/{theirs}"));
            }
            eprintln!("fading {fading} setting {setting}:{line}");
        }
    }
    eprintln!("total ours {total_ours} xng {total_xng}");
}
