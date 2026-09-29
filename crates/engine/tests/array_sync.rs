#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use common::array::{
    ARRAY, ARRAY4, BENCH_RADIUS_M, Bench, DONGLE1, DONGLE2, FULL_RATE, KRAKEN, LOCK_WAIT, WAIT,
    array, calibrated, df, dongle_line, kraken_array, kraken_members, lanes, processor_status,
    status, truth_residual, uca, wait_calibrated, wait_for, wait_solve_after, wait_status,
    wrap_deg,
};
use num_complex::Complex;
use sdrmm_device::{DeviceDriver, RxSink, SdrDevice};
use sdrmm_device_virtual::{
    BenchWorld, Emitter, ReportedGap, Ripple, Slip, VirtualDriver, Waveform, default_devices,
    default_scene,
};
use sdrmm_dsp::array_sync::{
    Boxcar, COARSE_FRAME, COARSE_LAGS, CoarseSearch, EQ_POINTS, coarse_decimation,
};
use sdrmm_engine::{ArrayEvent, ArraySpec};
use sdrmm_wire::{
    ArrayCal, ArrayCalRecord, ArrayCalSource, ArrayFailure, ArrayGain, ArrayNode, ArrayStatus,
    ArrayTune, ArrayTuneRequest, CalPhase, Coherence, DeviceSettings, ProcessorGate, SyncState,
};
use tokio::sync::broadcast::{self, error::TryRecvError};

const CHECK_S: u32 = 10;
const UHF_HZ: f64 = 433.92e6;
const PILOT_HZ: f64 = 100e3;
const EMITTER_HZ: f64 = -150e3;
const EMITTER_BEARING_DEG: f64 = 37.0;

fn with_cal(mut spec: ArraySpec, cal: ArrayCal) -> ArraySpec {
    spec.settings = ArrayNode {
        cal,
        ..spec.settings
    };
    spec
}

fn checked(ds: u32) -> ArraySpec {
    with_cal(
        kraken_array(ds),
        ArrayCal {
            check_s: CHECK_S,
            ..ArrayCal::default()
        },
    )
}

fn dongles(bench: &Bench) -> ArraySpec {
    let first = bench.open_at(DONGLE1, FULL_RATE);
    let second = bench.open_at(DONGLE2, FULL_RATE);
    let mut spec = array(
        lanes(first, [0]),
        ArrayNode {
            geometry: dongle_line(),
            ..ArrayNode::default()
        },
    );
    spec.lanes.extend(lanes(second, [0]));
    spec
}

fn array4(bench: &Bench, source: ArrayCalSource) -> ArraySpec {
    let ds = bench.open_at(ARRAY4, FULL_RATE);
    array(
        lanes(ds, 0..4),
        ArrayNode {
            geometry: uca(BENCH_RADIUS_M),
            declared: Coherence::PhaseCoherent,
            cal: ArrayCal {
                source,
                ..ArrayCal::default()
            },
            ..ArrayNode::default()
        },
    )
}

fn tone(azimuth_deg: f64, offset_hz: f64, power_dbfs: f64) -> Emitter {
    Emitter {
        azimuth_deg,
        elevation_deg: 0.0,
        offset_hz,
        power_dbfs,
        waveform: Waveform::Tone,
        paths: Vec::new(),
    }
}

fn phase_error_at(bench: &Bench, key: &str, solved: &ArrayStatus, offset_hz: f64) -> f64 {
    let reference = bench.truth(key, 0);
    (1..solved.lanes.len())
        .map(|lane| {
            let truth = bench.truth(key, lane);
            let skew = truth.first_sample_s - reference.first_sample_s;
            let expected = truth.phase_deg - reference.phase_deg + 360.0 * offset_hz * skew;
            wrap_deg(f64::from(solved.lanes[lane].phase_deg) - expected).abs()
        })
        .fold(0.0, f64::max)
}

