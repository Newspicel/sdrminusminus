use super::*;

#[tokio::test]
async fn validate_honors_configured_bandwidth_and_sideband() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .unwrap();

    let usb = |offset_hz: f64| ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            bandwidth_hz: 10_000.0,
        }),
        audio: Default::default(),
    };
    let wide_nfm = |offset_hz: f64| ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Nfm(NfmParams {
            bandwidth_hz: 25_000.0,
            ..NfmParams::default()
        }),
        audio: Default::default(),
    };

    let err = engine.add_channel(ds, 0, usb(120_000.0)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    let err = engine.add_channel(ds, 0, wide_nfm(118_000.0)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(engine.snapshot().device_sets[0].channels.is_empty());

    engine.add_channel(ds, 0, usb(-124_000.0)).unwrap();
    engine.add_channel(ds, 0, wide_nfm(112_000.0)).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patch_retunes_without_error() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(88_500_000.0),
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.center_hz, Some(88_500_000.0));
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn faulted_set_reconnects_and_restores_its_channels() {
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:ondemand").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(145_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 25_000.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let mut audio = engine.subscribe_audio(ds, ch).unwrap();

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    loop {
        wait_for_deviceset_event(&mut events, ds).await;
        if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
            break;
        }
    }

    die.store(false, Ordering::SeqCst);
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Running);
    assert_eq!(set.error, None);
    assert_eq!(set.settings.center_hz, Some(145_000_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].id, ch);
    assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);

    let packet = tokio::time::timeout(Duration::from_secs(10), audio.recv())
        .await
        .expect("audio within timeout")
        .expect("audio packet after reconnect");
    assert!(!packet.opus.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_patch_reports_what_the_device_holds_not_what_was_asked() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SnappingDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:snapping").unwrap();

    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(100_400_000.0),
                gains: vec![sdrmm_wire::GainValue {
                    stage: "LNA".to_string(),
                    value_db: 13.0,
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
    assert_eq!(
        set.settings
            .gains
            .iter()
            .find(|g| g.stage == "LNA")
            .map(|g| g.value_db),
        Some(16.0),
        "the request was echoed instead of the device's own value"
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                antenna: Some("RX2".to_string()),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.antenna.as_deref(), Some("RX2"));
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_device_that_reports_no_sample_rate_is_refused() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(RatelessDriver));
    let engine = Engine::with_registry(registry, None);

    let err = engine.create_device_set("mock:rateless").unwrap_err();
    assert!(err.to_string().contains("sample rate"), "{err}");
    assert!(engine.snapshot().device_sets.is_empty());
}

#[tokio::test]
async fn a_snapped_rate_is_what_channels_are_rebuilt_on() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SnappingDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:snapping").unwrap();
    let channel = engine
        .add_channel(ds, 0, nfm_settings(0.0))
        .expect("hosted channel");

    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.settings.sample_rate,
        Some(SNAPPED_RATE),
        "the request was echoed instead of the rate the device streams at"
    );
    assert!(
        set.channels.iter().any(|c| c.id == channel),
        "the channel did not survive the rebuild onto the device's rate"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_faulted_set_releases_its_device_so_the_replug_can_reopen_it() {
    let claimed = Arc::new(AtomicBool::new(false));
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(ExclusiveDriver {
            claimed: claimed.clone(),
            die: die.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:exclusive").unwrap();
    assert!(claimed.load(Ordering::SeqCst), "the open must claim it");

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    loop {
        wait_for_deviceset_event(&mut events, ds).await;
        if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
            break;
        }
    }
    assert!(
        !claimed.load(Ordering::SeqCst),
        "the faulted set is still holding the device"
    );

    die.store(false, Ordering::SeqCst);
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Running, "{:?}", set.error);
    assert!(claimed.load(Ordering::SeqCst));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn reconnect_failure_reports_once_and_keeps_the_set_faulted() {
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(UnopenableDriver {
            opens: AtomicUsize::new(0),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:refuse").unwrap();
    engine.mark_device_fault(ds, DeviceError::Io("unplugged".to_string()));
    let mut events = engine.subscribe_events();

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Error);
    let reported = set.error.clone().expect("reason");
    assert!(
        reported.contains("not reopenable") && reported.contains("still claimed"),
        "unhelpful reason: {reported}"
    );
    assert!(
        events.try_recv().is_ok(),
        "the first failure must reach clients"
    );

    while events.try_recv().is_ok() {}
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert!(
        events.try_recv().is_err(),
        "an unchanged reason must not re-invalidate every client"
    );
    engine.remove_device_set(ds).unwrap();
}
