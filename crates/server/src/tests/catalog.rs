use super::*;

#[tokio::test]
async fn occupancy_is_served_and_filtered_by_how_well_observed_it_is() {
    let (app, state) = test_router_with_state();

    let (status, body) = request(app.clone(), "GET", "/api/occupancy", None).await;
    assert_eq!(status, StatusCode::OK);
    let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
    assert!(report.buckets.is_empty());
    assert_eq!(report.bucket_hz, sdrmm_engine::occupancy::BUCKET_HZ);

    {
        let mut occupancy = state
            .engine
            .occupancy()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut db = vec![-100.0f32; 128];
        for round in 0..40 {
            db[64] = -40.0;
            db[96] = if round < 4 { -40.0 } else { -100.0 };
            occupancy.observe(&db, 100e6, 1.6e6, None, 0);
        }
    }

    let (status, body) = request(app.clone(), "GET", "/api/occupancy", None).await;
    assert_eq!(status, StatusCode::OK);
    let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
    assert!(
        report.buckets.len() >= 2,
        "nothing survived the sample floor"
    );
    assert!(
        report.buckets[0].duty >= report.buckets[1].duty,
        "the report is not ordered busiest first"
    );
    assert_eq!(report.buckets[0].by_hour.len(), 24);
    assert!(!report.since.is_empty());

    let (status, body) = request(app.clone(), "GET", "/api/occupancy?min_samples=1000", None).await;
    assert_eq!(status, StatusCode::OK);
    let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
    assert!(report.buckets.is_empty());
}