fn solved_record(events: &mut broadcast::Receiver<ArrayEvent>) -> ArrayCalRecord {
    wait_for("a stored calibration", WAIT, || {
        loop {
            match events.try_recv() {
                Ok(ArrayEvent::Solved { array, record }) if array == ARRAY => return Some(record),
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Closed) => panic!("the array events closed"),
            }
        }
    })
}

#[test]
fn a_kraken_like_bank_with_offsets_beyond_2048_locks_and_calibrates() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    let started = Instant::now();
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    assert!(started.elapsed() < LOCK_WAIT);
    let widest = solved
        .lanes
        .iter()
        .map(|lane| lane.delay_samples.abs())
        .fold(0.0, f64::max);
    assert!(widest > 2_048.0, "the bench spreads lanes by {widest}");
    let residual = truth_residual(&bench, &kraken_members(), &solved);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    assert!(residual.gain_db < 0.1, "{residual:?}");
    assert!(
        solved
            .lanes
            .iter()
            .all(|lane| lane.sync == SyncState::Locked)
    );
    assert_eq!(solved.failure, None);
}

#[test]
fn a_silent_slip_is_caught_by_the_next_check() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(checked(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    assert!(before.next_check_in_s.is_some());
    bench.impair(KRAKEN, 2, |lane| {
        lane.slips = vec![Slip { at: 0, samples: 3 }];
    });
    let lost = wait_status(
        &bench.engine,
        "the check to see the slip",
        Duration::from_secs(u64::from(CHECK_S) + 5),
        |now| now.lanes[2].sync == SyncState::Lost,
    );
    assert_ne!(lost.sync, SyncState::Locked);
    let relocked = wait_status(&bench.engine, "the lane to lock again", WAIT, |now| {
        calibrated(now) && now.lanes[2].sync == SyncState::Locked
    });
    let moved = relocked.lanes[2].delay_samples - before.lanes[2].delay_samples;
    assert!((moved.abs() - 3.0).abs() < 0.05, "moved {moved}");
    let residual = truth_residual(&bench, &kraken_members(), &relocked);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert_eq!(status(&bench.engine).failure, None);
}

#[test]
fn a_misestimated_gap_resyncs_instead_of_misaligning() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    bench.impair(KRAKEN, 3, |lane| {
        lane.gaps = vec![ReportedGap {
            at: 0,
            missing: 3_000,
            reported: 2_000,
            error: 1_500,
        }];
    });
    let lost = wait_status(&bench.engine, "the lane to be lost", WAIT, |now| {
        now.lanes[3].uncertain > 0
    });
    assert_eq!(lost.lanes[3].uncertain, 1);
    let relocked = wait_solve_after(&bench.engine, &before, WAIT);
    let residual = truth_residual(&bench, &kraken_members(), &relocked);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    let moved = relocked.lanes[3].delay_samples - before.lanes[3].delay_samples;
    assert!((moved.abs() - 1_000.0).abs() < 0.05, "moved {moved}");
}

#[test]
fn two_single_lane_radios_lock_across_a_200_ms_start_offset() {
    let bench = Bench::new();
    bench.engine.apply_array(dongles(&bench)).unwrap();
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let members = [(DONGLE1, 0), (DONGLE2, 0)];
    let spread = solved.lanes[1].delay_samples.abs();
    assert!(
        (spread - 0.2 * FULL_RATE).abs() < 0.01 * FULL_RATE,
        "{spread}"
    );
    let residual = truth_residual(&bench, &members, &solved);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    assert!(residual.gain_db < 0.1, "{residual:?}");
    assert_eq!(solved.tier, Coherence::TimeSync);
}

