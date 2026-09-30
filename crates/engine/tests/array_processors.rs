#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use common::{
    array::{
        ARRAY, Bench, KRAKEN, LOCK_WAIT, RATE, WAIT, df, kraken_array, next_reading, processor,
        processor_status, wait_calibrated, wait_for, wait_solve_after, wait_status, wrap_deg,
    },
    assert_tone_dominates, settle_then_collect_second,
};
use sdrmm_device_virtual::{BenchWorld, Emitter, Scene, Waveform, default_devices, default_scene};
use sdrmm_engine::{ArraySpec, Engine};
use sdrmm_wire::{
    ArrayCal, ArrayCalSource, ArrayOrientation, ArrayTuneRequest, Attitude, ChannelParams,
    ChannelSettings, NfmParams, PositionFix, ProcessorGate, ProcessorParams, ProcessorReading,
    processor::{
        beamformer::BeamformerParams,
        df::{DfParams, DfReading},
    },
    radar::PassiveRadarParams,
};

const BEARING_DEG: f64 = 137.0;
const VOICE_OFFSET_HZ: f64 = 20_000.0;
const RETUNED_HZ: f64 = 145_000_000.0;

fn noise_emitter() -> Emitter {
    Emitter {
        azimuth_deg: BEARING_DEG,
        elevation_deg: 0.0,
        offset_hz: 0.0,
        power_dbfs: -35.0,
        waveform: Waveform::Noise {
            bandwidth_hz: 200_000.0,
        },
        paths: Vec::new(),
    }
}

fn live_bench() -> Bench {
    let mut scene = default_scene();
    scene.emitters = vec![noise_emitter()];
    scene.echoes.clear();
    Bench::with(scene, default_devices(), None)
}

fn uncalibrated(ds: u32) -> ArraySpec {
    let mut spec = kraken_array(ds);
    spec.settings.cal = ArrayCal {
        source: ArrayCalSource::Off,
        ..ArrayCal::default()
    };
    spec
}

fn beamformer(node: &str) -> sdrmm_engine::ProcessorSpec {
    let mut spec = processor(
        node,
        ProcessorParams::Beamformer(BeamformerParams::default()),
    );
    spec.lane_ports = vec!["beam".to_owned()];
    spec
}

fn df_reading(
    events: &mut tokio::sync::broadcast::Receiver<sdrmm_engine::ArrayEvent>,
) -> DfReading {
    let reading = next_reading(events, "df", WAIT);
    match reading.as_ref() {
        ProcessorReading::Df(df) => df.clone(),
        other => panic!("a direction finder reading: {other:?}"),
    }
}

