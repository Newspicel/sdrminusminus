use super::*;

#[tokio::test]
async fn the_time_machine_captures_the_seconds_that_already_went_past() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = TimeMachineNode { history_seconds: 2 };

    let armed = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Arm, settings)
        .unwrap();
    assert_eq!(armed.capacity_samples, 2 * 2_048_000);
    assert_eq!(armed.history_seconds, 2);
    assert!(armed.capture.is_none());
    assert!(
        engine
            .control_time_machine(ds, "tm2".to_owned(), 0, TimeMachineAction::Arm, settings)
            .is_err(),
        "one time machine per radio"
    );

    let held = wait_for_history(&engine, ds, 2_048_000).await;
    assert!(
        held.held_samples >= 2_048_000,
        "a second of history at least"
    );

    let capturing = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Capture, settings)
        .unwrap();
    let capture = capturing.capture.expect("a capture is running");
    assert!(capture.file.starts_with(&format!("tm_{ds}_")));

    let stopped = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Stop, settings)
        .unwrap();
    let finished = stopped.capture.expect("the stop reports what it wrote");
    assert_eq!(finished.file, capture.file);
    assert!(finished.samples >= 2_048_000);
    assert!(
        engine.snapshot().device_sets[0]
            .time_machine
            .as_ref()
            .is_some_and(|history| history.capture.is_none()),
        "the armed history stays, its capture does not"
    );

    let stem = dir.path().join(&capture.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).expect("finalized pair");
    assert!(
        reader.total_samples() >= 2_048_000,
        "the buffered past never reached the file: {} samples",
        reader.total_samples()
    );
    assert_eq!(reader.meta().global.sample_rate, Some(2_048_000.0));

    let disarmed = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Disarm, settings)
        .unwrap();
    assert_eq!(disarmed.node, "tm");
    assert!(engine.snapshot().device_sets[0].time_machine.is_none());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn an_armed_time_machine_locks_the_sample_rate_and_refuses_a_window_that_will_not_fit() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let too_wide = engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Arm,
            TimeMachineNode {
                history_seconds: MAX_TIME_MACHINE_SECONDS,
            },
        )
        .unwrap_err();
    assert!(
        too_wide.to_string().contains("MiB"),
        "the refusal names the memory it would take: {too_wide}"
    );

    engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Arm,
            TimeMachineNode { history_seconds: 1 },
        )
        .unwrap();
    let locked = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap_err();
    assert!(
        locked.to_string().contains("disarm it first"),
        "the refusal says what to do: {locked}"
    );

    engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Disarm,
            TimeMachineNode::default(),
        )
        .unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect("a disarmed radio retunes its rate");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_time_machine_action_names_the_node_that_owns_the_history() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = TimeMachineNode { history_seconds: 1 };

    let idle = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Capture, settings)
        .unwrap_err();
    assert!(idle.to_string().contains("no time machine"));

    engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Arm, settings)
        .unwrap();
    let stranger = engine
        .control_time_machine(
            ds,
            "other".to_owned(),
            0,
            TimeMachineAction::Capture,
            settings,
        )
        .unwrap_err();
    assert!(stranger.to_string().contains("belongs to node `tm`"));

    let idle_stop = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Stop, settings)
        .unwrap_err();
    assert!(idle_stop.to_string().contains("not laying down a capture"));
    engine.remove_device_set(ds).unwrap();
}