#[test]
fn radios_on_different_clocks_report_drift_and_stop_phase_processors() {
    let bench = Bench::new();
    bench.impair(DONGLE2, 0, |lane| lane.ppm = 2.0);
    let mut spec = dongles(&bench);
    spec.tune = Some(ArrayTune {
        center_hz: UHF_HZ,
        gain: ArrayGain::default(),
    });
    bench.engine.apply_array(spec).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let mut seen = Vec::new();
    let drifted = wait_status(&bench.engine, "the clock drift", LOCK_WAIT, |now| {
        if let Some(failure) = &now.failure
            && !seen.contains(failure)
        {
            seen.push(failure.clone());
        }
        matches!(now.failure, Some(ArrayFailure::ClockDrift { .. }))
    });
    assert!(
        seen.iter().all(|failure| !matches!(
            failure,
            ArrayFailure::NoCommonSignal | ArrayFailure::LowCoherence { .. }
        )),
        "{seen:?}"
    );
    let Some(ArrayFailure::ClockDrift { ppm }) = drifted.failure else {
        unreachable!("the wait returned on clock drift");
    };
    assert!((ppm.abs() - 2.0).abs() < 0.1, "{ppm}");
    let stopped = wait_status(&bench.engine, "the tier to drop", WAIT, |now| {
        now.tier == Coherence::None
            && processor_status(now, "df").gated == Some(ProcessorGate::Tier)
    });
    assert!(!stopped.phase_ready);
}

struct Decimated {
    start: u64,
    boxcar: Boxcar,
    out: Vec<Vec<Complex<f32>>>,
    want: usize,
}

impl Decimated {
    fn take(&mut self, samples: &[Complex<f32>], index: u64) {
        let end = index + samples.len() as u64;
        if end <= self.start || self.out[0].len() >= self.want {
            return;
        }
        let from = self.start.saturating_sub(index) as usize;
        let _ = self.boxcar.push(&[&samples[from..]], &mut self.out);
        self.out[0].truncate(self.want);
    }
}

fn decimating_sink(start: u64, factor: usize, want: usize) -> (RxSink, Arc<Mutex<Decimated>>) {
    let held = Arc::new(Mutex::new(Decimated {
        start,
        boxcar: Boxcar::new(1, factor),
        out: vec![Vec::with_capacity(want)],
        want,
    }));
    let writer = held.clone();
    let sink = RxSink::new(move |samples, index| {
        writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(samples, index);
    });
    (sink, held)
}

fn started(driver: &VirtualDriver, key: &str, sink: RxSink) -> Box<dyn SdrDevice> {
    let info = driver
        .probe()
        .into_iter()
        .find(|info| info.key == key)
        .unwrap();
    let mut device = driver.open(&info).unwrap();
    device
        .apply(&DeviceSettings {
            center_hz: Some(UHF_HZ),
            sample_rate: Some(FULL_RATE),
            ..DeviceSettings::default()
        })
        .unwrap();
    device.rx_start(vec![sink]).unwrap();
    device
}

#[test]
fn coarse_finds_a_lag_under_a_1_khz_cfo() {
    let cfo_hz = 1_000.0;
    let mut devices = default_devices();
    let second = devices.iter_mut().find(|spec| spec.key == DONGLE2).unwrap();
    second.lanes[0].ppm = cfo_hz / UHF_HZ * 1e6;
    let world = BenchWorld::new(default_scene(), devices);
    let driver = VirtualDriver::with_world(world.clone());
    let factor = coarse_decimation(FULL_RATE, COARSE_LAGS);
    let span = COARSE_FRAME + 2 * COARSE_LAGS;
    let start = (0.1 * FULL_RATE) as u64;
    let (first_sink, first) = decimating_sink(start, factor, span);
    let (second_sink, lane) = decimating_sink(start, factor, span);
    let mut reference_radio = started(&driver, DONGLE1, first_sink);
    let mut lane_radio = started(&driver, DONGLE2, second_sink);
    reference_radio.set_noise_source(true).unwrap();
    let filled = |held: &Arc<Mutex<Decimated>>| {
        held.lock().unwrap_or_else(PoisonError::into_inner).out[0].len() >= span
    };
    wait_for("both captures", WAIT, || {
        (filled(&first) && filled(&lane)).then_some(())
    });
    reference_radio.rx_stop();
    lane_radio.rx_stop();
    let reference = first.lock().unwrap().out[0].clone();
    let samples = lane.lock().unwrap().out[0].clone();
    let mut search = CoarseSearch::new(COARSE_FRAME, COARSE_LAGS);
    let found = search
        .lag_across_clocks(&reference, &samples, FULL_RATE / factor as f64)
        .unwrap();
    let truth_one = world.lane_truth(DONGLE1, 0).unwrap();
    let truth_two = world.lane_truth(DONGLE2, 0).unwrap();
    let expected = (truth_one.first_sample_s - truth_two.first_sample_s) * FULL_RATE;
    let lag = (found.lag * factor as i64) as f64;
    assert!(
        (lag - expected).abs() <= factor as f64,
        "{lag} vs {expected}"
    );
    assert!((found.cfo_hz - cfo_hz).abs() < 20.0, "{found:?}");
}