#[tokio::test]
async fn the_band_plan_is_served_per_region() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/bandplan/regions", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: sdrmm_wire::BandRegionsResponse = serde_json::from_slice(&body).expect("json");
    assert!(listed.regions.iter().any(|region| region.id == "de"));
    assert!(
        listed
            .regions
            .iter()
            .any(|region| region.id == listed.default_region)
    );

    let (status, body) = request(app.clone(), "GET", "/api/bandplan/regions/de", None).await;
    assert_eq!(status, StatusCode::OK);
    let plan: sdrmm_wire::BandPlan = serde_json::from_slice(&body).expect("json");
    assert_eq!(plan.region.id, "de");
    assert!(
        plan.layers
            .iter()
            .any(|layer| layer.authority == "Bundesnetzagentur")
    );
    let allocation = &plan.lanes[0];
    assert!(!allocation.overlay);
    let block = allocation
        .blocks
        .iter()
        .find(|block| block.start_hz <= 121_500_000.0 && block.stop_hz > 121_500_000.0)
        .expect("118–137 MHz is allocated");
    let winner = &plan.allocations[block.of as usize];
    assert_eq!(winner.service, sdrmm_wire::BandService::Aeronautical);
    assert_eq!(
        winner.suggested.as_ref().map(ChannelParams::type_id),
        Some("am"),
        "the airband suggests AM, which is what one-click tuning applies"
    );

    let (status, _) = request(app, "GET", "/api/bandplan/regions/atlantis", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn locating_a_region_validates_its_coordinate() {
    let app = test_router();
    let (status, body) = request(
        app.clone(),
        "GET",
        "/api/bandplan/locate?lat=52.52&lon=13.40",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let found: sdrmm_wire::BandRegionMatch = serde_json::from_slice(&body).expect("json");
    assert_eq!(found.region, "de");
    assert!(!found.approximate);

    let (status, _) = request(
        app.clone(),
        "GET",
        "/api/bandplan/locate?lat=91&lon=0",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(app, "GET", "/api/bandplan/locate?lat=52.52", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_patch_catalog_describes_the_node_palette() {
    let app = test_router();
    let (status, body) = request(app, "GET", "/api/patch/catalog", None).await;
    assert_eq!(status, StatusCode::OK);
    let catalog: sdrmm_wire::PatchCatalog = serde_json::from_slice(&body).expect("json");
    assert_eq!(catalog, sdrmm_wire::PatchCatalog::build());
    let device = catalog
        .nodes
        .iter()
        .find(|n| n.kind == "device")
        .expect("a device in the palette");
    assert_eq!(device.category, sdrmm_wire::NodeCategory::Source);
    let port = |name: &str| {
        device
            .ports
            .iter()
            .find(|port| port.name == name)
            .unwrap_or_else(|| panic!("the device node has a {name} port"))
    };
    assert!(port("iq").multi, "one radio feeds many nodes");
    assert!(!port("control").multi, "one sweep owns a radio");
    assert!(port("tx").note.is_some(), "the reserved port says why");
}

#[tokio::test]
async fn tools_lists_what_this_build_offers() {
    let (status, body) = request(test_router(), "GET", "/api/tools", None).await;
    assert_eq!(status, StatusCode::OK);
    let tools: sdrmm_wire::ToolsResponse = serde_json::from_slice(&body).expect("json");
    let antenna = tools
        .tools
        .iter()
        .find(|tool| tool.id == sdrmm_wire::ANTENNA_TOOL_ID)
        .expect("the antenna calculator is a builtin");
    assert!(!antenna.needs_hardware);
    assert!(!antenna.summary.is_empty());
    let nanovna = tools
        .tools
        .iter()
        .find(|tool| tool.id == sdrmm_wire::NANOVNA_TOOL_ID)
        .expect("the NanoVNA instrument is a builtin");
    assert!(nanovna.needs_hardware);
    assert_eq!(nanovna.category, sdrmm_wire::ToolCategory::Instrument);
}

#[tokio::test]
async fn nanovna_device_discovery_uses_the_tool_handler() {
    let (status, body) = request(
        test_router(),
        "POST",
        "/api/tools/run",
        Some(r#"{"tool":"nanovna","request":{"action":"list_devices"}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let response: sdrmm_wire::ToolResponse = serde_json::from_slice(&body).expect("json");
    let sdrmm_wire::ToolResponse::NanoVna(result) = response else {
        panic!("a NanoVNA call must answer under the NanoVNA tag");
    };
    let sdrmm_wire::NanoVnaResult::Devices {
        devices,
        ignored_ports,
    } = *result
    else {
        panic!("NanoVNA discovery must return the device result");
    };
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].port, "fixture-port");
    assert_eq!(devices[0].match_kind, sdrmm_wire::NanoVnaMatch::Confirmed);
    assert_eq!(ignored_ports, vec!["fixture-gnss".to_owned()]);
}

#[tokio::test]
async fn a_tool_call_answers_under_the_tag_it_was_asked_with() {
    let (status, body) = request(
        test_router(),
        "POST",
        "/api/tools/run",
        Some(
            r#"{"tool":"antenna","request":{"frequency_hz":145500000.0,
                    "design":{"type":"yagi","settings":{"directors":3,
                    "spacing_wavelengths":0.2}}}}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response: sdrmm_wire::ToolResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(response.tool_id(), sdrmm_wire::ANTENNA_TOOL_ID);
    let sdrmm_wire::ToolResponse::Antenna(report) = response else {
        panic!("an antenna request is answered by the antenna tool");
    };
    assert_eq!(report.frequency_hz, 145_500_000.0);
    assert!(
        report
            .parts
            .iter()
            .any(|part| part.name == "Director 3" && part.position_m.is_some())
    );
}

#[tokio::test]
async fn a_tool_refusal_is_a_typed_bad_request() {
    let (status, body) = request(
        test_router(),
        "POST",
        "/api/tools/run",
        Some(r#"{"tool":"antenna","request":{"frequency_hz":0.0,"design":{"type":"dipole"}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert!(error.error.contains("frequency_hz"), "{}", error.error);
}

#[tokio::test]
async fn an_unknown_tool_tag_is_refused_in_the_error_shape() {
    let (status, body) = request(
        test_router(),
        "POST",
        "/api/tools/run",
        Some(r#"{"tool":"nanovna","request":{}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert_eq!(error.error, "invalid request body");
}

#[tokio::test]
async fn about_serves_the_notices_and_their_texts() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/about", None).await;
    assert_eq!(status, StatusCode::OK);
    let about: sdrmm_wire::AboutResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(about.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(about.license, "GPL-3.0-or-later");

    let component = about
        .components
        .iter()
        .find(|component| !component.texts.is_empty())
        .expect("some component ships a license text");
    let id = &component.texts[0];
    let (status, body) = request(
        app.clone(),
        "GET",
        &format!("/api/about/licenses/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text: sdrmm_wire::LicenseTextResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(&text.id, id);
    assert!(!text.text.is_empty());

    let (status, _) = request(app, "GET", "/api/about/licenses/nosuchtext", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn doctor_reports_the_running_configuration() {
    let (status, body) = request(test_router(), "GET", "/api/doctor", None).await;
    assert_eq!(status, StatusCode::OK);
    let report: sdrmm_wire::DoctorReport = serde_json::from_slice(&body).expect("json");
    assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
    let backends = report
        .checks
        .iter()
        .find(|c| c.id == "backends")
        .expect("backends check");
    assert_eq!(backends.status, sdrmm_wire::CheckStatus::Warn);
    assert!(backends.detail.contains("virtual"));
    assert!(report.checks.iter().any(|c| c.id == "storage.db"));
}
