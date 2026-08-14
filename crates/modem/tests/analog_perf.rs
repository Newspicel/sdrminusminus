#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    analog::{
        AmDemod, AmDetector, AmMod, AmMode, AmParams, AmRx, AngleDemod, AngleDetector, AngleKind,
        AngleMod, AngleParams, AngleRx, Sideband, SsbDemod, SsbDetector, SsbMethod, SsbMod,
        SsbParams,
    },
    ber::{
        analog::tone,
        catalog::analog::{
            NFM_DEVIATION_HZ, TAPS, VOICE_BANDWIDTH, VOICE_RATE_HZ, WFM_DEVIATION_HZ,
            WIDE_BANDWIDTH, WIDE_RATE_HZ,
        },
        perf::{
            CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf,
            host_id, load_baselines, measure_throughput, save_baselines,
        },
    },
};

/// This test binary's allocation counter — `#[global_allocator]` binds per binary, so the library
/// cannot install it on anyone's behalf (see `ber::perf`).
#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

/// Samples one bench call consumes. A tenth of a second of voice audio: long enough that the
/// per-call setup disappears into the measurement, short enough to iterate.
const BLOCK: usize = 4_096;
const TONE_HZ: f64 = 1_000.0;

/// Filter length the deployed rows are benched at — what `channels`'s analog modes configure,
/// and what a real-time factor is therefore a claim about.
const CHANNEL_TAPS: usize = 129;

const AM_STEM: &str = "analog/analog_perf";

fn am_at(mode: AmMode, taps: usize) -> AmParams {
    let mut params = AmParams::new(mode, VOICE_BANDWIDTH);
    params.band_taps = taps;
    params.audio_taps = taps;
    params
}

fn voice_am(mode: AmMode) -> AmParams {
    am_at(mode, CHANNEL_TAPS)
}

fn voice_ssb() -> SsbParams {
    let mut params = SsbParams::new(Sideband::Upper, SsbMethod::Hilbert, VOICE_BANDWIDTH);
    params.band_taps = CHANNEL_TAPS;
    params.audio_taps = CHANNEL_TAPS;
    params
}

fn angle(kind: AngleKind, bandwidth: f64) -> AngleParams {
    let mut params = AngleParams::new(kind, bandwidth);
    params.band_taps = CHANNEL_TAPS;
    params.audio_taps = CHANNEL_TAPS;
    params
}

fn voice_audio() -> Vec<f32> {
    tone(TONE_HZ / VOICE_RATE_HZ, 1.0, BLOCK)
}

fn am_waveform(params: &AmParams) -> Vec<Complex<f32>> {
    let mut out = Vec::new();
    AmMod::new(params).process(&voice_audio(), &mut out);
    out
}

fn ssb_waveform(params: &SsbParams) -> Vec<Complex<f32>> {
    let mut out = Vec::new();
    SsbMod::new(params).process(&voice_audio(), &mut out);
    out
}

fn angle_waveform(params: &AngleParams, rate_hz: f64) -> Vec<Complex<f32>> {
    let mut out = Vec::new();
    AngleMod::new(params).process(&tone(TONE_HZ / rate_hz, 1.0, BLOCK), &mut out);
    out
}

/// Runs `demod` twice to reach steady-state buffer capacity, then measures it.
fn throughput(iters: u64, wave: &[Complex<f32>], mut demod: impl FnMut(&[Complex<f32>])) -> f64 {
    demod(wave);
    demod(wave);
    measure_throughput(iters, wave.len() as u64, || demod(wave))
}

