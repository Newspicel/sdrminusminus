use super::*;

#[tokio::test]
async fn scan_finds_a_carrier_holds_and_owns_the_tuning() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();

    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: 100_000_000.0,
            stop_hz: 100_200_000.0,
            step_hz: 25_000.0,
        }],
        threshold_db: -60.0,
        dwell_ms: 60,
        resume_ms: 60_000,
        hold_channel: Some(ch),
        ..sdrmm_wire::ScanSettings::default()
    };
    let status = engine.start_scan(ds, settings).unwrap();
    assert_eq!(status.targets, 9);

    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        engine
            .start_scan(ds, sdrmm_wire::ScanSettings::default())
            .is_err()
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanner.clone().expect("scan listed on the set");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break (
                scanner,
                set.settings.center_hz.expect("center"),
                set.channels[0].settings.offset_hz,
            );
        }
        assert!(Instant::now() < deadline, "scan never found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (scanner, center_hz, offset_hz) = held;
    assert_eq!(scanner.current_hz, SIGNAL_HZ);
    assert!(scanner.hits >= 1);
    assert!(
        (center_hz + offset_hz - SIGNAL_HZ).abs() < 1.0,
        "hold channel parked at {} Hz, carrier at {SIGNAL_HZ} Hz",
        center_hz + offset_hz
    );

    let final_status = engine.stop_scan(ds).unwrap();
    assert_eq!(final_status.state, ScanState::Holding);
    assert!(
        engine.stop_scan(ds).is_err(),
        "double stop must be an error"
    );
    assert!(engine.snapshot().device_sets[0].scanner.is_none());
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_scan_spans_two_radios_and_each_sweeps_its_own_share() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let a = engine.create_device_set("mock:signal").unwrap();
    let b = engine.create_device_set("mock:signal2").unwrap();

    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: 100_000_000.0,
            stop_hz: 100_200_000.0,
            step_hz: 25_000.0,
        }],
        threshold_db: -60.0,
        dwell_ms: 60,
        resume_ms: 60_000,
        ..sdrmm_wire::ScanSettings::default()
    };
    let session = engine.start_scan_session(&[a, b], settings).unwrap();
    assert_eq!(session.members.len(), 2);
    let shares: Vec<u32> = session.members.iter().map(|m| m.status.targets).collect();
    assert_eq!(shares, vec![5, 4], "nine targets split across two radios");
    assert!(
        session.members[0].status.last_hz < session.members[1].status.first_hz,
        "each radio must get one contiguous band"
    );

    let listed = engine.snapshot();
    let session_view = listed.scan_session.expect("the ganged scan is listed");
    assert_eq!(session_view.device_sets, vec![a, b]);
    assert!(listed.device_sets.iter().all(|set| set.scanner.is_some()));

    assert!(
        engine
            .start_scan(a, sdrmm_wire::ScanSettings::default())
            .is_err(),
        "a second scan must not start while one is running"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let holder = engine.snapshot().device_sets.into_iter().find(|set| {
            set.scanner
                .as_ref()
                .is_some_and(|s| s.state == ScanState::Holding)
        });
        if let Some(set) = holder {
            let scanner = set.scanner.expect("holding");
            assert_eq!(scanner.current_hz, SIGNAL_HZ);
            assert_eq!(set.id, a, "the carrier sits in the first radio's share");
            break;
        }
        for set in engine.snapshot().device_sets {
            assert_eq!(set.scanner.and_then(|s| s.error), None, "scan failed");
        }
        assert!(Instant::now() < deadline, "neither radio found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let stopped = engine.stop_scan_session().unwrap();
    assert_eq!(stopped.members.len(), 2);
    assert!(engine.stop_scan_session().is_err(), "double stop must fail");
    let after = engine.snapshot();
    assert!(after.scan_session.is_none());
    assert!(after.device_sets.iter().all(|set| set.scanner.is_none()));
    engine.remove_device_set(a).unwrap();
    engine.remove_device_set(b).unwrap();
}

#[tokio::test]
async fn stopping_one_radio_leaves_the_rest_of_the_scan_running() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let a = engine.create_device_set("mock:signal").unwrap();
    let b = engine.create_device_set("mock:signal2").unwrap();
    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: 100_000_000.0,
            stop_hz: 100_400_000.0,
            step_hz: 25_000.0,
        }],
        threshold_db: 100.0,
        dwell_ms: 40,
        ..sdrmm_wire::ScanSettings::default()
    };
    engine.start_scan_session(&[a, b], settings).unwrap();
    engine.stop_scan(a).unwrap();

    let listed = engine.snapshot();
    assert_eq!(
        listed.scan_session.expect("still ganged").device_sets,
        vec![b]
    );
    assert!(
        listed
            .device_sets
            .iter()
            .find(|set| set.id == b)
            .is_some_and(|set| set.scanner.is_some()),
        "the other radio must keep sweeping"
    );

    engine.stop_scan(b).unwrap();
    assert!(engine.snapshot().scan_session.is_none());
    engine.remove_device_set(a).unwrap();
    engine.remove_device_set(b).unwrap();
}

