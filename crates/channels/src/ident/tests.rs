use std::time::Instant;

use num_complex::Complex;
use sdrmm_wire::{ChannelSettings, DecoderEvent, IdentParams, Modulation};

use super::{INPUT_RATE_HZ, IdentChannel, MAX_WINDOW};
use crate::{
    ChannelCtx, ChannelOutputs, ChannelRx,
    testgen::{self, dv as tgdv},
    testutil::complex_noise,
};

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
        squelch_auto_db: None,
        params: sdrmm_wire::ChannelParams::Ident(params),
        audio: Default::default(),
    }
}

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

fn in_noise(iq: &[Complex<f32>], seconds: f64, seed: u32, amp: f32) -> Vec<Complex<f32>> {
    let wanted = (seconds * INPUT_RATE_HZ) as usize;
    let mut out = Vec::with_capacity(wanted + iq.len());
    while out.len() < wanted {
        out.extend_from_slice(iq);
    }
    let noise = complex_noise(seed, amp, out.len());
    for (sample, noise) in out.iter_mut().zip(noise) {
        *sample += noise;
    }
    out
}

fn on_air(iq: &[Complex<f32>], seconds: f64, seed: u32) -> Vec<Complex<f32>> {
    in_noise(iq, seconds, seed, 0.004)
}

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
fn a_weak_pager_is_a_shift_and_not_amplitude_modulation() {
    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "IDENT TEST".to_owned(),
        numeric: false,
    }];
    let iq = in_noise(
        &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
        2.0,
        0x5d90,
        0.8,
    );
    let reports = run(params(), &iq);
    let loudest = reports.iter().map(|r| r.snr_db).fold(0.0, f32::max);
    assert!(
        loudest < 20.0,
        "meant to be a weak signal, got {loudest} dB"
    );
    assert_eq!(consensus(&reports), Modulation::Fsk2);
    assert!(
        reports.iter().any(|r| r
            .best()
            .is_some_and(|m| m.type_id.as_deref() == Some("pocsag"))),
        "candidates: {:?}",
        reports.iter().map(best).collect::<Vec<_>>()
    );
}

#[test]
fn weak_fm_voice_stays_analog() {
    let audio = programme((INPUT_RATE_HZ * 2.2) as usize, 0x4d21);
    let iq = in_noise(
        &testgen::fm_modulate(&audio, 3_000.0, INPUT_RATE_HZ),
        2.0,
        0x2ea7,
        1.2,
    );
    let reports = run(params(), &iq);
    let loudest = reports.iter().map(|r| r.snr_db).fold(0.0, f32::max);
    assert!(
        loudest < 20.0,
        "meant to be a weak signal, got {loudest} dB"
    );
    assert_eq!(consensus(&reports), Modulation::Fm);
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

fn broadcast_audio(len: usize, seed: u32, smoothing: f32) -> Vec<f32> {
    let mut state = seed | 1;
    let mut smoothed = 0.0f32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = state as f32 / u32::MAX as f32 - 0.5;
            smoothed += smoothing * (noise - smoothed);
            (smoothed * 14.0).clamp(-1.0, 1.0)
        })
        .collect()
}

fn station(smoothing: f32) -> Vec<Complex<f32>> {
    let len = (INPUT_RATE_HZ * 2.2) as usize;
    let mut iq = testgen::wfm::transmission(
        &broadcast_audio(len, 0x4d21, smoothing),
        &broadcast_audio(len, 0x7712, smoothing),
        true,
        INPUT_RATE_HZ,
    );
    testgen::add_noise(&mut iq, 0x6f02, 0.004);
    iq
}

#[test]
fn a_loud_stereo_station_is_broadcast_fm_in_every_window() {
    let reports = run(params(), &station(0.35));
    assert!(reports.len() >= 4, "{} reports", reports.len());
    for report in &reports {
        assert_eq!(report.modulation, Modulation::Fm, "{report:?}");
        assert_eq!(best(report), Some("FM broadcast"));
        assert!(report.confidence > 0.6, "confidence {}", report.confidence);
    }
}

#[test]
fn a_processed_station_is_neither_keyed_nor_shifted() {
    let reports = run(params(), &station(1.0));
    assert!(reports.len() >= 4, "{} reports", reports.len());
    for report in &reports {
        assert_eq!(report.modulation, Modulation::Fm, "{report:?}");
        assert!(
            report.bandwidth_hz > 100_000.0,
            "bandwidth {} Hz",
            report.bandwidth_hz
        );
    }
}

#[test]
fn a_keyed_carrier_is_morse_rather_than_a_bare_carrier() {
    let mut iq = testgen::morse::transmission("CQ CQ DE TEST", 20.0, 800.0, INPUT_RATE_HZ);
    testgen::add_noise(&mut iq, 0x3311, 0.004);
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Ook);
    assert!(
        reports.iter().any(|r| best(r) == Some("Morse (CW)")),
        "candidates: {:?}",
        reports.iter().map(best).collect::<Vec<_>>()
    );
}

#[test]
fn a_deeply_modulated_carrier_is_amplitude_modulation_not_keying() {
    let len = (INPUT_RATE_HZ * 2.2) as usize;
    let mut iq: Vec<Complex<f32>> = testgen::tone_audio(1_000.0, 1.0, INPUT_RATE_HZ, len)
        .iter()
        .map(|&a| Complex::new(0.5 * (1.0 + 0.8 * a), 0.0))
        .collect();
    testgen::add_noise(&mut iq, 0x5511, 0.004);
    let reports = run(params(), &iq);
    assert_eq!(consensus(&reports), Modulation::Am);
    assert!(
        reports.iter().all(|r| r.modulation != Modulation::Ook),
        "an 80 percent modulated carrier dips without ever being keyed off"
    );
    assert_eq!(best(&reports[0]), Some("AM voice"));
}

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

#[test]
fn one_report_costs_far_less_than_the_signal_it_describes() {
    let iq = on_air(
        &tgdv::dmr::transmission(&tgdv::dmr::Call::default(), INPUT_RATE_HZ),
        2.0,
        0x0f31,
    );
    let _ = run(params(), &iq);

    let mut reports = Vec::new();
    let mut elapsed = Vec::with_capacity(3);
    for _ in 0..3 {
        let started = Instant::now();
        reports = run(params(), &iq);
        elapsed.push(started.elapsed());
    }
    elapsed.sort_unstable();
    let elapsed = elapsed[1].as_secs_f64();
    let described = reports.len() as f64 * f64::from(INTERVAL_MS) / 1_000.0;
    assert!(described > 0.0, "the run produced no reports");
    assert!(
        elapsed < described / 2.0,
        "identification took {elapsed:.3} s for {described:.1} s of signal"
    );
}

#[test]
fn a_long_interval_lengthens_the_cadence_rather_than_the_analysis() {
    let long = IdentParams {
        interval_ms: 2_000,
        ..IdentParams::default()
    };
    let noise = complex_noise(0x77c2, 0.02, (INPUT_RATE_HZ * 4.5) as usize);
    assert_eq!(run(long, &noise).len(), 2);

    let ctx = ChannelCtx {
        input_rate: INPUT_RATE_HZ,
    };
    let mut channel = IdentChannel::new(ctx, settings(long)).expect("ident channel");
    let mut out = ChannelOutputs::default();
    channel.process(&noise[..MAX_WINDOW + 1], &mut out);
    assert_eq!(channel.window.len(), MAX_WINDOW);
    assert!(out.events.is_empty());
}
