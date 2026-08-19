use super::*;

#[tokio::test]
async fn templates_report_the_radios_that_can_run_them() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");

    assert!(!listed.templates.is_empty());
    for template in &listed.templates {
        assert!(
            template
                .supported_devices
                .contains(&"virtual:siggen".to_string()),
            "{} does not offer the signal generator: {:?}",
            template.id,
            template.supported_devices
        );
    }
}

#[tokio::test]
async fn a_template_the_radio_cannot_run_is_refused_before_anything_is_torn_down() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    record(&app, ds, "start").await;
    wait_for_recorded_samples(&app, ds, 1).await;
    record(&app, ds, "stop").await;

    let rec = list_recordings(&app).await.remove(0);
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(&format!(r#"{{"device_id":"{}"}}"#, rec.device_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let playback = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/templates/adsb/apply",
        Some(&format!(r#"{{"device_set":{playback}}}"#)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let set = get_state(&app)
        .await
        .device_sets
        .into_iter()
        .find(|set| set.id == playback)
        .expect("the playback set survived the refusal");
    assert!(set.channels.is_empty());
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
}

#[tokio::test]
async fn templates_list_and_apply_over_http() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");
    assert!(listed.templates.iter().any(|t| t.id == "fm-radio"));

    let ds = create_virtual_set(&app).await;
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/templates/fm-radio/apply",
        Some(&format!(r#"{{"device_set":{ds}}}"#)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.settings.center_hz, Some(98_000_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].settings.params.type_id(), "wfm");

    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/templates/nope/apply",
        Some(&format!(r#"{{"device_set":{ds}}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(
        app,
        "POST",
        "/api/templates/fm-radio/apply",
        Some(r#"{"device_set":999}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn every_template_runs_on_the_signal_generator() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let (_, body) = request(app.clone(), "GET", "/api/templates", None).await;
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");

    for template in &listed.templates {
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/templates/{}/apply", template.id),
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}: {}",
            template.id,
            String::from_utf8_lossy(&body)
        );
        let set = &get_state(&app).await.device_sets[0];
        assert_eq!(set.settings.center_hz, Some(template.center_hz));
        assert_eq!(
            set.channels.len(),
            template.channels.len(),
            "{}",
            template.id
        );
    }
}

#[tokio::test]
async fn applying_a_template_merges_its_patch_into_the_active_workspace() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let before = workspaces(&app).await;
    let active = before.active.expect("seeded workspace");
    let nodes_before = before.workspaces[0].nodes;

    for _ in 0..2 {
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/templates/fm-radio/apply",
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }

    let (status, body) = request(
        app.clone(),
        "GET",
        &format!("/api/workspaces/{active}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: sdrmm_wire::WorkspaceDetail = serde_json::from_slice(&body).expect("json");
    let (_, body) = request(app.clone(), "GET", "/api/templates", None).await;
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");
    let template = listed
        .templates
        .iter()
        .find(|t| t.id == "fm-radio")
        .expect("template");
    let patch = template.patch.as_ref().expect("templates carry a patch");

    let added = u32::try_from(patch.nodes.len()).unwrap();
    assert_eq!(
        u32::try_from(detail.snapshot.graph.nodes.len()).unwrap(),
        nodes_before + added
    );
    let device = detail
        .snapshot
        .graph
        .node("template:fm-radio:dev")
        .expect("the template's receiver");
    let sdrmm_wire::NodeBody::Device(bound) = &device.body else {
        panic!("a receiver node")
    };
    assert_eq!(
        bound.device.as_ref().map(|d| d.backend.as_str()),
        Some("virtual"),
        "the patch names the radio the template was applied to"
    );
    assert_eq!(
        detail
            .snapshot
            .graph
            .channels_of("template:fm-radio:dev")
            .count(),
        1
    );
    detail.snapshot.validate().expect("a valid workspace");
}
