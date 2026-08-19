use super::*;

#[tokio::test]
async fn record_start_stop_produces_a_finalized_sigmf_pair() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    engine.start_recording(ds, 0).unwrap();
    wait_for_deviceset_event(&mut events, ds).await;
    let live = wait_for_recorded_samples(&engine, ds, 1).await;
    assert!(!live.file.is_empty());
    live.started_at.parse::<jiff::Timestamp>().unwrap();
    assert_eq!(live.error, None);

    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(finalized.error, None);
    assert!(finalized.samples > 0);
    assert_eq!(
        finalized.bytes,
        finalized.samples * sdrmm_recorder::BYTES_PER_SAMPLE
    );
    assert!(engine.snapshot().device_sets[0].recording.is_none());

    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.total_samples(), finalized.samples);
    assert_eq!(reader.meta().global.sample_rate, Some(2_048_000.0));
    assert_eq!(reader.meta().captures[0].frequency, Some(100_000_000.0));

    let playback_id = format!("virtual:file:{}", finalized.stem.display());
    assert!(engine.probe_devices().iter().any(|d| d.id() == playback_id));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn active_recording_persists_live_position_in_sigmf_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    engine
        .update_recording_position(
            ds,
            Some(PositionFix {
                latitude: 52.52,
                longitude: 13.405,
                altitude_m: Some(40.0),
                accuracy_m: Some(3.0),
                speed_mps: Some(5.0),
                track_deg: Some(90.0),
                time: "2026-08-14T12:00:00Z".to_owned(),
            }),
        )
        .unwrap();

    let finalized = engine.stop_recording(ds).unwrap();
    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    let capture = reader.meta().captures.last().unwrap();
    assert_eq!(
        capture.geolocation.as_ref().unwrap().coordinates,
        vec![13.405, 52.52, 40.0]
    );
    assert_eq!(capture.datetime.as_deref(), Some("2026-08-14T12:00:00Z"));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_position_rejects_an_idle_device_set() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let error = engine.update_recording_position(ds, None).unwrap_err();
    assert!(matches!(
        error,
        EngineError::Recording(message) if message == "not recording"
    ));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_position_update_does_not_block_or_panic_during_stop() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
    let stopping_engine = engine.clone();
    std::thread::spawn(move || {
        stopped_tx.send(stopping_engine.stop_recording(ds)).unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match engine.update_recording_position(ds, None) {
            Err(EngineError::Recording(message)) if message == "not recording" => break,
            Ok(()) | Err(EngineError::Recording(_)) => {}
            Err(error) => panic!("unexpected position update error: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "recording stop did not release position state"
        );
        std::thread::yield_now();
    }
    stopped_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("recording stop completed")
        .unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn double_start_and_idle_stop_are_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let err = engine.stop_recording(ds).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    engine.start_recording(ds, 0).unwrap();
    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    engine.stop_recording(ds).unwrap();
    let err = engine.stop_recording(ds).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn start_without_a_recordings_dir_is_rejected() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn rate_patch_is_rejected_while_recording_center_retune_is_captured() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let before = wait_for_recorded_samples(&engine, ds, 1).await;

    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Recording(_)));
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_048_000.0));
    assert!(
        snap.device_sets[0].recording.is_some(),
        "rejected patch must not kill the recording"
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(88_500_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    wait_for_recorded_samples(
        &engine,
        ds,
        before.samples + ring_samples(2_048_000.0) as u64 + 200_000,
    )
    .await;
    let finalized = engine.stop_recording(ds).unwrap();
    engine.remove_device_set(ds).unwrap();

    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    let captures = &reader.meta().captures;
    assert_eq!(captures.len(), 2, "retune must append one capture segment");
    assert_eq!(captures[1].frequency, Some(88_500_000.0));
    assert!(captures[1].sample_start > 0);
}

#[tokio::test]
async fn device_fault_finalizes_the_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set("mock:ondemand").unwrap();

    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    let mut saw_recordings = false;
    let mut saw_device_set = false;
    while !(saw_recordings && saw_device_set) {
        let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        match ev {
            ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            } => saw_recordings = true,
            ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id),
            } if id == ds => saw_device_set = true,
            _ => {}
        }
    }

    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0].recording.is_none(),
        "fault must finalize and clear the recording"
    );
    let reader = sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();
    assert!(reader.total_samples() > 0);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_growth_rides_the_hotplug_tick() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    engine.stop_recording(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn start_during_rate_patch_cannot_commit_a_wrong_rate_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(BlockingApplyDriver {
            entered_tx,
            release_rx: Mutex::new(Some(release_rx)),
        }),
    );
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set("mock:blocking").unwrap();

    let patch = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(2_400_000.0),
                    ..Default::default()
                },
            )
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("in flight"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    release_tx.send(()).unwrap();
    patch.await.expect("join").expect("patch ok");
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
    assert!(snap.device_sets[0].recording.is_none());

    engine.start_recording(ds, 0).unwrap();
    let finalized = engine.stop_recording(ds).unwrap();
    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.meta().global.sample_rate, Some(2_400_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn engine_drop_finalizes_a_live_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    drop(engine);

    let stem = dir.path().join(&live.file);
    assert!(
        sdrmm_recorder::meta_path(&stem).exists(),
        "drop must join the writer and finalize the pair"
    );
    assert!(
        !dir.path()
            .join(format!("{}.sigmf-meta.tmp", live.file))
            .exists(),
        "no breadcrumb may survive an orderly teardown"
    );
    let reader = sdrmm_recorder::SigmfReader::open(&stem).unwrap();
    assert!(reader.total_samples() > 0);
}

#[tokio::test]
async fn shutdown_finalizes_recordings_emits_scopes_and_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    engine.shutdown();
    assert!(engine.snapshot().device_sets.is_empty());
    let mut saw_all = false;
    let mut saw_recordings = false;
    while !(saw_all && saw_recordings) {
        let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        match ev {
            ServerEvent::StateChanged {
                scope: StateScope::All,
            } => saw_all = true,
            ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            } => saw_recordings = true,
            _ => {}
        }
    }
    sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();

    engine.shutdown();
    drop(engine);
}

#[tokio::test]
async fn writer_fault_surfaces_in_state_via_the_hotplug_tick() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    {
        let mut inner = engine.lock();
        let state = inner.device_sets.get_mut(&ds).unwrap();
        state
            .recording
            .as_ref()
            .unwrap()
            .shared
            .fail("recording write failed: injected".to_string());
    }

    let mut events = engine.subscribe_events();
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    let rec = engine.snapshot().device_sets[0].recording.clone().unwrap();
    assert_eq!(
        rec.error.as_deref(),
        Some("recording write failed: injected")
    );

    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(
        finalized.error.as_deref(),
        Some("recording write failed: injected")
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn record_start_on_a_missing_set_is_not_found_even_without_a_recordings_dir() {
    let engine = virtual_engine();
    let err = engine.start_recording(99, 0).unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
}

#[tokio::test]
async fn record_start_io_failure_is_a_server_error_not_a_bad_request() {
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, Some(blocker.path().join("recordings")));
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(matches!(err, EngineError::RecordingIo(_)), "got {err}");
    assert!(!err.is_bad_request() && !err.is_not_found());
    engine.remove_device_set(ds).unwrap();
}
