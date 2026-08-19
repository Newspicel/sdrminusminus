use super::*;

#[tokio::test]
async fn channel_levels_are_measured_and_pushed_without_invalidating_state() {
    let engine = virtual_engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("device set");
    let mut events = engine.event_tx.subscribe();
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
        .expect("channel");

    let mut measured = f32::NEG_INFINITY;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let levels = engine.channel_levels(ds);
        if let Some(level) = levels.iter().find(|entry| entry.channel == channel) {
            measured = level.level_db;
            if measured > sdrmm_dsp::LEVEL_FLOOR_DB {
                break;
            }
        }
    }
    assert!(
        measured > sdrmm_dsp::LEVEL_FLOOR_DB,
        "the meter never rose off its floor (read {measured} dB)"
    );
    assert!(measured <= 0.0, "a level above full scale: {measured} dB");

    let levels = engine.channel_levels(ds);
    assert_eq!(levels.len(), 1);
    assert!(
        levels[0].peak_db >= levels[0].level_db,
        "the peak sits below the level it is a peak of"
    );

    while events.try_recv().is_ok() {}
    engine.level_tick();
    let mut pushed = None;
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(event, ServerEvent::StateChanged { .. }),
            "metering invalidated client state"
        );
        if let ServerEvent::ChannelLevels { device_set, levels } = event {
            pushed = Some((device_set, levels));
        }
    }
    let (device_set, levels) = pushed.expect("the tick pushed no levels");
    assert_eq!(device_set, ds);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].channel, channel);

    assert!(engine.channel_levels(ds + 999).is_empty());

    engine.remove_channel(ds, channel).expect("remove channel");
    assert!(!engine.device_sets_with_channels().contains(&ds));
    assert!(engine.channel_levels(ds).is_empty());
}

#[tokio::test]
async fn a_channel_baseband_recording_lands_as_a_sigmf_pair_at_the_channel_rate() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(120_000.0)).unwrap();

    let live = engine.start_channel_baseband_recording(ds, ch).unwrap();
    assert!(live.file.starts_with(&format!("bb_{ds}_{ch}_")));
    assert_eq!(live.error, None);
    assert!(
        engine.start_channel_baseband_recording(ds, ch).is_err(),
        "one baseband recording per channel"
    );
    wait_for_baseband_samples(&engine, ds, ch, 4_800).await;

    let finalized = engine.stop_channel_baseband_recording(ds, ch).unwrap();
    assert_eq!(finalized.error, None);
    assert!(finalized.samples >= 4_800);
    assert_eq!(
        finalized.bytes,
        finalized.samples * sdrmm_recorder::BYTES_PER_SAMPLE
    );
    assert!(
        engine.snapshot().device_sets[0].channels[0]
            .baseband_recording
            .is_none()
    );

    let stem = dir.path().join(&finalized.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).unwrap();
    assert_eq!(reader.meta().global.sample_rate, Some(48_000.0));
    assert_eq!(
        reader.meta().captures[0].frequency,
        Some(100_120_000.0),
        "a channel's baseband is centred on the channel, not the radio"
    );
    assert_eq!(reader.total_samples(), finalized.samples);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn removing_a_channel_finishes_the_baseband_it_was_writing() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let live = engine.start_channel_baseband_recording(ds, ch).unwrap();
    wait_for_baseband_samples(&engine, ds, ch, 480).await;

    engine.remove_channel(ds, ch).unwrap();
    assert!(engine.stop_channel_baseband_recording(ds, ch).is_err());

    let stem = dir.path().join(&live.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).expect("the pair was finalized");
    assert!(reader.total_samples() >= 480);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_channel_network_export_carries_that_channel_and_not_the_radio() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let settings = NetworkExportSettings {
        address: socket.local_addr().unwrap().to_string(),
        ..NetworkExportSettings::default()
    };

    let live = engine
        .start_channel_network_export(ds, ch, "net".to_owned(), settings.clone())
        .unwrap();
    assert_eq!(
        live.sample_rate, 48_000,
        "the channel's rate, not the radio's"
    );
    assert_eq!(live.node, "net");
    assert!(
        engine
            .start_channel_network_export(ds, ch, "other".to_owned(), settings.clone())
            .is_err(),
        "one export per channel"
    );

    let mut buffer = [0u8; 2_048];
    let read = socket.recv(&mut buffer).expect("datagram");
    assert!(
        read > 0 && read.is_multiple_of(8),
        "cf32 pairs arrive whole"
    );

    assert!(
        engine
            .stop_channel_network_export(ds, ch, "someone-else")
            .is_err(),
        "another node cannot stop this export"
    );
    let done = engine.stop_channel_network_export(ds, ch, "net").unwrap();
    assert!(done.bytes > 0);
    assert_eq!(done.error, None);
    assert!(
        engine.snapshot().device_sets[0].channels[0]
            .network_export
            .is_none()
    );
    engine.remove_device_set(ds).unwrap();
}
