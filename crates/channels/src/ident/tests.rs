//! End-to-end identification: a generated transmission in, a verdict out.
//!
//! Every case here runs the same path the engine runs — the whole channel, in ragged blocks —
//! so what is under test is the five stages composed, not any one of them in isolation.

use std::time::Instant;

use num_complex::Complex;
use sdrmm_wire::{ChannelSettings, DecoderEvent, IdentParams, Modulation};

use super::{INPUT_RATE_HZ, IdentChannel};
use crate::{
    ChannelCtx, ChannelOutputs, ChannelRx,
    testgen::{self, dv as tgdv},
    testutil::complex_noise,
};

/// Half a second per report: long enough for the slowest framing cadence in the digital-voice
/// family, short enough that a generated transmission produces several.
const INTERVAL_MS: u32 = 500;

fn params() -> IdentParams {
    IdentParams {
        interval_ms: INTERVAL_MS,
        ..IdentParams::default()
    }
}

fn settings(params: IdentParams) -> ChannelSettings {
    ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        params: sdrmm_wire::ChannelParams::Ident(params),
    }
}

/// Feed `iq` through a fresh identifier in deliberately ragged blocks and collect its reports.
/// The block sizes are the point: the observation window is filled across calls, and a report
/// must not depend on where a block boundary happened to fall.
fn run(params: IdentParams, iq: &[Complex<f32>]) -> Vec<sdrmm_wire::IdentReport> {
    let ctx = ChannelCtx {
        input_rate: INPUT_RATE_HZ,
    };
    let mut channel =
        IdentChannel::new(ctx, settings(params)).expect("ident channel builds at its own rate");
    let mut out = ChannelOutputs::default();
    let mut reports = Vec::new();
    let mut pos = 0;
    for len in [8_191usize, 1, 65_536, 129, 4_096].iter().cycle() {
        if pos >= iq.len() {
            break;
        }
        let end = (pos + len).min(iq.len());
        out.reset();
        channel.process(&iq[pos..end], &mut out);
        for event in out.events.drain(..) {
            match event {
                DecoderEvent::Ident(report) => reports.push(report),
                other => panic!("unexpected {} event", other.kind()),
            }
        }
        pos = end;
    }
    reports
}

/// Repeat `iq` until it is at least `seconds` long, then add a little noise — no real signal
/// arrives without any, and a spectrum with a zero floor is not one the detector would ever see.
fn on_air(iq: &[Complex<f32>], seconds: f64, seed: u32) -> Vec<Complex<f32>> {
    let wanted = (seconds * INPUT_RATE_HZ) as usize;
    let mut out = Vec::with_capacity(wanted + iq.len());
    while out.len() < wanted {
        out.extend_from_slice(iq);
    }
    for (sample, noise) in out.iter_mut().zip(complex_noise(seed, 0.004, wanted)) {
        *sample += noise;
    }
    out
}

/// Band-limited pseudorandom audio, standing in for programme material.
fn programme(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed | 1;
    let mut smoothed = 0.0f32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = state as f32 / u32::MAX as f32 - 0.5;
            smoothed += 0.08 * (noise - smoothed);
            smoothed * 8.0
        })
        .collect()
}

fn best(report: &sdrmm_wire::IdentReport) -> Option<&str> {
    report.best().map(|m| m.name.as_str())
}

/// The verdict the run settled on: whichever modulation most of its reports agreed about.
fn consensus(reports: &[sdrmm_wire::IdentReport]) -> Modulation {
    let mut counts: Vec<(Modulation, usize)> = Vec::new();
    for report in reports {
        match counts.iter_mut().find(|(m, _)| *m == report.modulation) {
            Some((_, n)) => *n += 1,
            None => counts.push((report.modulation, 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map_or(Modulation::None, |(m, _)| m)
}

#[test]
fn an_empty_channel_reports_nothing_rather_than_guessing() {
    let noise = complex_noise(0x2b71, 0.02, (INPUT_RATE_HZ * 1.2) as usize);
    let reports = run(params(), &noise);
    assert!(!reports.is_empty(), "reports arrive on the interval");
    for report in &reports {
        assert_eq!(report.modulation, Modulation::None);
        assert!(report.candidates.is_empty());
    }
}

#[test]
fn reports_arrive_once_per_interval() {
    let noise = complex_noise(0x9c14, 0.02, (INPUT_RATE_HZ * 2.0) as usize);
    let reports = run(params(), &noise);
    // Two seconds of samples at a 500 ms interval, and the last partial window is not reported.
    assert_eq!(reports.len(), 4);
}

#[test]
fn an_unmodulated_carrier_is_named_and_located() {
    let offset = 42_000.0;
    let len = (INPUT_RATE_HZ * 1.2) as usize;
    let mut iq: Vec<Complex<f32>> = (0..len)
        .map(|k| {
            Complex::from_polar(
                0.5,
                (std::f64::consts::TAU * offset * k as f64 / INPUT_RATE_HZ) as f32,
            )
        })
        .collect();
    for (s, n) in iq.iter_mut().zip(complex_noise(0x4411, 0.002, len)) {
        *s += n;
    }
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Carrier);
    let first = &reports[0];
    assert!(
        (first.center_offset_hz - offset).abs() < 500.0,
        "offset {} Hz",
        first.center_offset_hz
    );
    assert_eq!(best(first), Some("Unmodulated carrier"));
}

#[test]
fn a_dmr_transmission_is_four_level_and_confirmed_by_its_framing() {
    let call = tgdv::dmr::Call::default();
    let iq = on_air(&tgdv::dmr::transmission(&call, INPUT_RATE_HZ), 2.0, 0x71a2);
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Fsk4);
    let confirmed = reports
        .iter()
        .find(|r| r.best().is_some_and(|m| m.confirmed))
        .expect("a DMR transmission carries its own frame sync");
    let best = confirmed.best().expect("checked above");
    assert_eq!(best.name, "DMR");
    assert_eq!(best.type_id.as_deref(), Some("dmr"));
    assert!(
        (confirmed.symbol_rate_hz.unwrap_or_default() - 4_800.0).abs() < 250.0,
        "baud {:?}",
        confirmed.symbol_rate_hz
    );
}

/// The point of the framing stage: P25 and DMR are the same waveform, and only the sync says
/// which one is on the air.
#[test]
fn a_p25_transmission_is_told_apart_from_dmr() {
    let iq = on_air(&tgdv::p25::transmission(0x293, INPUT_RATE_HZ), 2.0, 0x33c1);
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Fsk4);
    let confirmed = reports
        .iter()
        .find(|r| r.best().is_some_and(|m| m.confirmed))
        .expect("a P25 transmission carries its own frame sync");
    assert_eq!(best(confirmed), Some("P25 Phase 1"));
    // The siblings stay on the shortlist, demoted rather than deleted.
    assert!(confirmed.candidates.iter().any(|m| m.name == "DMR"));
}

#[test]
fn a_pager_transmission_is_two_level_at_its_own_baud() {
    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "IDENT TEST".to_owned(),
        numeric: false,
    }];
    let iq = on_air(
        &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
        2.0,
        0x5d90,
    );
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Fsk2);
    let named = reports
        .iter()
        .find(|r| best(r) == Some("POCSAG (1200 bd)"))
        .expect("a 1200 baud pager shift is POCSAG at 1200 baud");
    assert!(
        (named.deviation_hz.unwrap_or_default() - 4_500.0).abs() < 1_200.0,
        "deviation {:?}",
        named.deviation_hz
    );
}