#[cfg(feature = "probe")]
#[test]
fn processors_never_see_noise_source_samples() {
    use common::array::processor;
    use sdrmm_channels::array_processor::probe::take_probe_log;
    use sdrmm_wire::{ProcessorParams, processor::ProbeParams};

    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench
        .engine
        .apply_processor(processor(
            "probe",
            ProcessorParams::Probe(ProbeParams {
                time: true,
                ..ProbeParams::default()
            }),
        ))
        .unwrap();
    let mut log = wait_for("the probe log", WAIT, || take_probe_log("probe"));
    let mut solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let mut scene = Vec::new();
    wait_for("scene blocks", WAIT, || {
        scene.extend(log.drain());
        (scene.len() > 20).then_some(())
    });
    let level = scene
        .iter()
        .flat_map(|block| block.power[..block.lanes].to_vec())
        .fold(0.0, f32::max);
    let mut loudest = 0.0f32;
    for _ in 0..5 {
        bench.engine.calibrate_array(ARRAY).unwrap();
        solved = wait_solve_after(&bench.engine, &solved, WAIT);
        for block in log.drain() {
            loudest = block.power[..block.lanes]
                .iter()
                .copied()
                .fold(loudest, f32::max);
            assert_eq!(block.lost, 0);
        }
    }
    assert!(
        loudest < 2.0 * level,
        "a block reached the probe at {loudest}, the scene sits at {level}"
    );
}

#[test]
fn gain_dependent_phase_is_solved_at_operating_gain() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    let Some(gain) = before.gain_db else {
        panic!("the kraken reports its gain: {before:?}");
    };
    bench
        .engine
        .tune_array(
            ARRAY,
            ArrayTuneRequest {
                gain: Some(ArrayGain::Manual { db: gain + 10.0 }),
                ..ArrayTuneRequest::default()
            },
        )
        .unwrap();
    wait_status(&bench.engine, "the phase to go stale", WAIT, |now| {
        !now.phase_ready && matches!(now.cal, CalPhase::Stale | CalPhase::Measuring)
    });
    let after = wait_solve_after(&bench.engine, &before, WAIT);
    assert_eq!(after.gain_db, Some(gain + 10.0));
    let residual = truth_residual(&bench, &kraken_members(), &after);
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    assert!(residual.gain_db < 0.1, "{residual:?}");
}

#[test]
fn a_retune_blanks_and_recalibrates() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    bench
        .engine
        .tune_array(
            ARRAY,
            ArrayTuneRequest {
                center_hz: Some(UHF_HZ),
                ..ArrayTuneRequest::default()
            },
        )
        .unwrap();
    wait_status(&bench.engine, "the retune blank", WAIT, |now| {
        !now.phase_ready && processor_status(now, "df").gated.is_some()
    });
    let after = wait_solve_after(&bench.engine, &before, WAIT);
    assert_eq!(after.center_hz, UHF_HZ);
    assert!(after.generation > before.generation);
    let residual = truth_residual(&bench, &kraken_members(), &after);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    wait_status(&bench.engine, "the direction finder to run", WAIT, |now| {
        processor_status(now, "df").gated.is_none()
    });
}

