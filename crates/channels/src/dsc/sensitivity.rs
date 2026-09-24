use num_complex::Complex;
use sdrmm_wire::DataLinkMessage;

use super::equivalence::{CALLS, offset_transmission, run_ours, run_xng};
use crate::datalink::hex;

type Decode = fn(&[Complex<f32>], usize) -> Vec<DataLinkMessage>;

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Score {
    pub valid: u32,
    pub wrong: u32,
    pub damaged: u32,
}

fn sigma_for(ebn0_db: f64) -> f32 {
    (40.0 / 10f64.powf(ebn0_db / 10.0)).sqrt() as f32
}

fn expected_raw(call: &[i32]) -> String {
    let bytes: Vec<u8> = call.iter().map(|&symbol| symbol as u8).collect();
    hex(&bytes)
}

fn agree(raw: &str, expected: &str) -> bool {
    let protected = 2..2 * (expected.len() / 2 - 2);
    raw.get(protected.clone()) == expected.get(protected)
}

fn score(messages: &[DataLinkMessage], call: &[i32]) -> Score {
    let expected = expected_raw(call);
    let mut score = Score::default();
    for message in messages {
        let matches = message
            .raw
            .as_deref()
            .is_some_and(|raw| agree(raw, &expected));
        match (message.crc_ok, matches) {
            (true, true) => score.valid += 1,
            (true, false) => score.wrong += 1,
            (false, _) => score.damaged += 1,
        }
    }
    score
}

pub(super) fn sweep_point(decode: Decode, ebn0_db: f64, trials: u64) -> Score {
    let span = env_or("DSC_OFFSET_HZ", 15.0f64);
    let mut total = Score::default();
    for trial in 0..trials {
        let call = CALLS[trial as usize % CALLS.len()];
        let offset = (trial as f64 * 7.3) % (2.0 * span) - span;
        let iq = offset_transmission(&[call], true, offset, sigma_for(ebn0_db), 1_000 + trial);
        let found = score(&decode(&iq, 4_096), call);
        total.valid += found.valid.min(1);
        total.wrong += found.wrong;
        total.damaged += found.damaged;
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
    let trials = env_or("DSC_TRIALS", 40u64);
    let points: String = env_or("DSC_POINTS", "6,7,8,9,10,11,12,13,14,16".to_owned());
    for ebn0 in points
        .split(',')
        .filter_map(|point| point.parse::<f64>().ok())
    {
        let xng = sweep_point(run_xng, ebn0, trials);
        let ours = sweep_point(run_ours, ebn0, trials);
        eprintln!(
            "Eb/N0 {ebn0:>4.1} dB  xng {:>3}/{trials} wrong {} damaged {}  ours {:>3}/{trials} wrong {} damaged {}",
            xng.valid, xng.wrong, xng.damaged, ours.valid, ours.wrong, ours.damaged
        );
    }
}

#[test]
fn decodes_where_xng_cannot() {
    let ours = sweep_point(run_ours, 9.0, 20);
    let xng = sweep_point(run_xng, 9.0, 20);
    assert!(ours.valid >= 17, "{ours:?}");
    assert_eq!(ours.wrong, 0, "{ours:?}");
    assert!(ours.valid > xng.valid + 10, "ours {ours:?} xng {xng:?}");
}

#[test]
#[ignore = "measurement, prints false alarms on noise"]
fn noise_false_alarms() {
    let minutes = env_or("DSC_NOISE_MINUTES", 10usize);
    let mut iq = vec![Complex::new(0.0, 0.0); minutes * 60 * 8_000];
    crate::testutil::add_awgn(&mut iq, 1.0, 77);
    let mut filtered = Vec::new();
    super::channel_filter().process(&iq, &mut filtered);
    for (name, decode) in [("xng", run_xng as Decode), ("ours", run_ours)] {
        let messages = decode(&filtered, 4_096);
        let valid = messages.iter().filter(|m| m.crc_ok).count();
        eprintln!(
            "{name}: {} messages on {minutes} min of noise, {valid} with a valid ECC",
            messages.len()
        );
    }
}