fn measured_baselines() -> Vec<PerfBaseline> {
    let voice = format!(
        "{VOICE_RATE_HZ:.0} Hz, {:.0} Hz message, {CHANNEL_TAPS}-tap filters, {BLOCK}-sample blocks",
        VOICE_BANDWIDTH * VOICE_RATE_HZ
    );

    let am = voice_am(AmMode::FullCarrier { depth: 0.8 });
    let am_wave = am_waveform(&am);
    let mut envelope = AmDemod::new(&am, &AmRx::new(AmDetector::Envelope));
    let mut sink = Vec::with_capacity(BLOCK);
    let envelope_msps = throughput(400, &am_wave, |wave| envelope.process(wave, &mut sink));

    let mut synchronous = AmDemod::new(&am, &AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 }));
    let mut sink = Vec::with_capacity(BLOCK);
    let synchronous_msps = throughput(400, &am_wave, |wave| synchronous.process(wave, &mut sink));

    let ssb = voice_ssb();
    let ssb_wave = ssb_waveform(&ssb);
    let mut sideband = SsbDemod::new(&ssb, SsbDetector::Filter, true);
    let mut sink = Vec::with_capacity(BLOCK);
    let ssb_msps = throughput(400, &ssb_wave, |wave| sideband.process(wave, &mut sink));

    let nfm = angle(
        AngleKind::Fm {
            deviation: NFM_DEVIATION_HZ / VOICE_RATE_HZ,
        },
        VOICE_BANDWIDTH,
    );
    let nfm_wave = angle_waveform(&nfm, VOICE_RATE_HZ);
    let mut discriminator = AngleDemod::new(&nfm, &AngleRx::new(AngleDetector::Discriminator));
    let mut sink = Vec::with_capacity(BLOCK);
    let nfm_msps = throughput(400, &nfm_wave, |wave| {
        discriminator.process(wave, &mut sink)
    });

    let wfm = angle(
        AngleKind::Fm {
            deviation: WFM_DEVIATION_HZ / WIDE_RATE_HZ,
        },
        WIDE_BANDWIDTH,
    );
    let wfm_wave = angle_waveform(&wfm, WIDE_RATE_HZ);
    let mut wide = AngleDemod::new(&wfm, &AngleRx::new(AngleDetector::Discriminator));
    let mut sink = Vec::with_capacity(BLOCK);
    let wfm_msps = throughput(400, &wfm_wave, |wave| wide.process(wave, &mut sink));

    let sharp = am_at(AmMode::FullCarrier { depth: 0.8 }, TAPS);
    let sharp_wave = am_waveform(&sharp);
    let mut sharp_demod = AmDemod::new(&sharp, &AmRx::new(AmDetector::Envelope));
    let mut sink = Vec::with_capacity(BLOCK);
    let sharp_msps = throughput(200, &sharp_wave, |wave| {
        sharp_demod.process(wave, &mut sink)
    });

    vec![
        PerfBaseline {
            bench: "am_envelope_48k".into(),
            msamples_per_s: envelope_msps,
            realtime_factor: envelope_msps * 1e6 / VOICE_RATE_HZ,
            config: format!("{voice}, envelope detector"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "am_synchronous_48k".into(),
            msamples_per_s: synchronous_msps,
            realtime_factor: synchronous_msps * 1e6 / VOICE_RATE_HZ,
            config: format!("{voice}, carrier PLL at 1e-3 cycles/sample"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "ssb_filter_48k".into(),
            msamples_per_s: ssb_msps,
            realtime_factor: ssb_msps * 1e6 / VOICE_RATE_HZ,
            config: format!("{voice}, one-sided complex band filter + product detector"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "nfm_discriminator_48k".into(),
            msamples_per_s: nfm_msps,
            realtime_factor: nfm_msps * 1e6 / VOICE_RATE_HZ,
            config: format!("{voice}, ±{NFM_DEVIATION_HZ:.0} Hz, quadrature discriminator"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "wfm_discriminator_240k".into(),
            msamples_per_s: wfm_msps,
            realtime_factor: wfm_msps * 1e6 / WIDE_RATE_HZ,
            config: format!(
                "{WIDE_RATE_HZ:.0} Hz, {:.0} Hz message, ±{WFM_DEVIATION_HZ:.0} Hz, \
                 {CHANNEL_TAPS}-tap filters, quadrature discriminator",
                WIDE_BANDWIDTH * WIDE_RATE_HZ
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "am_envelope_48k_acceptance".into(),
            msamples_per_s: sharp_msps,
            realtime_factor: sharp_msps * 1e6 / VOICE_RATE_HZ,
            config: format!(
                "{VOICE_RATE_HZ:.0} Hz, {:.0} Hz message, {TAPS}-tap filters, envelope detector \
                 — the SINAD curves' own configuration",
                VOICE_BANDWIDTH * VOICE_RATE_HZ
            ),
            host: host_id(),
        },
    ]
}

fn path(stem: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

#[test]
fn the_am_envelope_path_allocates_nothing() {
    let params = voice_am(AmMode::FullCarrier { depth: 0.8 });
    let wave = am_waveform(&params);
    let mut demod = AmDemod::new(&params, &AmRx::new(AmDetector::Envelope));
    let mut sink = Vec::with_capacity(BLOCK);
    demod.process(&wave, &mut sink);
    demod.process(&wave, &mut sink);
    assert_no_alloc("AmDemod::process (envelope)", || {
        demod.process(&wave, &mut sink);
    });
    assert_eq!(sink.len(), wave.len());
}

/// And the synchronous one, whose carrier loop is the state an allocation would hide behind.
#[test]
fn the_am_synchronous_path_allocates_nothing() {
    let params = voice_am(AmMode::Suppressed);
    let wave = am_waveform(&params);
    let mut demod = AmDemod::new(
        &params,
        &AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 }),
    );
    let mut sink = Vec::with_capacity(BLOCK);
    demod.process(&wave, &mut sink);
    demod.process(&wave, &mut sink);
    assert_no_alloc("AmDemod::process (synchronous)", || {
        demod.process(&wave, &mut sink);
    });
    assert!(demod.lock() > 0.0);
}

/// Both sideband detectors, since they are different code paths: one filter, or two mixers
/// around a lowpass.
#[test]
fn both_sideband_detectors_allocate_nothing() {
    let params = voice_ssb();
    let wave = ssb_waveform(&params);
    for detector in [SsbDetector::Filter, SsbDetector::Weaver] {
        let mut demod = SsbDemod::new(&params, detector, true);
        let mut sink = Vec::with_capacity(BLOCK);
        demod.process(&wave, &mut sink);
        demod.process(&wave, &mut sink);
        assert_no_alloc("SsbDemod::process", || demod.process(&wave, &mut sink));
        assert_eq!(sink.len(), wave.len());
    }
}

/// Both angle detectors, at both kinds — four paths, because the reader is chosen by the pair.
#[test]
fn every_angle_path_allocates_nothing() {
    let kinds = [
        AngleKind::Fm {
            deviation: NFM_DEVIATION_HZ / VOICE_RATE_HZ,
        },
        AngleKind::Pm { deviation_rad: 1.0 },
    ];
    let detectors = [
        AngleDetector::Discriminator,
        AngleDetector::Pll {
            loop_bw: 2.0 * VOICE_BANDWIDTH,
        },
    ];
    for kind in kinds {
        let params = angle(kind, VOICE_BANDWIDTH);
        let wave = angle_waveform(&params, VOICE_RATE_HZ);
        for detector in detectors {
            let mut demod = AngleDemod::new(&params, &AngleRx::new(detector));
            let mut sink = Vec::with_capacity(BLOCK);
            demod.process(&wave, &mut sink);
            demod.process(&wave, &mut sink);
            assert_no_alloc("AngleDemod::process", || demod.process(&wave, &mut sink));
            assert_eq!(sink.len(), wave.len());
        }
    }
}

/// The transmitters too: `tx.rs` drives them from the same hot path a receiver runs on.
#[test]
fn the_modulators_allocate_nothing() {
    let audio = voice_audio();
    let am = voice_am(AmMode::FullCarrier { depth: 0.8 });
    let mut modulator = AmMod::new(&am);
    let mut sink = Vec::with_capacity(BLOCK);
    modulator.process(&audio, &mut sink);
    modulator.process(&audio, &mut sink);
    assert_no_alloc("AmMod::process", || modulator.process(&audio, &mut sink));

    let ssb = voice_ssb();
    let mut modulator = SsbMod::new(&ssb);
    let mut sink = Vec::with_capacity(BLOCK);
    modulator.process(&audio, &mut sink);
    modulator.process(&audio, &mut sink);
    assert_no_alloc("SsbMod::process", || modulator.process(&audio, &mut sink));

    let fm = angle(
        AngleKind::Fm {
            deviation: NFM_DEVIATION_HZ / VOICE_RATE_HZ,
        },
        VOICE_BANDWIDTH,
    );
    let mut modulator = AngleMod::new(&fm);
    let mut sink = Vec::with_capacity(BLOCK);
    modulator.process(&audio, &mut sink);
    modulator.process(&audio, &mut sink);
    assert_no_alloc("AngleMod::process", || modulator.process(&audio, &mut sink));
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_analog_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = path(AM_STEM);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let measured = measured_baselines();
    save_baselines(&path, &measured).unwrap();
    for row in &measured {
        println!(
            "{}: {:.1} Msamples/s, {:.0}x real time",
            row.bench, row.msamples_per_s, row.realtime_factor
        );
    }
}

#[test]
#[ignore = "nightly perf gate; run in release"]
fn compare_analog_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    let committed = load_baselines(&path(AM_STEM)).unwrap();
    if committed.iter().any(|b| b.host != host_id()) {
        eprintln!("skipping the perf gate: baseline host is not {}", host_id());
        return;
    }
    match compare_perf(&measured_baselines(), &committed, REGRESSION_FRACTION) {
        Ok(changes) => {
            for c in changes {
                eprintln!(
                    "{}: {:+.1}% vs baseline ({:.1} -> {:.1} Msamples/s)",
                    c.bench,
                    100.0 * c.change_fraction,
                    c.committed_msamples_per_s,
                    c.measured_msamples_per_s
                );
            }
        }
        Err(regressions) => panic!(
            "throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * REGRESSION_FRACTION
        ),
    }
}

/// Every *deployed* analog receiver must clear its own rate by a wide margin, or a channel could
/// not run several of them at once — which is what the repo actually does. Asserted in release
/// only: a debug build measures the profile, not the engine.
///
/// The acceptance row is held to its own floor and the reason is that it is not a deployment: it
/// runs the SINAD curves' 1023-tap filters, which the reference host clears by about 10% of the
/// deployed floor. Holding it to the same number would make a slower CI host report a machine as
/// a regression, and the committed baseline is what actually watches this row for drift.
#[test]
fn every_analog_receiver_clears_real_time() {
    if cfg!(debug_assertions) {
        return;
    }
    for row in measured_baselines() {
        let floor = if row.bench.ends_with("_acceptance") {
            5.0
        } else {
            20.0
        };
        assert!(
            row.realtime_factor > floor,
            "{}: {:.1}x real time, below its {floor:.0}x floor",
            row.bench,
            row.realtime_factor
        );
    }
}
