use super::*;

#[tokio::test]
async fn templates_report_the_radios_that_can_run_them() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");

    assert!(!listed.templates.is_empty());
    for template in &listed.templates {
        let offers = |id: &str| template.supported_devices.iter().any(|device| device == id);
        if template.min_lanes > 1 {
            assert!(offers("virtual:kraken5"), "{}", template.id);
            assert!(!offers("virtual:band"), "{}", template.id);
        } else {
            assert!(
                offers("virtual:band"),
                "{} does not offer the test band: {:?}",
                template.id,
                template.supported_devices
            );
        }
    }
}

#[tokio::test]
async fn a_template_the_radio_cannot_run_is_refused_before_anything_is_torn_down() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let app = recording_router(dir.path());
    let ds = create_virtual_set(&app).await;
    record(&app, ds, true).await.expect("recording started");
    wait_for_recorded_samples(&app, ds, 1).await;
    record(&app, ds, false).await;

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
    assert!(
        set.channels.iter().all(|channel| !channel.out_of_band),
        "the radio did not settle over the template's channels"
    );
    assert_eq!(
        set.settings.sample_rate,
        Some(2_400_000.0),
        "a radio that offers the template's own rate runs it"
    );
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

    for template in listed.templates.iter().filter(|t| t.min_lanes <= 1) {
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
        assert!(
            set.channels.iter().all(|channel| !channel.out_of_band),
            "{}: the radio did not settle over the template's channels",
            template.id
        );
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

#[tokio::test]
async fn the_df_drive_wires_every_kraken_lane_into_one_array() {
    let app = test_router();
    let band = create_virtual_set(&app).await;
    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/templates/df-drive/apply",
        Some(&format!(r#"{{"device_set":{band}}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "one lane is refused");

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(r#"{"device_id":"virtual:kraken5"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let kraken = serde_json::from_slice::<CreatedId>(&body).expect("json").id;
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/templates/df-drive/apply",
        Some(&format!(r#"{{"device_set":{kraken}}}"#)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let active = workspaces(&app).await.active.expect("seeded workspace");
    let (_, body) = request(
        app.clone(),
        "GET",
        &format!("/api/workspaces/{active}"),
        None,
    )
    .await;
    let detail: sdrmm_wire::WorkspaceDetail = serde_json::from_slice(&body).expect("json");
    let graph = &detail.snapshot.graph;
    let lanes = graph
        .edges
        .iter()
        .filter(|edge| {
            edge.from.node == "template:df-drive:dev" && edge.to.node == "template:df-drive:array"
        })
        .count();
    assert_eq!(lanes, 5);
    assert_eq!(
        graph.array_of_processor("template:df-drive:df"),
        Some("template:df-drive:array")
    );
    detail.snapshot.validate().expect("a valid workspace");
    let set = get_state(&app)
        .await
        .device_sets
        .into_iter()
        .find(|set| set.id == kraken)
        .expect("the kraken");
    assert_eq!(set.settings.center_hz, Some(433_920_000.0));
}

struct HidesOpenRadios {
    inner: sdrmm_device_virtual::VirtualDriver,
    opened: std::sync::atomic::AtomicBool,
}

impl sdrmm_device::DeviceDriver for HidesOpenRadios {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn probe(&self) -> Vec<sdrmm_wire::DeviceInfo> {
        if self.opened.load(std::sync::atomic::Ordering::Acquire) {
            Vec::new()
        } else {
            self.inner.probe()
        }
    }

    fn open(
        &self,
        info: &sdrmm_wire::DeviceInfo,
    ) -> Result<Box<dyn sdrmm_device::SdrDevice>, sdrmm_device::DeviceError> {
        let device = self.inner.open(info)?;
        self.opened
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(device)
    }
}

#[tokio::test]
async fn an_open_radio_the_driver_no_longer_lists_can_still_run_templates() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(
        1,
        Box::new(HidesOpenRadios {
            inner: sdrmm_device_virtual::VirtualDriver::new(),
            opened: std::sync::atomic::AtomicBool::new(false),
        }),
    );
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let state = AppState::new(Engine::with_registry(registry, None), store);
    let (app, background) = router_with_state(state, &ServerOptions::default());
    background.detach();
    create_virtual_set(&app).await;

    let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");
    let fm = listed
        .templates
        .iter()
        .find(|t| t.id == "fm-radio")
        .expect("fm-radio");
    assert_eq!(fm.supported_devices, vec!["virtual:band".to_string()]);
}
