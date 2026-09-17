use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use sdrmm_recorder::SigmfReader;
use sdrmm_wire::{TimeMachineAction, TimeMachineNode};

use crate::Engine;

const NODE: &str = "capture-health-history";

fn settings() -> TimeMachineNode {
    TimeMachineNode {
        history_seconds: super::number("SDRMM_CAPTURE_HISTORY_SECONDS", 1),
    }
}

pub(super) fn check(engine: &Engine, elapsed: Duration) {
    for set in &engine.snapshot().device_sets {
        let history = set.time_machine.as_ref().expect("armed history");
        assert!(
            history.error.is_none(),
            "history {} failed after {elapsed:?}: {:?}",
            set.device.id(),
            history.error
        );
    }
}

pub(super) fn start(engine: &Engine, sets: &[(u32, Vec<u32>)]) {
    let settings = settings();
    for (ds, _) in sets {
        engine
            .control_time_machine(*ds, NODE.to_owned(), 0, TimeMachineAction::Arm, settings)
            .expect("arm history");
    }
    let deadline = Instant::now() + Duration::from_secs(10 + u64::from(settings.history_seconds));
    loop {
        let snapshot = engine.snapshot();
        let ready = snapshot.device_sets.iter().all(|set| {
            let history = set.time_machine.as_ref().expect("armed history");
            assert!(
                history.error.is_none(),
                "history failed: {:?}",
                history.error
            );
            history.held_samples == history.capacity_samples
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "history never filled");
        thread::sleep(Duration::from_millis(10));
    }
    for (ds, _) in sets {
        engine
            .control_time_machine(
                *ds,
                NODE.to_owned(),
                0,
                TimeMachineAction::Capture,
                settings,
            )
            .expect("capture history");
    }
}

pub(super) fn finish(
    engine: &Engine,
    sets: &[(u32, Vec<u32>)],
    directory: &Path,
    seconds: u64,
    allow_drops: bool,
) {
    let settings = settings();
    for (ds, _) in sets {
        let status = engine
            .control_time_machine(*ds, NODE.to_owned(), 0, TimeMachineAction::Stop, settings)
            .expect("finish history capture");
        let capture = status.capture.expect("history recording");
        eprintln!(
            "history ds={ds} samples={} bytes={} overruns={} error={:?}",
            capture.samples, capture.bytes, capture.overruns, status.error
        );
        assert!(status.error.is_none(), "history failed: {:?}", status.error);
        assert!(capture.error.is_none());
        let reader = SigmfReader::open(&directory.join(&capture.file)).expect("history file");
        assert_eq!(reader.total_samples(), capture.samples);
        assert_eq!(
            capture.bytes,
            capture.samples * sdrmm_recorder::BYTES_PER_SAMPLE
        );
        if !allow_drops {
            assert_eq!(capture.overruns, 0);
            assert!(
                capture.samples
                    >= (seconds + u64::from(settings.history_seconds)).saturating_sub(1)
                        * status.sample_rate,
                "history recording is too short"
            );
        }
        engine
            .control_time_machine(*ds, NODE.to_owned(), 0, TimeMachineAction::Disarm, settings)
            .expect("disarm history");
    }
}
