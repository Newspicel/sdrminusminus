use super::*;

#[tokio::test]
async fn call_endpoints_list_completed_calls_and_reject_missing_audio() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/calls", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: VoiceCallsResponse = serde_json::from_slice(&body).expect("json");
    assert!(listed.calls.is_empty());

    let (status, body) = request(app, "GET", "/api/calls/99/audio", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
}

#[tokio::test]
async fn a_plain_dmr_channel_records_every_call_without_any_trunk_system() {
    const RATE: f64 = 240_000.0;
    let dir = tempfile::TempDir::new().expect("temp dir");
    let stem = dir.path().join("dmr");
    let spoken = sdrmm_channels::testgen::dv::dmr::Call::default();
    let one = sdrmm_channels::testgen::dv::dmr::transmission(&spoken, RATE);
    let mut iq = Vec::new();
    for _ in 0..6 {
        iq.extend_from_slice(&one);
    }
    let floor = RATE as usize * 2;
    if iq.len() < floor {
        iq.extend(sdrmm_channels::testgen::silence(floor - iq.len()));
    }
    let mut writer =
        sdrmm_recorder::SigmfWriter::create(&stem, RATE, 145_000_000.0, "conventional dmr fixture")
            .expect("create fixture");
    writer.write_block(&iq).expect("write fixture");
    writer.finalize().expect("finalize fixture");

    let app = recording_router(dir.path());
    let mut snapshot =
        virtual_snapshot(&format!("file:{}", stem.display()), &[("dmr", "dmr", "iq")]);
    for node in &mut snapshot.graph.nodes {
        if let sdrmm_wire::NodeBody::Channel(channel) = &mut node.body {
            channel.record_calls = true;
        }
    }
    let workspace = put_active_workspace(&app, &snapshot).await;
    assert_eq!(apply(&app, workspace).await.created, 1);

    let recorded = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let (status, body) = request(app.clone(), "GET", "/api/calls", None).await;
            assert_eq!(status, StatusCode::OK);
            let listed: VoiceCallsResponse = serde_json::from_slice(&body).expect("json");
            if let Some(call) = listed.calls.into_iter().next() {
                return call;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a conventional DMR call reaches the call store");

    assert_eq!(recorded.node, "dmr", "the channel node owns the call");
    assert_eq!(recorded.source, Some(spoken.source));
    assert_eq!(recorded.destination, Some(spoken.destination));
    assert_eq!(recorded.group_call, Some(true));
    assert!(!recorded.encrypted);
    let audio = recorded.audio.expect("the call kept its audio");
    let (status, wav) = request(app, "GET", &audio.url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&wav[0..4], b"RIFF");
}

#[tokio::test]
async fn image_endpoints_list_captures_and_reject_a_missing_picture() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/images", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: CapturedImagesResponse = serde_json::from_slice(&body).expect("json");
    assert!(listed.images.is_empty());

    let (status, body) = request(app, "GET", "/api/images/99/png", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
}
