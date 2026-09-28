use std::sync::atomic::AtomicU32;

use super::*;

fn nfm_decoder(engine: &Engine, ds: u32, frequency_hz: f64) -> u32 {
    engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                frequency_hz,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
            },
        )
        .expect("a decoder to scan with")
}

#[tokio::test]
async fn scan_finds_a_carrier_and_holds() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);

    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: 100_000_000.0,
            stop_hz: 100_200_000.0,
            step_hz: 25_000.0,
        }],
        threshold_db: -60.0,
        dwell_ms: 60,
        resume_ms: 60_000,
        ..sdrmm_wire::ScanSettings::for_channel(ch)
    };
    let status = engine.start_scan(ds, settings).unwrap();
    assert_eq!(status.targets, 9);
    assert_eq!(
        status.settings.measure_bw_hz,
        Some(NfmParams::default().bandwidth_hz),
        "the decoder's own bandwidth is what gets measured"
    );

    assert!(
        engine
            .start_scan(ds, sdrmm_wire::ScanSettings::for_channel(ch))
            .is_err(),
        "one scan per decoder"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set
            .scanners
            .first()
            .cloned()
            .expect("scan listed on the set");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break (scanner, set.channels[0].settings.frequency_hz);
        }
        assert!(Instant::now() < deadline, "scan never found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (scanner, parked_hz) = held;
    assert_eq!(scanner.current_hz, SIGNAL_HZ);
    assert!(scanner.hits >= 1);
    assert!(
        (parked_hz - SIGNAL_HZ).abs() < 1.0,
        "the decoder parked at {parked_hz} Hz, carrier at {SIGNAL_HZ} Hz"
    );

    let final_status = engine.stop_scan(ds, ch).unwrap();
    assert_eq!(final_status.state, ScanState::Holding);
    assert!(
        engine.stop_scan(ds, ch).is_err(),
        "double stop must be an error"
    );
    let after = &engine.snapshot().device_sets[0];
    assert!(after.scanners.is_empty());
    let other = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ],
                ..sdrmm_wire::ScanSettings::for_channel(other)
            },
        )
        .expect("another decoder on the same radio can scan");
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ],
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .expect("two decoders scan side by side");
    let both = &engine.snapshot().device_sets[0];
    assert_eq!(
        both.scanners
            .iter()
            .map(|scanner| scanner.settings.channel)
            .collect::<Vec<_>>(),
        vec![ch, other]
    );
    engine.remove_channel(ds, other).unwrap();
    assert_eq!(
        engine.snapshot().device_sets[0].scanners.len(),
        1,
        "removing a decoder stops its scan"
    );
    engine.stop_scan(ds, ch).unwrap();
    assert_eq!(
        after.channels[0].settings.frequency_hz, SIGNAL_HZ,
        "the decoder stays where the scan left it"
    );
    assert!(
        !after.channels[0].out_of_band,
        "the radio follows the decoder once the scan lets go"
    );
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
async fn a_radio_tuned_by_hand_stays_put_and_the_scan_searches_only_its_window() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    hold_tuning(&engine, ds);
    let held_hz = engine.snapshot().device_sets[0].settings.center_hz;
    let beyond = TEST_CENTER_HZ + 10_000_000.0;
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ, beyond],
                threshold_db: -60.0,
                dwell_ms: 40,
                resume_ms: 60_000,
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanners.first().cloned().expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        assert_eq!(
            set.settings.center_hz, held_hz,
            "a scan moved a radio the operator tuned by hand"
        );
        assert_ne!(
            set.channels[0].settings.frequency_hz, beyond,
            "the decoder was sent where the radio cannot hear"
        );
        if scanner.state == ScanState::Holding {
            break;
        }
        assert!(Instant::now() < deadline, "scan never found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    engine.skip_scan(ds, ch).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanners.first().cloned().expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        assert_eq!(set.settings.center_hz, held_hz);
        assert_ne!(set.channels[0].settings.frequency_hz, beyond);
        if scanner.sweeps >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the scan never swept past the window"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    engine.stop_scan(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn the_decoder_follows_the_sweep_and_stays_where_the_scan_stops() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    let targets = [110_000_000.0, 110_025_000.0, 110_050_000.0];
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: targets.to_vec(),
                threshold_db: 100.0,
                dwell_ms: 40,
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanners.first().cloned().expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        let decoder_hz = set.channels[0].settings.frequency_hz;
        if targets.contains(&decoder_hz) {
            assert!(
                !set.channels[0].out_of_band,
                "the decoder follows the scan into the radio's window"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the decoder never followed the scan, still at {decoder_hz} Hz"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let stopped = engine.stop_scan(ds, ch).unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.channels[0].settings.frequency_hz, stopped.current_hz,
        "the decoder stays where the scan stopped"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn skipping_a_held_frequency_resumes_and_never_holds_there_again() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    assert!(
        engine.skip_scan(ds, ch).is_err(),
        "nothing to skip before a scan runs"
    );
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ, SIGNAL_HZ + 25_000.0],
                threshold_db: -60.0,
                dwell_ms: 40,
                resume_ms: 60_000,
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanners
            .first()
            .cloned()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break;
        }
        assert!(Instant::now() < deadline, "scan never found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let skipped = engine.skip_scan(ds, ch).unwrap();
    assert_eq!(skipped.settings.skip, vec![SIGNAL_HZ]);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanners
            .first()
            .cloned()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Scanning && scanner.sweeps >= 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the hold never let go");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let scanner = engine.snapshot().device_sets[0]
        .scanners
        .first()
        .cloned()
        .expect("scan listed");
    assert_eq!(
        scanner.state,
        ScanState::Scanning,
        "held on a skipped frequency"
    );
    assert_eq!(scanner.hits, 1, "the skipped carrier was called again");
    assert!(engine.skip_scan(ds, ch).is_err(), "nothing held to skip");
    engine.stop_scan(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
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
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);

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
        measure_bw_hz: Some(25_000.0),
        ..sdrmm_wire::ScanSettings::for_channel(ch)
    };
    let status = engine.start_scan(ds, settings).unwrap();
    assert!(
        status.hardware_sweep,
        "the scan must take the firmware path"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set
            .scanners
            .first()
            .cloned()
            .expect("scan listed on the set");
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

    engine.stop_scan(ds, ch).unwrap();
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
async fn a_radio_sweeping_in_firmware_refuses_a_retune_by_name() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![marker],
                threshold_db: 100.0,
                dwell_ms: 40,
                measure_bw_hz: Some(25_000.0),
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanners
            .first()
            .cloned()
            .expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        assert!(scanner.hardware_sweep, "the scan must sweep in firmware");
        if scanner.sweeps >= 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the firmware sweep never completed a pass"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(TEST_CENTER_HZ + 1_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("firmware"), "unhelpful: {err}");
    engine.stop_scan(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_hunt_streams_a_strength_a_walker_can_follow() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, SIGNAL_HZ);
    let mut events = engine.subscribe_events();

    assert!(
        engine
            .start_hunt(ds, sdrmm_wire::HuntSettings::for_channel(42))
            .unwrap_err()
            .is_not_found(),
        "a hunt needs a decoder that exists"
    );
    let status = engine
        .start_hunt(
            ds,
            sdrmm_wire::HuntSettings {
                interval_ms: 20,
                ..sdrmm_wire::HuntSettings::for_channel(ch)
            },
        )
        .unwrap();
    assert_eq!(status.readings, 0);
    assert_eq!(
        status.freq_hz, SIGNAL_HZ,
        "the hunt reads the decoder's frequency"
    );
    assert_eq!(status.bw_hz, NfmParams::default().bandwidth_hz);
    assert!(
        engine
            .start_hunt(ds, sdrmm_wire::HuntSettings::for_channel(ch))
            .is_err(),
        "a second hunt must not start"
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
        .hunts
        .first()
        .cloned()
        .expect("the hunt is listed on the set");
    assert!(listed.readings >= 1);

    engine
        .patch_channel(
            ds,
            ch,
            ChannelSettings {
                frequency_hz: SIGNAL_HZ + 50_000.0,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
            },
        )
        .expect("the decoder retunes under a hunt");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let hunt = engine.snapshot().device_sets[0]
            .hunts
            .first()
            .cloned()
            .expect("still hunting");
        assert_eq!(hunt.error, None, "hunt failed");
        if hunt.freq_hz == SIGNAL_HZ + 50_000.0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the hunt never followed the decoder"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let final_status = engine.stop_hunt(ds, ch).unwrap();
    assert!(final_status.readings >= 1);
    assert!(
        engine.stop_hunt(ds, ch).is_err(),
        "double stop must be an error"
    );
    assert!(engine.snapshot().device_sets[0].hunts.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_hunt_leaves_the_dial_alone_and_says_so_when_the_radio_is_off_the_decoder() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, SIGNAL_HZ);
    engine
        .start_hunt(ds, sdrmm_wire::HuntSettings::for_channel(ch))
        .unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(110_000_000.0),
                tuning: Some(sdrmm_wire::Tuning::Manual),
                ..DeviceSettings::default()
            },
        )
        .expect("a hunt does not own the dial");

    let deadline = Instant::now() + Duration::from_secs(10);
    let fault = loop {
        let hunt = engine.snapshot().device_sets[0]
            .hunts
            .first()
            .cloned()
            .expect("the hunt stays listed with its fault");
        if let Some(error) = hunt.error {
            break error;
        }
        assert!(
            Instant::now() < deadline,
            "a hunt that cannot hear stayed silent"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(fault.contains("not tuned over"), "unhelpful fault: {fault}");
    engine.stop_hunt(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_hunt_and_a_scan_do_not_share_a_decoder() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, SIGNAL_HZ);
    engine
        .start_hunt(ds, sdrmm_wire::HuntSettings::for_channel(ch))
        .unwrap();
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ],
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    let other = nfm_decoder(&engine, ds, SIGNAL_HZ);
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![SIGNAL_HZ],
                ..sdrmm_wire::ScanSettings::for_channel(other)
            },
        )
        .expect("another decoder on the hunted radio can still scan");
    engine.stop_scan(ds, other).unwrap();
    engine.stop_hunt(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn close_call_holds_on_the_loudest_carrier_nobody_named() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
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
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();
    assert_eq!(status.settings.mode, sdrmm_wire::ScanMode::CloseCall);

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanners
            .first()
            .cloned()
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
    engine.stop_scan(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn close_call_stays_quiet_on_an_empty_band() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
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
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let scanner = engine.snapshot().device_sets[0]
        .scanners
        .first()
        .cloned()
        .expect("scan listed");
    assert_eq!(scanner.error, None, "scan failed");
    assert_eq!(
        scanner.state,
        ScanState::Scanning,
        "an empty band must not be called"
    );
    assert_eq!(scanner.hits, 0);
    engine.stop_scan(ds, ch).unwrap();
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
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);

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
                ..sdrmm_wire::ScanSettings::for_channel(ch)
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
            .scanners
            .first()
            .cloned()
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

    engine.stop_scan(ds, ch).unwrap();
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
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
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
                measure_bw_hz: Some(25_000.0),
                ..sdrmm_wire::ScanSettings::for_channel(ch)
            },
        )
        .unwrap();
    assert!(status.hardware_sweep);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let scanner = engine.snapshot().device_sets[0]
            .scanners
            .first()
            .cloned()
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

    let stopped = engine.stop_scan(ds, ch).unwrap();
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
    let channel = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    engine.remove_channel(ds, channel).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_sweep_hands_back_a_working_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let marker = sdrmm_device_virtual::SWEEP_MARKER_HZ;
    let channel = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![marker, marker + 100_000.0],
                threshold_db: -50.0,
                dwell_ms: 40,
                resume_ms: 60_000,
                measure_bw_hz: Some(25_000.0),
                ..sdrmm_wire::ScanSettings::for_channel(channel)
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanners.first().cloned().expect("scan listed");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            let parked_hz = set.channels[0].settings.frequency_hz;
            assert!(
                (parked_hz - scanner.current_hz).abs() < 1.0,
                "the rebuilt channel was parked at {parked_hz} Hz, hold at {} Hz",
                scanner.current_hz
            );
            break;
        }
        assert!(Instant::now() < deadline, "never held");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    engine.stop_scan(ds, channel).unwrap();
    assert_eq!(
        engine.snapshot().device_sets[0].channels.len(),
        1,
        "the channel must survive the sweep"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn removing_a_scanning_set_tears_the_scan_down() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
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
                ..sdrmm_wire::ScanSettings::for_channel(ch)
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
    let ch = nfm_decoder(&engine, ds, TEST_CENTER_HZ);
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![2_400_000_000.0],
                ..sdrmm_wire::ScanSettings::for_channel(ch)
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
                ..sdrmm_wire::ScanSettings::for_channel(42)
            },
        )
        .unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
    engine.remove_device_set(ds).unwrap();
}

