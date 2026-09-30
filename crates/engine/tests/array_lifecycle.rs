#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::array::{
    ARRAY, Bench, DONGLE1, DONGLE2, FULL_RATE, KRAKEN, LOCK_WAIT, RATE, WAIT, array, df,
    dongle_line, kraken_array, kraken_members, lanes, next_reading, processor_status, status,
    truth_residual, uca, wait_calibrated, wait_solve_after, wait_status,
};
use sdrmm_wire::{
    ArrayFailure, ArrayNode, DeviceSettings, ProcessorReading, StreamSettings, SyncState,
};

fn df_bearing(bench: &Bench) -> f32 {
    let mut events = bench.engine.subscribe_arrays();
    next_reading(&mut events, "df", WAIT);
    let reading = next_reading(&mut events, "df", WAIT);
    let ProcessorReading::Df(df) = reading.as_ref() else {
        panic!("a direction finder reading: {reading:?}");
    };
    df.peaks.first().map_or(f32::NAN, |peak| peak.relative_deg)
}

#[test]
fn replugging_a_member_keeps_the_array_and_resyncs_it() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    bench.pull(KRAKEN);
    let down = wait_status(&bench.engine, "the radio to go down", WAIT, |now| {
        matches!(now.failure, Some(ArrayFailure::DeviceDown { .. }))
    });
    assert_eq!(down.node, ARRAY);
    bench.plug(KRAKEN);
    let back = wait_solve_after(&bench.engine, &first, LOCK_WAIT);
    assert_eq!(back.node, ARRAY);
    assert_eq!(back.failure, None);
    assert_eq!(bench.engine.array_statuses().len(), 1);
    let residual = truth_residual(&bench, &kraken_members(), &back);
    assert!(residual.delay < 0.05, "{residual:?}");
    assert!(residual.phase_deg < 1.0, "{residual:?}");
}

#[test]
fn a_member_fault_shows_device_down_and_gates_processors() {
    let bench = Bench::new();
    let first = bench.open(DONGLE1);
    let second = bench.open(DONGLE2);
    let mut spec = array(
        lanes(first, [0]),
        ArrayNode {
            geometry: dongle_line(),
            ..ArrayNode::default()
        },
    );
    spec.lanes.extend(lanes(second, [0]));
    bench.engine.apply_array(spec).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    wait_calibrated(&bench.engine, LOCK_WAIT);
    wait_status(&bench.engine, "the direction finder to run", WAIT, |now| {
        processor_status(now, "df").gated.is_none()
    });
    bench.pull(DONGLE2);
    let down = wait_status(&bench.engine, "the second radio to go down", WAIT, |now| {
        now.failure == Some(ArrayFailure::DeviceDown { lane: 1 })
    });
    assert_eq!(down.lanes[1].sync, SyncState::Lost);
    wait_status(&bench.engine, "the direction finder to stop", WAIT, |now| {
        processor_status(now, "df").gated.is_some()
    });
}

#[test]
fn a_rate_change_rebuilds_processors_and_resizes_taps() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    assert_eq!(before.sample_rate, RATE);
    let bearing = df_bearing(&bench);
    assert!((bearing - 137.0).abs() < 1.0, "{bearing}");
    bench
        .engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(FULL_RATE),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let after = wait_solve_after(&bench.engine, &before, LOCK_WAIT);
    assert_eq!(after.sample_rate, FULL_RATE);
    let residual = truth_residual(&bench, &kraken_members(), &after);
    assert!(residual.delay < 0.05, "{residual:?}");
    let df = wait_status(&bench.engine, "the rebuilt direction finder", WAIT, |now| {
        let df = processor_status(now, "df");
        df.running && df.gated.is_none()
    });
    assert_eq!(processor_status(&df, "df").error, None);
    let bearing = df_bearing(&bench);
    assert!((bearing - 137.0).abs() < 1.0, "{bearing}");
    let gaps: Vec<u64> = status(&bench.engine)
        .lanes
        .iter()
        .map(|lane| lane.gaps)
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    let later: Vec<u64> = status(&bench.engine)
        .lanes
        .iter()
        .map(|lane| lane.gaps)
        .collect();
    assert_eq!(gaps, later, "the taps keep up at the new rate");
}

