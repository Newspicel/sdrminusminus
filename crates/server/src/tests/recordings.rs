use super::*;

#[tokio::test]
async fn playback_transport_pauses_seeks_and_stops() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;
    let ds = playback_set(&app, &rec).await;

    let reported = |app: &Router| {
        let app = app.clone();
        async move {
            get_state(&app)
                .await
                .device_sets
                .into_iter()
                .find(|set| set.id == ds)
                .expect("the playback set is listed")
                .playback
                .expect("a replaying set reports a transport")
        }
    };

    let initial = reported(&app).await;
    assert!(!initial.paused);
    assert_eq!(initial.total_samples, rec.samples);

    let (status, body) = playback(&app, ds, r#"{"action":"pause"}"#).await;
    assert_eq!(status, StatusCode::OK);
    let paused: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
    assert!(paused.paused);
    assert_eq!(reported(&app).await, paused);

    let (status, body) = playback(
        &app,
        ds,
        &format!(
            r#"{{"action":"seek","position_samples":{}}}"#,
            rec.samples / 2
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sought: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(sought.position_samples, rec.samples / 2);
    assert_eq!(reported(&app).await, sought);

    let (status, body) = playback(&app, ds, r#"{"action":"stop"}"#).await;
    assert_eq!(status, StatusCode::OK);
    let stopped: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
    assert!(stopped.paused);
    assert_eq!(stopped.position_samples, 0);

    let (status, _) = playback(&app, ds, r#"{"action":"play"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!reported(&app).await.paused);
}

#[tokio::test]
async fn a_radio_has_no_transport_to_drive() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let set = get_state(&app)
        .await
        .device_sets
        .into_iter()
        .find(|set| set.id == ds)
        .expect("set listed");
    assert_eq!(set.playback, None);

    let (status, body) = playback(&app, ds, r#"{"action":"pause"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        String::from_utf8_lossy(&body).contains("not a recording"),
        "{}",
        String::from_utf8_lossy(&body)
    );

    let (status, _) = playback(&app, 9_999, r#"{"action":"pause"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_serves_the_pair_as_a_sigmf_archive() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;

    let (status, headers, body) = request_parts(
        app,
        "GET",
        &format!("/api/recordings/{}/download", rec.id),
        None,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_value(&headers, "content-type"), "application/x-tar");
    assert_eq!(
        header_value(&headers, "content-disposition"),
        format!("attachment; filename=\"{}.sigmf\"", rec.file)
    );
    assert_eq!(
        header_value(&headers, "content-length"),
        body.len().to_string()
    );
    assert!(
        body.starts_with(format!("{}/", rec.file).as_bytes()),
        "first tar header names the recording's directory"
    );
    assert_eq!(&body[257..263], b"ustar\0");
}

#[tokio::test]
async fn download_serves_iq_as_a_float_wav() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;

    let (status, headers, body) = request_parts(
        app,
        "GET",
        &format!("/api/recordings/{}/download?format=wav", rec.id),
        None,
        &[],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_value(&headers, "content-type"), "audio/wav");
    assert_eq!(
        header_value(&headers, "content-disposition"),
        format!("attachment; filename=\"{}.wav\"", rec.file)
    );
    assert_eq!(
        header_value(&headers, "content-length"),
        body.len().to_string()
    );
    assert_eq!(&body[..4], b"RIFF");
    assert_eq!(&body[8..12], b"WAVE");
    assert_eq!(&body[12..16], b"fmt ");
    assert_eq!(u16::from_le_bytes([body[20], body[21]]), 3);
    assert_eq!(u16::from_le_bytes([body[22], body[23]]), 2);
    assert_eq!(
        u32::from_le_bytes([body[24], body[25], body[26], body[27]]),
        2_048_000
    );
    assert_eq!(
        body.len() as u64,
        230 + rec.samples * sdrmm_recorder::BYTES_PER_SAMPLE,
        "header plus every recorded sample"
    );
}

#[tokio::test]
async fn downloads_are_never_compressed() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;

    for format in ["sigmf", "wav"] {
        let (status, headers, body) = request_parts(
            app.clone(),
            "GET",
            &format!("/api/recordings/{}/download?format={format}", rec.id),
            None,
            &[("accept-encoding", "gzip, deflate, br")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header_value(&headers, "content-encoding"), "", "{format}");
        assert_eq!(
            header_value(&headers, "content-length"),
            body.len().to_string(),
            "{format}"
        );
    }

    let (_, headers, _) = request_parts(
        app,
        "GET",
        "/api/state",
        None,
        &[("accept-encoding", "gzip")],
    )
    .await;
    assert_eq!(header_value(&headers, "content-encoding"), "gzip");
}

#[tokio::test]
async fn downloading_an_unknown_recording_or_format_fails_cleanly() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;

    let (status, _) = request(app.clone(), "GET", "/api/recordings/9999/download", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        app.clone(),
        "GET",
        &format!("/api/recordings/{}/download?format=flac", rec.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    std::fs::remove_file(sdrmm_recorder::data_path(&dir.path().join(&rec.file)))
        .expect("remove data");
    let (status, _) = request(
        app,
        "GET",
        &format!("/api/recordings/{}/download", rec.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn record_start_stop_index_and_delete_roundtrip_over_http() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;

    let (status, body) = record(&app, ds, "start").await;
    assert_eq!(status, StatusCode::OK);
    let live: RecordingStatus = serde_json::from_slice(&body).expect("json");
    assert!(!live.file.is_empty());
    assert_eq!(live.error, None);
    live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

    wait_for_recorded_samples(&app, ds, 1).await;

    let (status, body) = record(&app, ds, "stop").await;
    assert_eq!(status, StatusCode::OK);
    let done: RecordingStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(done.file, live.file);
    assert!(done.samples > 0);
    assert_eq!(done.bytes, done.samples * sdrmm_recorder::BYTES_PER_SAMPLE);
    assert_eq!(done.error, None);

    let listed = list_recordings(&app).await;
    assert_eq!(listed.len(), 1);
    let rec = &listed[0];
    assert_eq!(rec.file, done.file);
    assert_eq!(rec.samples, done.samples);
    assert_eq!(rec.sample_rate, 2_048_000.0);
    assert_eq!(rec.center_hz, 100_000_000.0);
    assert_eq!(rec.device_label, "Signal Generator (virtual)");
    assert!(rec.duration_s > 0.0);
    assert_eq!(
        rec.device_id,
        format!("virtual:file:{}", dir.path().join(&rec.file).display())
    );

    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(&format!(r#"{{"device_id":"{}"}}"#, rec.device_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/recordings/{}", rec.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let stem = dir.path().join(&rec.file);
    assert!(!sdrmm_recorder::meta_path(&stem).exists());
    assert!(!sdrmm_recorder::data_path(&stem).exists());
    assert!(list_recordings(&app).await.is_empty());
    let (status, _) = request(app, "DELETE", &format!("/api/recordings/{}", rec.id), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recordings_list_reconciles_planted_files_and_prunes_vanished_ones() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());

    let stem = dir.path().join("planted");
    let block: Vec<num_complex::Complex<f32>> = vec![num_complex::Complex::new(0.5, -0.5); 4_800];
    let mut writer =
        sdrmm_recorder::SigmfWriter::create(&stem, 48_000.0, 7_100_000.0, "Foreign HW")
            .expect("writer");
    writer.write_block(&block).expect("write");
    writer.finalize().expect("finalize");
    drop(
        sdrmm_recorder::SigmfWriter::create(&dir.path().join("crashed"), 48_000.0, 1e6, "hw")
            .expect("writer"),
    );

    let listed = list_recordings(&app).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].file, "planted");
    assert_eq!(listed[0].samples, 4_800);
    assert_eq!(listed[0].duration_s, 0.1);
    assert_eq!(listed[0].device_label, "Foreign HW");
    listed[0]
        .created_at
        .parse::<jiff::Timestamp>()
        .expect("rfc3339");

    std::fs::remove_file(sdrmm_recorder::meta_path(&stem)).expect("remove meta");
    std::fs::remove_file(sdrmm_recorder::data_path(&stem)).expect("remove data");
    assert!(list_recordings(&app).await.is_empty());
}

#[tokio::test]
async fn an_annotation_lands_in_the_sigmf_metadata_and_survives_a_reconcile() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;
    assert!(rec.tags.is_empty());
    assert_eq!(rec.note, None);

    let (status, body) = annotate(
        &app,
        rec.id,
        r#"{"tags":["  Airband ","airband","tower"],"note":"  EDDF ground  "}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let annotated: sdrmm_wire::RecordingInfo = serde_json::from_slice(&body).expect("json");
    assert_eq!(annotated.tags, ["Airband", "tower"]);
    assert_eq!(annotated.note.as_deref(), Some("EDDF ground"));
    assert_eq!(annotated.id, rec.id);
    assert_eq!(annotated.samples, rec.samples);

    let meta = sdrmm_recorder::SigmfReader::open(&dir.path().join(&rec.file))
        .expect("reopen")
        .meta()
        .clone();
    assert_eq!(meta.global.tags, ["Airband", "tower"]);
    assert_eq!(meta.global.description.as_deref(), Some("EDDF ground"));

    let listed = list_recordings(&app).await;
    assert_eq!(listed[0].tags, ["Airband", "tower"]);
    assert_eq!(listed[0].note.as_deref(), Some("EDDF ground"));

    let (status, _) = annotate(&app, rec.id, r#"{"tags":[],"note":null}"#).await;
    assert_eq!(status, StatusCode::OK);
    let listed = list_recordings(&app).await;
    assert!(listed[0].tags.is_empty());
    assert_eq!(listed[0].note, None);
}

#[tokio::test]
async fn annotation_error_mapping_over_http() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let rec = recorded(&app).await;

    let (status, body) = annotate(&app, 9_999, r#"{"tags":["x"]}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let many = (0..=sdrmm_wire::MAX_RECORDING_TAGS)
        .map(|i| format!("\"t{i}\""))
        .collect::<Vec<_>>()
        .join(",");
    let (status, body) = annotate(&app, rec.id, &format!(r#"{{"tags":[{many}]}}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let long = "n".repeat(sdrmm_wire::MAX_RECORDING_NOTE_LEN + 1);
    let (status, _) = annotate(&app, rec.id, &format!(r#"{{"note":"{long}"}}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = annotate(&app, rec.id, r#"{"tags":"airband"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    std::fs::remove_file(sdrmm_recorder::meta_path(&dir.path().join(&rec.file)))
        .expect("remove meta");
    let (status, _) = annotate(&app, rec.id, r#"{"tags":["x"]}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn record_error_mapping_over_http() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());

    let (status, _) = record(&app, 999, "start").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let ds = create_virtual_set(&app).await;
    let (status, body) = record(&app, ds, "stop").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = record(&app, ds, "start").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = record(&app, ds, "start").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"sample_rate":2400000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = record(&app, ds, "stop").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn recording_endpoints_without_a_recordings_dir() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let (status, body) = record(&app, ds, "start").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = record(&app, 999, "start").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert!(list_recordings(&app).await.is_empty());
    let (status, _) = request(app, "DELETE", "/api/recordings/1", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_recording_never_404s_against_concurrent_reconciles() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let block: Vec<num_complex::Complex<f32>> = vec![num_complex::Complex::new(0.5, -0.5); 64];
    for i in 0..10 {
        let file = format!("planted_{i}");
        let mut writer =
            sdrmm_recorder::SigmfWriter::create(&dir.path().join(&file), 48_000.0, 1e6, "hw")
                .expect("writer");
        writer.write_block(&block).expect("write");
        writer.finalize().expect("finalize");

        let listed = list_recordings(&app).await;
        let id = listed.iter().find(|r| r.file == file).expect("indexed").id;
        let delete = {
            let app = app.clone();
            tokio::spawn(async move {
                request(app, "DELETE", &format!("/api/recordings/{id}"), None).await
            })
        };
        let lists: Vec<_> = (0..3)
            .map(|_| {
                let app = app.clone();
                tokio::spawn(async move {
                    request(app, "GET", "/api/recordings", None).await;
                })
            })
            .collect();
        let (status, body) = delete.await.expect("join");
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "iteration {i}: {}",
            String::from_utf8_lossy(&body)
        );
        for list in lists {
            list.await.expect("join");
        }
        assert!(
            !list_recordings(&app).await.iter().any(|r| r.file == file),
            "iteration {i}: deleted recording resurfaced"
        );
    }
}
