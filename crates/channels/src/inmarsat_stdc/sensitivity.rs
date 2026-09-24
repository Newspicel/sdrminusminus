use num_complex::Complex;
use sdrmm_wire::DataLinkMessage;

use super::{
    RATE, equivalence::frame_payload, equivalence::run_ours, equivalence::run_xng, frame,
    modulate::modulate,
};
use crate::testutil::add_awgn;

type Decode = fn(&[Complex<f32>], usize) -> Vec<DataLinkMessage>;

const SAMPLES_PER_SYMBOL: f64 = 10.0;

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Score {
    pub valid: u32,
    pub wrong: u32,
    pub packets: u32,
}

fn text(index: usize) -> String {
    format!("SECURITE NAVAREA XII {index:03} BUOY ADRIFT")
}

pub(super) fn noisy_frames(
    frames: usize,
    esn0_db: f64,
    offset_hz: f64,
    seed: u64,
) -> Vec<Complex<f32>> {
    let mut symbols: Vec<u8> = (0..4_000).map(|index| (index % 2) as u8).collect();
    for index in 0..frames {
        symbols.extend(frame::encode_frame(&frame_payload(
            text(index).as_bytes(),
            index as u16,
        )));
    }
    symbols.extend((0..400).map(|index| (index % 2) as u8));
    let mut iq = modulate(&symbols, 1_200.0, RATE, offset_hz, 0.5);
    let power = iq.iter().map(Complex::norm_sqr).sum::<f32>() / iq.len() as f32;
    let sigma = (f64::from(power) * SAMPLES_PER_SYMBOL / 2.0 / 10f64.powf(esn0_db / 10.0)).sqrt();
    add_awgn(&mut iq, sigma as f32, seed);
    let mut filtered = Vec::with_capacity(iq.len());
    super::channel_filter().process(&iq, &mut filtered);
    filtered
}

fn score(messages: &[DataLinkMessage], frames: usize) -> Score {
    let mut score = Score::default();
    for message in messages.iter().filter(|m| m.crc_ok) {
        score.packets += 1;
        if message.message_type != "egc-message" {
            continue;
        }
        let expected = (0..frames).any(|index| message.text.as_deref() == Some(&text(index)));
        if expected {
            score.valid += 1;
        } else {
            score.wrong += 1;
        }
    }
    score
}

pub(super) fn sweep_point(decode: Decode, esn0_db: f64, trials: u64, frames: usize) -> Score {
    let mut total = Score::default();
    for trial in 0..trials {
        let offset = (trial as f64 * 137.0) % 600.0 - 300.0;
        let iq = noisy_frames(frames, esn0_db, offset, 500 + trial);
        let found = score(&decode(&iq, 8_192), frames);
        total.valid += found.valid;
        total.wrong += found.wrong;
        total.packets += found.packets;
    }
    total
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "measurement, prints a table"]
fn sensitivity_table() {
    let trials = env_or("STDC_TRIALS", 8u64);
    let frames = env_or("STDC_FRAMES", 5usize);
    let points: String = env_or("STDC_POINTS", "0,1,2,3,4,5,6,8".to_owned());
    for esn0 in points
        .split(',')
        .filter_map(|point| point.parse::<f64>().ok())
    {
        let xng = sweep_point(run_xng, esn0, trials, frames);
        let ours = sweep_point(run_ours, esn0, trials, frames);
        let total = trials as usize * frames;
        eprintln!(
            "Es/N0 {esn0:>4.1} dB  xng {:>3}/{total} wrong {} packets {}  ours {:>3}/{total} wrong {} packets {}",
            xng.valid, xng.wrong, xng.packets, ours.valid, ours.wrong, ours.packets
        );
    }
}

fn identity(message: &DataLinkMessage) -> (String, Option<String>, Option<String>) {
    (
        message.message_type.clone(),
        message.text.clone(),
        message.raw.clone(),
    )
}

fn offair_score(decode: Decode, iq: &[Complex<f32>], reference: &[DataLinkMessage]) -> Score {
    let known: Vec<_> = reference.iter().map(identity).collect();
    let mut score = Score::default();
    for message in decode(iq, 8_192).iter().filter(|m| m.crc_ok) {
        score.packets += 1;
        if known.contains(&identity(message)) {
            score.valid += 1;
        } else {
            score.wrong += 1;
        }
    }
    score
}

#[test]
#[ignore = "measurement, prints a table"]
fn offair_with_added_noise() {
    let clean = super::tests::offair_channel_iq();
    let reference = run_xng(&clean, 8_192);
    let power = clean.iter().map(Complex::norm_sqr).sum::<f32>() / clean.len() as f32;
    for esn0 in [10.0, 4.0, 3.0, 2.0, 1.0, 0.0, -1.0, -2.0] {
        let mut iq = clean.clone();
        let sigma = (f64::from(power) * SAMPLES_PER_SYMBOL / 2.0 / 10f64.powf(esn0 / 10.0)).sqrt();
        add_awgn(&mut iq, sigma as f32, 7);
        let xng = offair_score(run_xng, &iq, &reference);
        let ours = offair_score(run_ours, &iq, &reference);
        eprintln!(
            "added noise at Es/N0 {esn0:>4.1} dB  xng {}/{} wrong {}  ours {}/{} wrong {}",
            xng.valid,
            reference.len(),
            xng.wrong,
            ours.valid,
            reference.len(),
            ours.wrong
        );
    }
}

#[test]
fn decodes_where_xng_cannot() {
    let ours = sweep_point(run_ours, 0.5, 2, 5);
    let xng = sweep_point(run_xng, 0.5, 2, 5);
    assert!(ours.valid >= 7, "{ours:?}");
    assert_eq!(ours.wrong, 0, "{ours:?}");
    assert!(ours.valid >= xng.valid + 5, "ours {ours:?} xng {xng:?}");
}

#[test]
#[ignore = "measurement, prints false packets on noise"]
fn noise_false_packets() {
    let minutes = env_or("STDC_NOISE_MINUTES", 5usize);
    let mut iq = vec![Complex::new(0.0, 0.0); minutes * 60 * 12_000];
    add_awgn(&mut iq, 0.3, 91);
    let mut filtered = Vec::new();
    super::channel_filter().process(&iq, &mut filtered);
    for (name, decode) in [("xng", run_xng as Decode), ("ours", run_ours)] {
        let messages = decode(&filtered, 8_192);
        eprintln!(
            "{name}: {} packets on {minutes} min of noise: {:?}",
            messages.len(),
            messages.iter().map(|m| &m.message_type).collect::<Vec<_>>()
        );
    }
}