#[test]
fn a_remote_control_is_keyed_rather_than_shifted() {
    let frame = testgen::subghz::Pwm {
        bits: (0..24).map(|i| 0x0A_1B23u32 >> (23 - i) & 1 == 1).collect(),
        short_us: 320,
        long_multiple: 3,
        sync_gap_multiple: 31,
        repeats: 6,
    };
    let iq = on_air(&testgen::subghz::pwm(&frame, INPUT_RATE_HZ), 2.0, 0x1e44);
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Ook);
    assert!(
        reports
            .iter()
            .any(|r| best(r) == Some("Sub-GHz remote (OOK)")),
        "candidates: {:?}",
        reports.iter().map(best).collect::<Vec<_>>()
    );
}

#[test]
fn a_broadcast_signal_is_wideband_fm() {
    // Programme material rather than a single tone: a pure tone modulates an FM carrier into a
    // spectrum of discrete Bessel lines, which is not what a broadcast station puts on the air
    // and not what the identifier should be tuned against.
    let audio = programme(48_000, 0x4d21);
    let iq = on_air(
        &testgen::wfm::transmission(&audio, &audio, true, INPUT_RATE_HZ),
        1.6,
        0x6f02,
    );
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Fm);
    assert!(
        reports.iter().any(|r| best(r) == Some("FM broadcast")),
        "candidates: {:?}",
        reports.iter().map(best).collect::<Vec<_>>()
    );
}

/// Retuning throws the observation away: half a window of one signal and half of the next would
/// be measured as one thing and reported as a protocol neither of them is.
#[test]
fn a_retune_discards_the_half_window_it_was_holding() {
    let ctx = ChannelCtx {
        input_rate: INPUT_RATE_HZ,
    };
    let mut channel = IdentChannel::new(ctx, settings(params())).expect("builds");
    let mut out = ChannelOutputs::default();
    let half = (INPUT_RATE_HZ * f64::from(INTERVAL_MS) / 1_000.0) as usize / 2;
    channel.process(&complex_noise(0x1122, 0.02, half + 10), &mut out);
    assert!(out.events.is_empty());
    channel.retuned();
    channel.process(&complex_noise(0x3344, 0.02, half), &mut out);
    assert!(
        out.events.is_empty(),
        "the pre-retune samples must not have counted towards this window"
    );
}

/// A performance gate, not a benchmark: one report analyses half a second of signal, and it has
/// to cost a small fraction of that or the identifier cannot share a thread with the radio.
#[test]
fn one_report_costs_far_less_than_the_signal_it_describes() {
    let iq = on_air(
        &tgdv::dmr::transmission(&tgdv::dmr::Call::default(), INPUT_RATE_HZ),
        2.0,
        0x0f31,
    );
    // Warm the plan caches and the allocator before the measured pass.
    let _ = run(params(), &iq);

    let started = Instant::now();
    let reports = run(params(), &iq);
    let elapsed = started.elapsed().as_secs_f64();
    let described = reports.len() as f64 * f64::from(INTERVAL_MS) / 1_000.0;
    assert!(described > 0.0, "the run produced no reports");
    assert!(
        elapsed < described / 4.0,
        "identification took {elapsed:.3} s for {described:.1} s of signal"
    );
}

/// The cadence is what the operator set; the observation is capped. Past the cap they stop being
/// the same number, and the cadence is the one that has to hold.
#[test]
fn a_long_interval_lengthens_the_cadence_rather_than_the_analysis() {
    let long = IdentParams {
        interval_ms: 2_000,
        ..IdentParams::default()
    };
    let noise = complex_noise(0x77c2, 0.02, (INPUT_RATE_HZ * 4.5) as usize);
    assert_eq!(run(long, &noise).len(), 2);
}