#[test]
fn a_phase_processor_waits_for_calibration() {
    let bench = live_bench();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(uncalibrated(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let synced = wait_status(
        &bench.engine,
        "time sync on the live signal",
        LOCK_WAIT,
        |now| now.sync == sdrmm_wire::SyncState::Locked,
    );
    assert!(!synced.phase_ready);
    std::thread::sleep(Duration::from_millis(500));
    let waiting = wait_status(&bench.engine, "the phase gate", WAIT, |now| {
        processor_status(now, "df").gated == Some(ProcessorGate::Phase)
    });
    assert!(processor_status(&waiting, "df").gated_samples > 0);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    wait_calibrated(&bench.engine, LOCK_WAIT);
    wait_status(&bench.engine, "the direction finder to run", WAIT, |now| {
        processor_status(now, "df").gated.is_none()
    });
    let mut events = bench.engine.subscribe_arrays();
    df_reading(&mut events);
    let reading = df_reading(&mut events);
    let peak = reading.peaks.first().expect("a bearing");
    assert!(
        wrap_deg(f64::from(peak.relative_deg) - BEARING_DEG).abs() < 1.0,
        "{peak:?}"
    );
}

#[test]
fn a_retuned_array_moves_its_radio_and_keeps_the_bearing() {
    let bench = live_bench();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    bench
        .engine
        .tune_array(
            ARRAY,
            ArrayTuneRequest {
                center_hz: Some(RETUNED_HZ),
                ..ArrayTuneRequest::default()
            },
        )
        .unwrap();
    wait_solve_after(&bench.engine, &before, WAIT);
    let radio = bench.engine.snapshot().device_sets[0].settings.clone();
    assert_eq!(radio.center_hz, Some(RETUNED_HZ));
    wait_status(&bench.engine, "the direction finder to run", WAIT, |now| {
        processor_status(now, "df").gated.is_none()
    });
    let mut events = bench.engine.subscribe_arrays();
    df_reading(&mut events);
    let reading = df_reading(&mut events);
    let peak = reading.peaks.first().expect("the emitter after the retune");
    assert!(
        wrap_deg(f64::from(peak.relative_deg) - BEARING_DEG).abs() < 1.0,
        "{peak:?}"
    );
}

#[test]
fn a_time_only_processor_runs_before_phase_is_solved() {
    let bench = live_bench();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(uncalibrated(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    bench
        .engine
        .apply_processor(processor(
            "radar",
            ProcessorParams::PassiveRadar(PassiveRadarParams::default()),
        ))
        .unwrap();
    let running = wait_status(&bench.engine, "the radar to run", LOCK_WAIT, |now| {
        let radar = processor_status(now, "radar");
        now.sync == sdrmm_wire::SyncState::Locked && radar.running && radar.gated.is_none()
    });
    assert!(!running.phase_ready);
    assert_eq!(
        processor_status(&running, "df").gated,
        Some(ProcessorGate::Phase)
    );
    let mut events = bench.engine.subscribe_arrays();
    let reading = next_reading(&mut events, "radar", WAIT);
    assert!(matches!(
        reading.as_ref(),
        ProcessorReading::PassiveRadar(_)
    ));
}

#[cfg(feature = "probe")]
#[test]
fn a_spread_processor_is_gated_on_a_together_array() {
    use common::array::calibrated;
    use sdrmm_wire::processor::{ProbeParams, stitch::StitchParams};

    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench
        .engine
        .apply_processor(processor(
            "spread",
            ProcessorParams::Probe(ProbeParams {
                time: true,
                spread: true,
                ..ProbeParams::default()
            }),
        ))
        .unwrap();
    let gated = wait_status(&bench.engine, "the tuning gate", LOCK_WAIT, |now| {
        calibrated(now) && processor_status(now, "spread").gated == Some(ProcessorGate::TuningMode)
    });
    assert!(processor_status(&gated, "spread").gated_samples > 0);
    let refused = bench
        .engine
        .apply_processor(processor(
            "stitch",
            ProcessorParams::Stitch(StitchParams::default()),
        ))
        .unwrap_err();
    assert_eq!(refused.to_string(), "Needs spread tuning");
}

fn voice_bench() -> Bench {
    let mut scene: Scene = default_scene();
    scene.emitters = vec![Emitter {
        offset_hz: VOICE_OFFSET_HZ,
        waveform: Waveform::Fm {
            deviation_hz: 2_500.0,
            rate_hz: sdrmm_device_virtual::MOD_TONE_HZ,
        },
        ..noise_emitter()
    }];
    scene.echoes.clear();
    Bench::with(scene, default_devices(), None)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_on_a_lane_port_hears_the_processor_output() {
    let bench = voice_bench();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(beamformer("beam")).unwrap();
    let lane = wait_for("the beam lane", WAIT, || {
        bench.engine.snapshot().device_sets[0]
            .virtual_lanes
            .first()
            .cloned()
    });
    assert_eq!(lane.node, "beam");
    assert_eq!(lane.port, "beam");
    let channel = bench
        .engine
        .add_channel(
            ds,
            lane.stream,
            ChannelSettings {
                frequency_hz: lane.center_hz + VOICE_OFFSET_HZ,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
            },
        )
        .unwrap();
    let engine = bench.engine.clone();
    tokio::task::spawn_blocking(move || wait_calibrated(&engine, LOCK_WAIT))
        .await
        .unwrap();
    let mut audio = bench.engine.subscribe_audio(ds, channel).unwrap();
    let heard = settle_then_collect_second(&mut audio).await;
    assert_tone_dominates(&heard);
}

#[test]
fn two_processors_on_one_array_get_two_virtual_lanes() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(beamformer("beam-a")).unwrap();
    bench.engine.apply_processor(beamformer("beam-b")).unwrap();
    let set = bench.engine.snapshot().device_sets.remove(0);
    let mut owners: Vec<(String, String)> = set
        .virtual_lanes
        .iter()
        .map(|lane| (lane.node.clone(), lane.port.clone()))
        .collect();
    owners.sort();
    assert_eq!(
        owners,
        vec![
            ("beam-a".to_owned(), "beam".to_owned()),
            ("beam-b".to_owned(), "beam".to_owned())
        ]
    );
    assert_ne!(set.virtual_lanes[0].stream, set.virtual_lanes[1].stream);
    assert!(set.virtual_lanes.iter().all(|lane| lane.stream >= 5));
    bench.engine.remove_processor("beam-a").unwrap();
    let left = bench.engine.snapshot().device_sets.remove(0).virtual_lanes;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].node, "beam-b");
}

#[test]
fn df_at_offset_zero_ignores_rtl_dc() {
    let band_hz = DfParams::default().bandwidth_hz;
    let mut scene = default_scene();
    let dc_dbfs = scene.thermal_dbfs + 10.0 * (band_hz / RATE).log10() + 10.0;
    scene.emitters = vec![Emitter {
        power_dbfs: dc_dbfs + 3.0,
        waveform: Waveform::Noise {
            bandwidth_hz: 0.8 * band_hz,
        },
        ..noise_emitter()
    }];
    scene.echoes.clear();
    let bench = Bench::with(scene, default_devices(), None);
    for lane in 0..5 {
        bench.impair(KRAKEN, lane, |impaired| {
            impaired.dc_dbfs = Some(dc_dbfs);
            impaired.dc_phase_deg = 71.0 * lane as f64;
        });
    }
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    wait_calibrated(&bench.engine, LOCK_WAIT);
    wait_status(&bench.engine, "the direction finder to run", WAIT, |now| {
        processor_status(now, "df").gated.is_none()
    });
    let mut events = bench.engine.subscribe_arrays();
    df_reading(&mut events);
    for _ in 0..3 {
        let reading = df_reading(&mut events);
        assert_eq!(reading.sources, 1, "{reading:?}");
        let peak = reading.peaks.first().expect("a bearing");
        assert!(
            wrap_deg(f64::from(peak.relative_deg) - BEARING_DEG).abs() < 1.0,
            "{peak:?}"
        );
    }
}

const TURN_DPS: f64 = 15.0;
const POSE_EVERY: Duration = Duration::from_millis(50);
const POSE_LATE: Duration = Duration::from_millis(400);
const POSE_JITTER_MS: u64 = 100;

fn heading_at(true_s: f64) -> f64 {
    (TURN_DPS * true_s).rem_euclid(360.0)
}

fn spin(world: Arc<BenchWorld>, running: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut scene = world.scene().as_ref().clone();
        while running.load(Ordering::Acquire) {
            let heading = heading_at(world.true_time_s());
            scene.emitters[0].azimuth_deg = (BEARING_DEG - heading).rem_euclid(360.0);
            world.set_scene(scene.clone()).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
    })
}

fn stamp(at: SystemTime) -> String {
    let nanos = at.duration_since(UNIX_EPOCH).unwrap().as_nanos();
    jiff::Timestamp::from_nanosecond(i128::try_from(nanos).unwrap())
        .unwrap()
        .to_string()
}

fn pose(heading_deg: f64, at: SystemTime) -> PositionFix {
    PositionFix {
        latitude: 48.1,
        longitude: 11.5,
        altitude_m: None,
        accuracy_m: Some(3.0),
        speed_mps: Some(0.0),
        track_deg: None,
        time: stamp(at),
        attitude: Attitude {
            heading_deg: Some(heading_deg),
            heading_accuracy_deg: Some(1.0),
            yaw_rate_dps: Some(TURN_DPS),
            ..Attitude::default()
        },
    }
}

fn late_poses(
    engine: Arc<Engine>,
    world: Arc<BenchWorld>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut queue: Vec<(std::time::Instant, PositionFix)> = Vec::new();
        let mut draw = 0x9E37_79B9_u64;
        let mut next = std::time::Instant::now();
        while running.load(Ordering::Acquire) {
            let now = std::time::Instant::now();
            if now >= next {
                draw = draw.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let jitter = Duration::from_millis((draw >> 33) % POSE_JITTER_MS);
                let fix = pose(heading_at(world.true_time_s()), SystemTime::now());
                queue.push((
                    now + POSE_LATE - Duration::from_millis(POSE_JITTER_MS / 2) + jitter,
                    fix,
                ));
                next += POSE_EVERY;
            }
            queue.retain(|(due, fix)| {
                if *due > now {
                    return true;
                }
                let received = i64::try_from(sdrmm_device::now_ns()).unwrap();
                let _ = engine.update_array_pose(ARRAY, Some(fix.clone()), received);
                false
            });
            std::thread::sleep(Duration::from_millis(2));
        }
    })
}

#[test]
fn a_rotating_array_with_late_poses_keeps_true_bearings() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    let mut spec = kraken_array(ds);
    spec.settings.orientation = ArrayOrientation::Heading {
        mount_offset_deg: 0.0,
    };
    bench.engine.apply_array(spec).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let spinner = spin(bench.world.clone(), running.clone());
    let poser = late_poses(bench.engine.clone(), bench.world.clone(), running.clone());
    wait_calibrated(&bench.engine, LOCK_WAIT);
    std::thread::sleep(Duration::from_secs(2));
    let mut events = bench.engine.subscribe_arrays();
    df_reading(&mut events);
    let mut errors = Vec::new();
    for _ in 0..6 {
        let reading = df_reading(&mut events);
        let peak = reading.peaks.first().expect("a bearing");
        let true_deg = peak.true_deg.expect("a true bearing");
        errors.push(wrap_deg(f64::from(true_deg) - BEARING_DEG));
    }
    running.store(false, Ordering::Release);
    spinner.join().unwrap();
    poser.join().unwrap();
    assert!(
        errors.iter().all(|error| error.abs() < 2.0),
        "true bearing errors {errors:?}"
    );
}
