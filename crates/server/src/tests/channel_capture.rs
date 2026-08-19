use super::*;

#[tokio::test]
async fn channel_audio_record_list_download_and_delete_roundtrip_over_http() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

    let (status, body) = record_channel(&app, ds, ch, "start").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let live: sdrmm_wire::AudioRecordingStatus = serde_json::from_slice(&body).expect("json");
    assert!(live.file.ends_with(".wav"));
    assert_eq!(live.channels, 1);
    live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

    wait_for_recorded_frames(&app, ds, ch, 4_800).await;

    let (status, body) = record_channel(&app, ds, ch, "stop").await;
    assert_eq!(status, StatusCode::OK);
    let done: sdrmm_wire::AudioRecordingStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(done.file, live.file);
    assert!(done.frames > 0);
    assert_eq!(done.bytes, done.frames * 2);
    assert_eq!(done.error, None);

    let listed = list_audio_recordings(&app).await;
    assert_eq!(listed.len(), 1);
    let rec = &listed[0];
    assert_eq!(rec.file, done.file);
    assert_eq!((rec.channels, rec.sample_rate), (1, 48_000));
    assert_eq!(rec.frames, done.frames);
    assert!(rec.duration_s > 0.0);
    rec.created_at
        .parse::<jiff::Timestamp>()
        .expect("rfc3339 created_at");

    let (status, wav) = request(
        app.clone(),
        "GET",
        &format!("/api/audiorecordings/{}/download", rec.file),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len() as u64, 44 + rec.bytes);

    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/audiorecordings/{}", rec.file),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(list_audio_recordings(&app).await.is_empty());
    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/audiorecordings/{}", rec.file),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_audio_recording_name_cannot_reach_outside_its_directory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("secret.wav"), b"not yours").expect("plant");
    let app = recording_router(dir.path());
    for name in [
        "..%2Fsecret.wav",
        "..",
        "nothing.wav",
        "notes.txt",
        "%2Fetc%2Fpasswd",
    ] {
        let (status, _) = request(
            app.clone(),
            "GET",
            &format!("/api/audiorecordings/{name}/download"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name} was served");
    }
}

#[tokio::test]
async fn recording_a_channel_that_makes_no_audio_is_refused() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"params":{"type":"adsb","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

    let (status, body) = record_channel(&app, ds, ch, "start").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("no audio"));

    let (status, _) = record_channel(&app, ds, 9_999, "start").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn network_export_start_stream_and_stop_roundtrip_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let address = receiver.local_addr().expect("address");
    let start = serde_json::json!({
        "action": "start",
        "node": "network:test",
        "stream": 0,
        "settings": {
            "transport": "udp",
            "format": "cu8",
            "address": address.to_string(),
        },
    });
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/network-export"),
        Some(&start.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let live: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
    assert_eq!(live.node, "network:test");
    assert_eq!(live.settings.address, address.to_string());

    let mut datagram = [0u8; 1_500];
    let received = receiver.recv(&mut datagram).expect("IQ datagram");
    assert!(received > 0);
    assert_eq!(received % 2, 0);

    let stop = serde_json::json!({ "action": "stop", "node": "network:test" });
    let (status, body) = request(
        app,
        "POST",
        &format!("/api/devicesets/{ds}/network-export"),
        Some(&stop.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let done: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
    assert!(done.samples > 0);
    assert_eq!(done.bytes, done.samples * 2);
    assert!(done.packets > 0);
    assert_eq!(done.error, None);
}

#[tokio::test]
async fn channel_baseband_record_roundtrip_lands_in_the_recording_library() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    let ch = create_nfm_channel(&app, ds).await;

    let (status, body) = record_baseband(&app, ds, ch, "start").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let live: RecordingStatus = serde_json::from_slice(&body).expect("json");
    assert!(live.file.starts_with(&format!("bb_{ds}_{ch}_")));
    live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

    let (status, body) = record_baseband(&app, ds, ch, "start").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("already recording"));

    wait_for_baseband_samples(&app, ds, ch, 4_800).await;
    let (status, body) = record_baseband(&app, ds, ch, "stop").await;
    assert_eq!(status, StatusCode::OK);
    let done: RecordingStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(done.file, live.file);
    assert!(done.samples >= 4_800);
    assert_eq!(done.error, None);

    let listed = list_recordings(&app).await;
    let entry = listed
        .iter()
        .find(|entry| entry.file == done.file)
        .expect("the finished baseband pair is in the library");
    assert_eq!(entry.sample_rate, 48_000.0);
    assert_eq!(entry.center_hz, 100_000_000.0);
    assert_eq!(entry.samples, done.samples);

    let (status, _) = record_baseband(&app, ds, ch, "stop").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = record_baseband(&app, ds, 9_999, "start").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn channel_network_export_streams_that_channels_baseband_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let ch = create_nfm_channel(&app, ds).await;
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let address = receiver.local_addr().expect("address").to_string();
    let start = serde_json::json!({
        "action": "start",
        "node": "baseband:test",
        "settings": { "transport": "udp", "format": "cf32_le", "address": address },
    });
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/network-export"),
        Some(&start.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let live: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
    assert_eq!(live.sample_rate, 48_000);
    assert_eq!(live.node, "baseband:test");

    let mut datagram = [0u8; 1_500];
    let received = receiver.recv(&mut datagram).expect("baseband datagram");
    assert!(received > 0 && received.is_multiple_of(8));

    let state = get_state(&app).await;
    let channel = &state.device_sets[0].channels[0];
    assert_eq!(
        channel
            .network_export
            .as_ref()
            .map(|export| export.node.as_str()),
        Some("baseband:test")
    );

    let stop = serde_json::json!({ "action": "stop", "node": "baseband:test" });
    let (status, body) = request(
        app,
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/network-export"),
        Some(&stop.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let done: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
    assert!(done.samples > 0);
    assert_eq!(done.error, None);
}

#[tokio::test]
async fn the_time_machine_arms_captures_and_files_its_window_over_http() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    let node = "time_machine:test";

    let (status, body) = time_machine(
        &app,
        ds,
        serde_json::json!({ "action": "arm", "node": node, "settings": { "history_seconds": 1 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let armed: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
    assert_eq!(armed.history_seconds, 1);
    assert_eq!(armed.capacity_samples, 2_048_000);
    assert!(armed.capture.is_none());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let held = get_state(&app)
            .await
            .device_sets
            .iter()
            .find(|set| set.id == ds)
            .expect("set listed")
            .time_machine
            .clone()
            .expect("armed")
            .held_samples;
        if held >= 1_024_000 {
            break;
        }
        assert!(Instant::now() < deadline, "the window never filled");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let (status, body) = time_machine(
        &app,
        ds,
        serde_json::json!({ "action": "capture", "node": node }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let capturing: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
    let file = capturing.capture.expect("a capture is running").file;

    let (status, body) = time_machine(
        &app,
        ds,
        serde_json::json!({ "action": "stop", "node": node }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stopped: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
    let written = stopped.capture.expect("the stop reports what it wrote");
    assert!(
        written.samples >= 1_024_000,
        "the past never reached the file"
    );

    let listed = list_recordings(&app).await;
    let entry = listed
        .iter()
        .find(|entry| entry.file == file)
        .expect("the captured window is in the library");
    assert_eq!(entry.sample_rate, 2_048_000.0);

    let (status, _) = time_machine(
        &app,
        ds,
        serde_json::json!({ "action": "disarm", "node": node }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.device_sets[0].time_machine.is_none());

    let (status, body) = time_machine(
        &app,
        ds,
        serde_json::json!({ "action": "capture", "node": node }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("no time machine"));
}