#[test]
fn lanes_held_by_one_array_are_refused_to_another() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    let mut first = kraken_array(ds);
    first.lanes = lanes(ds, 0..3);
    first.settings.geometry = uca(0.3);
    bench.engine.apply_array(first).unwrap();
    let mut second = kraken_array(ds);
    second.node = "array-2".to_owned();
    second.lanes = lanes(ds, 2..5);
    second.settings.geometry = uca(0.3);
    let refused = bench.engine.apply_array(second.clone()).unwrap_err();
    assert_eq!(refused.to_string(), format!("Lane 1 in {ARRAY}"));
    let retune = bench.engine.patch_device(
        ds,
        DeviceSettings {
            streams: vec![StreamSettings {
                stream: 0,
                center_hz: Some(433.92e6),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        },
    );
    assert_eq!(retune.unwrap_err().to_string(), format!("Tuned by {ARRAY}"));
    assert_eq!(bench.engine.array_statuses().len(), 1);
    bench.engine.remove_array(ARRAY).unwrap();
    bench.engine.apply_array(second).unwrap();
    let held: Vec<(u32, String)> = bench.engine.snapshot().device_sets[0]
        .held
        .iter()
        .map(|lane| (lane.stream, lane.array.clone()))
        .collect();
    assert_eq!(
        held,
        (2..5)
            .map(|stream| (stream, "array-2".to_owned()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_failed_processor_build_leaves_the_array_running() {
    let bench = Bench::new();
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    bench.engine.apply_processor(df("df")).unwrap();
    let mut broken = df("df-broken");
    broken.lane_ports = vec!["nope".to_owned()];
    let refused = bench.engine.apply_processor(broken).unwrap_err();
    assert_eq!(refused.to_string(), "No port nope");
    let solved = wait_calibrated(&bench.engine, LOCK_WAIT);
    let broken = processor_status(&solved, "df-broken");
    assert!(!broken.running);
    assert_eq!(broken.error.as_deref(), Some("No port nope"));
    wait_status(&bench.engine, "the working direction finder", WAIT, |now| {
        let df = processor_status(now, "df");
        df.running && df.gated.is_none()
    });
    let bearing = df_bearing(&bench);
    assert!((bearing - 137.0).abs() < 1.0, "{bearing}");
    assert_eq!(status(&bench.engine).failure, None);
}

#[cfg(feature = "probe")]
mod probed {
    use std::time::{Duration, Instant};

    use super::common::array::{
        ARRAY, Bench, KRAKEN, LOCK_WAIT, RATE, WAIT, kraken_array, processor, processor_status,
        status, wait_calibrated, wait_for, wait_status,
    };
    use sdrmm_wire::{ArrayFailure, ProcessorParams, processor::ProbeParams};

    fn probe(params: ProbeParams) -> ProcessorParams {
        ProcessorParams::Probe(params)
    }

    #[test]
    fn removing_an_array_never_blocks_the_engine() {
        let bench = Bench::new();
        let ds = bench.open(KRAKEN);
        bench.engine.apply_array(kraken_array(ds)).unwrap();
        bench
            .engine
            .apply_processor(processor(
                "slow",
                probe(ProbeParams {
                    block_drop_ms: 500,
                    ..ProbeParams::default()
                }),
            ))
            .unwrap();
        let remover = {
            let engine = bench.engine.clone();
            std::thread::spawn(move || engine.remove_array(ARRAY))
        };
        wait_for("the array to leave", WAIT, || {
            bench.engine.array_statuses().is_empty().then_some(())
        });
        let slow_drop = Instant::now() + Duration::from_millis(600);
        while Instant::now() < slow_drop {
            let started = Instant::now();
            let snapshot = bench.engine.snapshot();
            assert!(
                started.elapsed() < Duration::from_millis(50),
                "a snapshot waited {:?}",
                started.elapsed()
            );
            assert!(snapshot.arrays.is_empty());
            std::thread::sleep(Duration::from_millis(5));
        }
        remover.join().unwrap().unwrap();
        assert!(bench.engine.snapshot().device_sets[0].held.is_empty());
    }

    #[test]
    fn every_drop_shows_up_in_the_status() {
        let bench = Bench::new();
        let ds = bench.open(KRAKEN);
        bench.engine.apply_array(kraken_array(ds)).unwrap();
        bench
            .engine
            .apply_processor(processor("probe", probe(ProbeParams::default())))
            .unwrap();
        let before = wait_calibrated(&bench.engine, LOCK_WAIT);
        assert!(before.lanes.iter().all(|lane| lane.gaps == 0));
        let pause = Duration::from_millis(400);
        bench
            .engine
            .hold_array(ARRAY, move || std::thread::sleep(pause))
            .unwrap();
        let dropped = wait_status(&bench.engine, "the overflow to be counted", WAIT, |now| {
            now.lanes.iter().all(|lane| lane.gaps > 0)
        });
        let lost = 0.2 * RATE;
        for lane in &dropped.lanes {
            assert!(
                lane.gap_samples as f64 > lost,
                "lane {} counts {} missing samples",
                lane.lane,
                lane.gap_samples
            );
        }
        let probe = processor_status(&dropped, "probe");
        assert_eq!(probe.dropped_samples, 0);
        wait_status(&bench.engine, "the array to settle", WAIT, |now| {
            processor_status(now, "probe").gated.is_none()
        });
    }

    #[test]
    fn a_dead_aggregator_is_reported_as_stopped() {
        let bench = Bench::new();
        let ds = bench.open(KRAKEN);
        bench.engine.apply_array(kraken_array(ds)).unwrap();
        bench
            .engine
            .apply_processor(processor("probe", probe(ProbeParams::default())))
            .unwrap();
        wait_calibrated(&bench.engine, LOCK_WAIT);
        bench
            .engine
            .hold_array(ARRAY, || {
                std::panic::panic_any("aggregator lost".to_owned())
            })
            .unwrap();
        let stopped = wait_status(&bench.engine, "the stop", WAIT, |now| {
            matches!(now.failure, Some(ArrayFailure::Stopped { .. }))
        });
        assert_eq!(
            stopped.failure,
            Some(ArrayFailure::Stopped {
                message: "aggregator lost".to_owned()
            })
        );
        assert!(!processor_status(&stopped, "probe").running);
        assert_eq!(status(&bench.engine).node, ARRAY);
    }
}