const SWING_SOURCE_DEG: f64 = 123.0;
const SWING_S: f64 = 5.0;
const ANTENNA_LAG_S: f64 = 0.08;

struct SweptDriver {
    gain_db: Arc<AtomicU32>,
}

impl DeviceDriver for SweptDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("swept", Some("MOCK-SWEPT"))]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SweptDevice {
            capabilities: Capabilities {
                freq_ranges: vec![sdrmm_wire::Range {
                    min: 80_000_000.0,
                    max: 120_000_000.0,
                    step: None,
                }],
                sample_rates: vec![SIGNAL_RATE_HZ],
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(SIGNAL_RATE_HZ),
                ..DeviceSettings::default()
            },
            center: Arc::new(Mutex::new(100_000_000.0)),
            gain_db: self.gain_db.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct SweptDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    center: Arc<Mutex<f64>>,
    gain_db: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SdrDevice for SweptDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(center) = settings.center_hz {
            *lock(&self.center) = center;
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let center = self.center.clone();
        let gain_db = self.gain_db.clone();
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        self.worker = Some(std::thread::spawn(move || {
            let mut phase = 0.0f64;
            let mut block = vec![Complex::new(0.0f32, 0.0); 2_048];
            while !stop.load(Ordering::SeqCst) {
                let step = std::f64::consts::TAU * (SIGNAL_HZ - *lock(&center)) / SIGNAL_RATE_HZ;
                let gain = f32::from_bits(gain_db.load(Ordering::Relaxed));
                let amplitude = 0.5 * 10f64.powf(f64::from(gain) / 20.0);
                for slot in &mut block {
                    phase = (phase + step).rem_euclid(std::f64::consts::TAU);
                    *slot = Complex::new(
                        (amplitude * phase.cos()) as f32,
                        (amplitude * phase.sin()) as f32,
                    );
                }
                sink.push(&block);
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn swing_heading(t: f64) -> f64 {
    10.0 + 120.0 * (t - 1.0).clamp(0.0, 2.5)
}

fn antenna_gain_db(offset_deg: f64) -> f64 {
    let half = 30f64.to_radians().cos();
    let power = 0.5f64.ln() / ((1.0 + half) / 2.0).ln();
    let cosine = offset_deg.to_radians().cos();
    (10.0 * power * ((1.0 + cosine) / 2.0).log10()).max(-15.0)
}

fn pose_facing(heading_deg: f64) -> sdrmm_wire::PositionFix {
    sdrmm_wire::PositionFix {
        latitude: 48.137,
        longitude: 11.575,
        altitude_m: None,
        accuracy_m: Some(5.0),
        speed_mps: None,
        track_deg: None,
        time: jiff::Timestamp::now().to_string(),
        attitude: sdrmm_wire::Attitude {
            heading_deg: Some(heading_deg.rem_euclid(360.0)),
            heading_accuracy_deg: Some(2.0),
            ..sdrmm_wire::Attitude::default()
        },
    }
}

fn wall_ms() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap()
}

#[tokio::test]
async fn hunt_sweep_publishes_a_df_event() {
    let gain_db = Arc::new(AtomicU32::new((-15.0f32).to_bits()));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(SweptDriver {
            gain_db: gain_db.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:swept").unwrap();
    let ch = nfm_decoder(&engine, ds, SIGNAL_HZ);
    let mut decoded = engine.subscribe_decoded();
    assert!(
        engine.hunt_mark(ds, ch).is_err(),
        "no mark before the hunt runs"
    );
    let status = engine
        .sweep_hunt(
            ds,
            ch,
            Some(sdrmm_wire::HuntSettings {
                interval_ms: 20,
                node: Some("hunt-1".to_owned()),
                ..sdrmm_wire::HuntSettings::for_channel(ch)
            }),
        )
        .unwrap();
    assert_eq!(
        status.sweep.map(|sweep| sweep.state),
        Some(sdrmm_wire::SweepState::NoHeading)
    );

    let started = Instant::now();
    loop {
        let t = started.elapsed().as_secs_f64();
        if t > SWING_S {
            break;
        }
        let facing = antenna_gain_db(swing_heading(t - ANTENNA_LAG_S) - SWING_SOURCE_DEG);
        gain_db.store((facing as f32).to_bits(), Ordering::Relaxed);
        engine
            .hunt_pose(ds, ch, pose_facing(swing_heading(t)), wall_ms())
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    let (origin, bearing) = loop {
        match decoded.try_recv() {
            Ok(DecodedRecord {
                origin,
                event: DecoderEvent::Df(bearing),
                ..
            }) => break (origin, bearing),
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "the sweep sent no bearing");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    assert_eq!(origin.map(|origin| origin.node).as_deref(), Some("hunt-1"));
    assert_eq!(bearing.source, sdrmm_wire::BearingSource::Sweep);
    assert_eq!(bearing.lat, Some(48.137));
    let error =
        (f64::from(bearing.bearing_deg) - SWING_SOURCE_DEG + 180.0).rem_euclid(360.0) - 180.0;
    assert!(error.abs() < 10.0, "{bearing:?}");
    let hunt = engine.snapshot().device_sets[0].hunts[0].clone();
    assert_eq!(
        hunt.sweep.and_then(|sweep| sweep.peak_deg),
        Some(bearing.bearing_deg)
    );
    assert!(hunt.at_ms > 0);
    assert_eq!(hunt.pose_drops, 0);

    let stopped = engine
        .start_hunt(ds, sdrmm_wire::HuntSettings::for_channel(ch))
        .unwrap();
    assert_eq!(
        stopped.sweep.map(|sweep| sweep.state),
        Some(sdrmm_wire::SweepState::Off),
        "start on a sweeping hunt turns the sweep off"
    );
    engine.stop_hunt(ds, ch).unwrap();
    engine.remove_device_set(ds).unwrap();
}