#[test]
fn a_pilot_calibration_ignores_an_off_air_emitter() {
    let mut scene = default_scene();
    scene.emitters = vec![tone(EMITTER_BEARING_DEG, EMITTER_HZ, -10.0)];
    scene.echoes.clear();
    let bench = Bench::with(scene, default_devices(), None);
    let spec = array4(
        &bench,
        ArrayCalSource::Pilot {
            offset_hz: PILOT_HZ,
            bandwidth_hz: 10e3,
        },
    );
    bench.engine.apply_array(spec).unwrap();
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let error = phase_error_at(&bench, ARRAY4, &solved, PILOT_HZ);
    assert!(error < 1.0, "phase error {error}");
    assert!(solved.lanes.iter().all(|lane| lane.delay_samples == 0.0));
}

#[test]
fn a_known_emitter_recovers_hardware_phase() {
    let mut scene = default_scene();
    scene.emitters = vec![tone(EMITTER_BEARING_DEG, EMITTER_HZ, -20.0)];
    scene.echoes.clear();
    let bench = Bench::with(scene, default_devices(), None);
    let spec = array4(
        &bench,
        ArrayCalSource::Emitter {
            offset_hz: EMITTER_HZ,
            bandwidth_hz: 10e3,
            bearing_deg: EMITTER_BEARING_DEG,
        },
    );
    bench.engine.apply_array(spec).unwrap();
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let error = phase_error_at(&bench, ARRAY4, &solved, EMITTER_HZ);
    assert!(error < 1.0, "phase error {error}");
}

#[test]
fn the_equaliser_flattens_a_lane_ripple() {
    let ripple = Ripple {
        depth_db: 1.0,
        cycles: 3,
    };
    let bench = Bench::new();
    bench.impair(KRAKEN, 2, |lane| lane.ripple = Some(ripple));
    let ds = bench.open(KRAKEN);
    let mut events = bench.engine.subscribe_arrays();
    let spec = with_cal(
        kraken_array(ds),
        ArrayCal {
            equaliser: true,
            ..ArrayCal::default()
        },
    );
    bench.engine.apply_array(spec).unwrap();
    wait_calibrated(&bench.engine, LOCK_WAIT);
    let record = solved_record(&mut events);
    for (lane, solution) in record.solution.iter().enumerate() {
        assert_eq!(solution.equaliser.len(), EQ_POINTS);
        let residual: Vec<f64> = solution
            .equaliser
            .iter()
            .enumerate()
            .filter_map(|(point, [re, im])| {
                let f = point as f64 / EQ_POINTS as f64 - 0.5;
                let shape = if lane == 2 { ripple.gain_at(f) } else { 1.0 };
                let measured = f64::from(re.hypot(*im));
                (f.abs() <= 0.35).then(|| 20.0 * (measured / shape).log10())
            })
            .collect();
        let mean = residual.iter().sum::<f64>() / residual.len() as f64;
        let worst = residual
            .iter()
            .map(|db| (db - mean).abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.2, "lane {lane} leaves {worst} dB");
    }
}

#[test]
fn a_warm_record_skips_the_gain_solve_but_not_the_phase_solve() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    let mut events = bench.engine.subscribe_arrays();
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    let record = solved_record(&mut events);
    bench.engine.remove_array(ARRAY).unwrap();
    let mut warmed = kraken_array(ds);
    warmed.warm = Some(record.clone());
    bench.engine.apply_array(warmed).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let warm = wait_status(&bench.engine, "the warm start", WAIT, |now| {
        now.cal == CalPhase::Warm
    });
    assert!(warm.last_solve_at.is_none());
    assert!(!warm.phase_ready);
    for (lane, solution) in record.solution.iter().enumerate() {
        assert!(
            (f64::from(warm.lanes[lane].gain_db) - solution.gain_db).abs() < 0.01,
            "lane {lane}: {} vs {}",
            warm.lanes[lane].gain_db,
            solution.gain_db
        );
    }
    assert!(processor_status(&warm, "df").gated.is_some());
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let residual = truth_residual(&bench, &kraken_members(), &solved);
    assert!(residual.phase_deg < 1.0, "{residual:?}");
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(first.last_solve_at.is_some());
}
