use super::*;

#[tokio::test]
async fn create_emits_state_changed() {
    let engine = virtual_engine();
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("event within timeout")
        .expect("event");
    assert!(matches!(
        ev,
        ServerEvent::StateChanged {
            scope: StateScope::All
        }
    ));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn channel_crud_updates_state() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
    engine.remove_channel(ds, ch).unwrap();
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    assert!(engine.remove_channel(ds, 999).is_err());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn live_position_survives_a_channel_rate_rebuild() {
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(AdsbTestDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("test-adsb:surface").unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Adsb(AdsbParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let fix = PositionFix {
        latitude: 52.52,
        longitude: 13.405,
        altitude_m: Some(40.0),
        accuracy_m: Some(3.0),
        speed_mps: Some(12.0),
        track_deg: Some(90.0),
        time: "2026-08-14T12:00:00Z".to_owned(),
    };
    engine
        .update_channel_position(ds, ch, Some(fix.clone()))
        .unwrap();

    engine
        .patch_channel(
            ds,
            ch,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Adsb(AdsbParams {
                    crc_fix: false,
                    ref_lat: Some(0.0),
                    ref_lon: Some(0.0),
                }),
                audio: Default::default(),
            },
        )
        .unwrap();
    assert_eq!(
        engine.lock().device_sets[&ds].media[&ch].position.as_ref(),
        Some(&fix)
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        engine.lock().device_sets[&ds].media[&ch].position.as_ref(),
        Some(&fix)
    );

    let mut decoded = engine.subscribe_decoded();
    let record = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = decoded.recv().await.expect("decoded stream");
            if matches!(&record.event, DecoderEvent::Adsb(message) if message.lat.is_some()) {
                break record;
            }
        }
    })
    .await
    .expect("post-rebuild local position");
    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!()
    };
    assert!((message.lat.expect("latitude") - fix.latitude).abs() < 0.01);
    assert!((message.lon.expect("longitude") - fix.longitude).abs() < 0.01);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn add_channel_rejects_out_of_passband_offset() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine
        .add_channel(ds, 0, nfm_settings(1_100_000.0))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn add_channel_rejects_audio_settings_outside_their_controls() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let bad: [AudioProcessing; 3] = [
        AudioProcessing {
            blanker: sdrmm_wire::NoiseBlankerSettings {
                enabled: true,
                threshold: 0.2,
            },
            ..AudioProcessing::default()
        },
        AudioProcessing {
            filter: sdrmm_wire::AudioFilterSettings {
                enabled: true,
                low_hz: 3_000.0,
                high_hz: 300.0,
            },
            ..AudioProcessing::default()
        },
        AudioProcessing {
            notches: vec![sdrmm_wire::NotchSettings {
                freq_hz: 1_000.0,
                width_hz: f64::NAN,
            }],
            ..AudioProcessing::default()
        },
    ];
    for audio in bad {
        let settings = ChannelSettings {
            audio: audio.clone(),
            ..nfm_settings(0.0)
        };
        let err = engine.add_channel(ds, 0, settings).unwrap_err();
        assert!(
            err.is_bad_request(),
            "{audio:?}: expected bad request, {err}"
        );
    }
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_channel_with_no_audio_refuses_an_audio_chain() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Pocsag(sdrmm_wire::PocsagParams::default()),
        audio: AudioProcessing {
            agc: sdrmm_wire::AudioAgcMode::Fast,
            ..AudioProcessing::default()
        },
    };
    let err = engine.add_channel(ds, 0, settings).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patching_the_audio_chain_reaches_the_running_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let patched = ChannelSettings {
        audio: AudioProcessing {
            auto_notch: true,
            agc: sdrmm_wire::AudioAgcMode::Slow,
            ..AudioProcessing::default()
        },
        ..nfm_settings(0.0)
    };
    engine.patch_channel(ds, ch, patched.clone()).unwrap();
    let live = &engine.snapshot().device_sets[0].channels[0].settings;
    assert_eq!(live.audio, patched.audio);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patch_channel_rejects_missing_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.patch_channel(ds, 7, nfm_settings(0.0)).unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn rate_change_stranding_a_channel_is_rejected_before_device_io() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.add_channel(ds, 0, nfm_settings(900_000.0)).unwrap();
    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert_eq!(
        engine.snapshot().device_sets[0].settings.sample_rate,
        Some(2_048_000.0)
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_rate_rebuild_and_remove_never_strands_a_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    for i in 0..40u32 {
        let ch = engine.add_channel(ds, 0, nfm_settings(100_000.0)).unwrap();
        let rate = if i % 2 == 0 { 2_400_000.0 } else { 2_048_000.0 };
        let patch = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || {
                engine.patch_device(
                    ds,
                    DeviceSettings {
                        sample_rate: Some(rate),
                        ..Default::default()
                    },
                )
            })
        };
        let remove = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch))
        };
        let (patch, remove) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(patch, remove)
        })
        .await
        .unwrap_or_else(|_| panic!("iteration {i}: patch_device/remove_channel deadlocked"));
        patch.expect("join").expect("patch ok");
        remove.expect("join").expect("remove ok");
        assert!(
            engine.snapshot().device_sets[0].channels.is_empty(),
            "iteration {i}: channel survived its removal"
        );
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn ring_overrun_surfaces_in_state_and_emits_event() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FloodingDriver));
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:flood").unwrap();

    let snap = engine.snapshot();
    assert!(
        snap.device_sets[0].overruns >= mock_ring() as u64,
        "flooded ring must report drops, got {}",
        snap.device_sets[0].overruns
    );

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    let mut quiet = engine.subscribe_events();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert!(
        matches!(quiet.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
        "tick without overrun growth must not emit"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn virtual_capture_recovers_from_a_stalled_dsp_with_an_audio_timestamp_gap() {
    let engine = virtual_engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("virtual radio");
    let ch = engine
        .add_channel(ds, 0, nfm_settings(0.0))
        .expect("channel");
    let mut audio = engine.subscribe_audio(ds, ch).expect("audio");
    let first = tokio::time::timeout(Duration::from_secs(5), audio.recv())
        .await
        .expect("audio starts")
        .expect("packet");
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let mut once = true;
    let command = engine.lock().device_sets[&ds].cmd_txs[0].clone();
    command
        .send(DspCommand::ConnectArray {
            id: 999,
            sink: RxSink::new(move |_, _| {
                if once {
                    once = false;
                    entered.send(()).expect("DSP entered barrier");
                    resume
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release DSP");
                }
            }),
        })
        .expect("install barrier");
    tokio::task::spawn_blocking(move || waiting.recv_timeout(Duration::from_secs(5)))
        .await
        .expect("wait task")
        .expect("DSP blocked");
    let overloaded = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if engine.snapshot().device_sets[0].overruns > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    release.send(()).expect("resume DSP");
    overloaded.expect("capture must report congestion");
    let mut previous = first.timestamp;
    let gap = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let packet = match audio.recv().await {
                Ok(packet) => packet,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(error) => panic!("audio ended: {error}"),
            };
            if packet.timestamp > previous + 960 {
                break packet.timestamp - previous - 960;
            }
            previous = packet.timestamp;
        }
    })
    .await;
    let health = engine.pipeline_health();
    engine.remove_device_set(ds).expect("shutdown");
    assert!(gap.expect("audio must preserve the capture gap") > 0);
    assert!(
        health
            .iter()
            .any(|queue| queue.stage == sdrmm_wire::PipelineStage::Capture
                && queue.health.dropped > 0)
    );
}
