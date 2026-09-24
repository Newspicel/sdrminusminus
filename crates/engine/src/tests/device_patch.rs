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
                tuning: Some(Tuning::Manual),
                ..Default::default()
            },
        )
        .unwrap();

    let usb = |offset_hz: f64| ChannelSettings {
        frequency_hz: TEST_CENTER_HZ + offset_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            bandwidth_hz: 10_000.0,
        }),
        blanker: Default::default(),
    };
    let wide_nfm = |offset_hz: f64| ChannelSettings {
        frequency_hz: TEST_CENTER_HZ + offset_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Nfm(NfmParams {
            bandwidth_hz: 25_000.0,
            ..NfmParams::default()
        }),
        blanker: Default::default(),
    };

    let past_the_edge = engine.add_channel(ds, 0, usb(120_000.0)).unwrap();
    let too_wide = engine.add_channel(ds, 0, wide_nfm(118_000.0)).unwrap();
    let inside_usb = engine.add_channel(ds, 0, usb(-124_000.0)).unwrap();
    let inside_nfm = engine.add_channel(ds, 0, wide_nfm(112_000.0)).unwrap();

    let set = &engine.snapshot().device_sets[0];
    let heard = |id: u32| {
        !set.channels
            .iter()
            .find(|channel| channel.id == id)
            .expect("the channel opened")
            .out_of_band
    };
    assert!(!heard(past_the_edge), "usb sideband runs past the edge");
    assert!(!heard(too_wide), "a 25 kHz channel does not fit there");
    assert!(heard(inside_usb), "the lower sideband fits below the edge");
    assert!(heard(inside_nfm), "a 25 kHz channel fits there");
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
                frequency_hz: 145_025_000.0,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
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
    assert_eq!(set.channels[0].settings.frequency_hz, 145_025_000.0);

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

#[tokio::test]
async fn a_channel_added_mid_tune_does_not_drag_the_radio_back_to_where_it_was() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let tuned = Arc::new(Mutex::new(Vec::new()));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(SlowTunerDriver {
            entered_tx,
            release_rx: Mutex::new(Some(release_rx)),
            tuned: tuned.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:slow-tuner").unwrap();

    let patch = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(460_024_499.0),
                    sample_rate: Some(2_048_000.0),
                    ..DeviceSettings::default()
                },
            )
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let added = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.add_channel(
                ds,
                0,
                ChannelSettings {
                    frequency_hz: 460_137_500.0,
                    squelch: sdrmm_wire::Squelch::Off,
                    params: ChannelParams::Nfm(NfmParams::default()),
                    blanker: Default::default(),
                },
            )
        })
    };

    while engine.snapshot().device_sets[0].channels.is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    release_tx.send(()).unwrap();
    patch.await.expect("join").expect("the retune lands");
    added.await.expect("join").expect("the channel opens");

    let set = engine.snapshot().device_sets.remove(0);
    let asked = set.settings.center_hz.expect("the radio reports a centre");
    assert_eq!(
        lock(&tuned).last().copied(),
        Some(asked),
        "the radio was left tuned somewhere other than where the device set says"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_converter_offset_moves_what_is_shown_and_leaves_the_radio_still() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(TEST_CENTER_HZ),
                ..Default::default()
            },
        )
        .unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                offset_hz: Some(9.75e9),
                ..Default::default()
            },
        )
        .unwrap();

    let set = engine.snapshot().device_sets[0].clone();
    assert_eq!(set.settings.center_hz, Some(TEST_CENTER_HZ + 9.75e9));
    assert_eq!(set.settings.offset_hz, Some(9.75e9));
    assert_eq!(set.capabilities.freq_ranges[0].min, 9.75e9);
    assert_eq!(set.capabilities.freq_ranges[0].max, 15.75e9);
    let radio = engine.capabilities(ds).unwrap();
    assert_eq!(radio.freq_ranges[0].min, 0.0);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn tuning_through_a_converter_reaches_what_the_radio_alone_cannot() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(9.85e9),
                offset_hz: Some(9.75e9),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        engine.snapshot().device_sets[0].settings.center_hz,
        Some(9.85e9)
    );

    let below_the_radio = engine.patch_device(
        ds,
        DeviceSettings {
            center_hz: Some(TEST_CENTER_HZ),
            ..Default::default()
        },
    );
    assert!(
        below_the_radio.is_err(),
        "100 MHz through a 9.75 GHz converter is nothing the radio reaches"
    );
    engine.remove_device_set(ds).unwrap();
}

struct BiasTeeRefusingDriver;

impl DeviceDriver for BiasTeeRefusingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("refusing", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(BiasTeeRefusingDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct BiasTeeRefusingDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for BiasTeeRefusingDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if settings.bias_tee == Some(true) {
            return Err(DeviceError::Io("endpoint stalled".to_string()));
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        single_rx_sink(sinks).map(|_| ())
    }

    fn rx_stop(&mut self) {}
}

#[tokio::test]
async fn a_setting_the_radio_refuses_stays_on_the_set_until_one_is_taken() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(BiasTeeRefusingDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:refusing").unwrap();
    let bias_tee = |on: bool| DeviceSettings {
        bias_tee: Some(on),
        ..DeviceSettings::default()
    };

    engine.patch_device(ds, bias_tee(true)).unwrap_err();
    let refused = engine.snapshot().device_sets[0].refused.clone();
    assert_eq!(
        refused,
        Some(sdrmm_wire::SettingsRefused {
            settings: vec!["bias tee".to_string()],
            error: "device I/O error: endpoint stalled".to_string(),
        })
    );

    engine.patch_device(ds, bias_tee(false)).unwrap();
    assert_eq!(engine.snapshot().device_sets[0].refused, None);
    engine.remove_device_set(ds).unwrap();
}

struct AgcDriver;

impl DeviceDriver for AgcDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("agc", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(AgcDevice {
            capabilities: Capabilities {
                agc: sdrmm_wire::Agc::Switch,
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                agc: Some(sdrmm_wire::AgcSetting::switched(true)),
                ..mock_settings()
            },
        }))
    }
}

struct AgcDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for AgcDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        single_rx_sink(sinks).map(|_| ())
    }

    fn rx_stop(&mut self) {}

    fn agc_gains(&self) -> Result<Vec<sdrmm_wire::AgcGain>, DeviceError> {
        Ok(vec![sdrmm_wire::AgcGain {
            stream: 0,
            value_db: 28.0,
        }])
    }
}

#[tokio::test]
async fn the_gain_an_agc_settled_on_is_read_back_while_it_runs() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(AgcDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:agc").unwrap();
    let tick = || engine.hotplug_tick_for_test(&mut None, &mut HashSet::new());

    tick();
    assert_eq!(
        engine.snapshot().device_sets[0].agc_gains,
        [sdrmm_wire::AgcGain {
            stream: 0,
            value_db: 28.0
        }]
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                agc: Some(sdrmm_wire::AgcSetting::switched(false)),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    tick();
    assert!(engine.snapshot().device_sets[0].agc_gains.is_empty());
    engine.remove_device_set(ds).unwrap();
}