#[tokio::test]
async fn a_firmware_sweep_finds_a_carrier_without_the_scanner_retuning() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    assert!(
        engine.sweeps_in_firmware(ds),
        "the virtual radio has to offer a firmware sweep for this to test anything"
    );
    let parked = engine.snapshot().device_sets[0]
        .settings
        .center_hz
        .expect("a tuning");

    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: marker - 200_000.0,
            stop_hz: marker + 200_000.0,
            step_hz: 50_000.0,
        }],
        threshold_db: -50.0,
        dwell_ms: 40,
        resume_ms: 60_000,
        measure_bw_hz: 25_000.0,
        ..sdrmm_wire::ScanSettings::default()
    };
    let status = engine.start_scan(ds, settings).unwrap();
    assert!(
        status.hardware_sweep,
        "the scan must take the firmware path"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanner.clone().expect("scan listed on the set");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break scanner;
        }
        assert!(
            Instant::now() < deadline,
            "the firmware sweep never found the marker"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(held.hardware_sweep, "the sweep stayed in firmware");
    assert!(
        (held.current_hz - marker).abs() <= 50_000.0,
        "held on {} Hz, marker at {marker} Hz",
        held.current_hz
    );
    assert!(held.hits >= 1);

    engine.stop_scan(ds).unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.status,
        DeviceSetStatus::Running,
        "the sweep must hand the receive stream back"
    );
    assert!(set.settings.center_hz.is_some());
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(parked),
                ..DeviceSettings::default()
            },
        )
        .expect("the radio takes a tuning again after a sweep");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_hunt_streams_a_strength_a_walker_can_follow() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let mut events = engine.subscribe_events();

    let status = engine
        .start_hunt(
            ds,
            sdrmm_wire::HuntSettings {
                freq_hz: SIGNAL_HZ,
                bw_hz: 25_000.0,
                interval_ms: 20,
            },
        )
        .unwrap();
    assert_eq!(status.readings, 0);
    assert!(
        engine
            .start_hunt(ds, sdrmm_wire::HuntSettings::default())
            .is_err(),
        "a second hunt must not start"
    );
    assert!(
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(101_000_000.0),
                    ..DeviceSettings::default()
                },
            )
            .is_err(),
        "a hunt owns the dial while it runs"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = None;
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(ServerEvent::HuntUpdate { device_set, status }) => {
                assert_eq!(device_set, ds);
                assert_eq!(status.error, None, "hunt failed");
                if status.readings >= 3 {
                    seen = Some(*status);
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    let seen = seen.expect("the hunt never reported a reading");
    let level = seen.level_db.expect("a level");
    assert!(level > -80.0, "the carrier read as {level} dB");
    assert!(seen.smooth_db.is_some());
    assert!((0.0..=1.0).contains(&seen.strength));

    let listed = engine.snapshot().device_sets[0]
        .hunt
        .clone()
        .expect("the hunt is listed on the set");
    assert!(listed.readings >= 1);

    let final_status = engine.stop_hunt(ds).unwrap();
    assert!(final_status.readings >= 1);
    assert!(
        engine.stop_hunt(ds).is_err(),
        "double stop must be an error"
    );
    assert!(engine.snapshot().device_sets[0].hunt.is_none());
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(100_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect("the dial comes back after a hunt");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_hunt_and_a_scan_do_not_share_a_dial() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    engine
        .start_hunt(
            ds,
            sdrmm_wire::HuntSettings {
                freq_hz: SIGNAL_HZ,
                ..sdrmm_wire::HuntSettings::default()
            },
        )
        .unwrap();
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ],
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.stop_hunt(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn close_call_holds_on_the_loudest_carrier_nobody_named() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    let status = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                mode: sdrmm_wire::ScanMode::CloseCall,
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: marker - 400_000.0,
                    stop_hz: marker + 400_000.0,
                    step_hz: 200_000.0,
                }],
                margin_db: 12.0,
                dwell_ms: 60,
                resume_ms: 60_000,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    assert_eq!(status.settings.mode, sdrmm_wire::ScanMode::CloseCall);

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanner
            .clone()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break scanner;
        }
        assert!(Instant::now() < deadline, "close call never fired");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        (held.current_hz - marker).abs() < 50_000.0,
        "called {} Hz, carrier at {marker} Hz",
        held.current_hz
    );
    assert!(held.hits >= 1);
    engine.stop_scan(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn close_call_stays_quiet_on_an_empty_band() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                mode: sdrmm_wire::ScanMode::CloseCall,
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: 110_000_000.0,
                    stop_hz: 110_200_000.0,
                    step_hz: 50_000.0,
                }],
                margin_db: 40.0,
                dwell_ms: 40,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let scanner = engine.snapshot().device_sets[0]
        .scanner
        .clone()
        .expect("scan listed");
    assert_eq!(scanner.error, None, "scan failed");
    assert_eq!(
        scanner.state,
        ScanState::Scanning,
        "an empty band must not be called"
    );
    assert_eq!(scanner.hits, 0);
    engine.stop_scan(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_refused_firmware_sweep_falls_back_to_retuning_without_losing_the_radio() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(RefusedSweepDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:refuses-sweep").unwrap();
    assert!(
        engine.sweeps_in_firmware(ds),
        "this radio has to claim a firmware sweep for the fallback to be exercised"
    );

    let status = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: 100_000_000.0,
                    stop_hz: 100_200_000.0,
                    step_hz: 25_000.0,
                }],
                threshold_db: -60.0,
                dwell_ms: 60,
                resume_ms: 60_000,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    assert!(
        status.hardware_sweep,
        "the scan set out to use the firmware"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanner
            .clone()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break scanner;
        }
        assert!(
            Instant::now() < deadline,
            "the fallback never found the carrier"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        !held.hardware_sweep,
        "a refused firmware sweep must be reported as retuning, not left looking like a slow one"
    );
    assert_eq!(held.current_hz, SIGNAL_HZ);

    engine.stop_scan(ds).unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.status,
        DeviceSetStatus::Running,
        "asking for a sweep must never cost the receive stream"
    );
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect("the radio still takes a tuning");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn stopping_mid_sweep_hands_the_radio_back() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    let status = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: marker - 500_000.0,
                    stop_hz: marker + 500_000.0,
                    step_hz: 50_000.0,
                }],
                threshold_db: 100.0,
                dwell_ms: 40,
                measure_bw_hz: 25_000.0,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    assert!(status.hardware_sweep);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanner
            .clone()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.sweeps >= 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the firmware sweep never completed a pass"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let stopped = engine.stop_scan(ds).unwrap();
    assert_eq!(stopped.state, ScanState::Scanning, "stopped mid-sweep");
    assert!(stopped.sweeps >= 1, "the pass counter must advance");
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Running);
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(marker),
                ..DeviceSettings::default()
            },
        )
        .expect("the radio takes a tuning again after a sweep it never finished");
    let channel = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .expect("the receive stream is back");
    engine.remove_channel(ds, channel).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_sweep_hands_back_a_working_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    let channel = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![marker, marker + 100_000.0],
                threshold_db: -50.0,
                dwell_ms: 40,
                resume_ms: 60_000,
                measure_bw_hz: 25_000.0,
                hold_channel: Some(channel),
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanner.clone().expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            let center = set.settings.center_hz.expect("center");
            let offset = set.channels[0].settings.offset_hz;
            assert!(
                (center + offset - scanner.current_hz).abs() < 1.0,
                "the rebuilt channel was parked at {} Hz, hold at {} Hz",
                center + offset,
                scanner.current_hz
            );
            break;
        }
        assert!(Instant::now() < deadline, "never held");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    engine.stop_scan(ds).unwrap();
    assert_eq!(
        engine.snapshot().device_sets[0].channels.len(),
        1,
        "the channel must survive the sweep"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_scan_refuses_more_radios_than_it_has_targets() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let a = engine.create_device_set("mock:signal").unwrap();
    let b = engine.create_device_set("mock:signal2").unwrap();
    let err = engine
        .start_scan_session(
            &[a, b],
            sdrmm_wire::ScanSettings {
                frequencies: vec![100_000_000.0],
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        err.to_string().contains("cannot be spread"),
        "unhelpful message: {err}"
    );
    assert!(engine.snapshot().scan_session.is_none());
    assert!(
        engine
            .start_scan_session(
                &[a, a],
                sdrmm_wire::ScanSettings {
                    frequencies: vec![100_000_000.0, 100_100_000.0],
                    ..sdrmm_wire::ScanSettings::default()
                },
            )
            .is_err(),
        "one radio listed twice must be refused"
    );
    engine.remove_device_set(a).unwrap();
    engine.remove_device_set(b).unwrap();
}

#[tokio::test]
async fn removing_a_scanning_set_tears_the_scan_down() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: 100_000_000.0,
                    stop_hz: 100_400_000.0,
                    step_hz: 25_000.0,
                }],
                threshold_db: 100.0,
                dwell_ms: 40,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    engine.remove_device_set(ds).unwrap();
    assert!(engine.snapshot().device_sets.is_empty());
}

#[tokio::test]
async fn scan_rejects_targets_the_tuner_cannot_reach() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![2_400_000_000.0],
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        err.to_string().contains("tuning range"),
        "unhelpful message: {err}"
    );
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![100_000_000.0],
                hold_channel: Some(42),
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
    engine.remove_device_set(ds).unwrap();
}
