use sdrmm_wire::DiagnosticsReport;

use super::*;

#[tokio::test]
async fn the_report_carries_the_environment_and_this_run_s_log() {
    let marker = "diagnostics-endpoint-marker-4d1f";
    let app = test_router();
    crate::diagnostics::log().push(sdrmm_wire::LogLine {
        at: jiff::Timestamp::now().to_string(),
        level: sdrmm_wire::LogLevel::Warn,
        target: "sdrmm_server::tests".to_string(),
        message: marker.to_string(),
    });

    let (status, body) = request(app, "GET", "/api/diagnostics", None).await;
    assert_eq!(status, StatusCode::OK);
    let report: DiagnosticsReport = serde_json::from_slice(&body).expect("json");

    assert!(!report.generated_at.is_empty());
    assert_eq!(report.doctor.version, env!("CARGO_PKG_VERSION"));
    assert!(!report.doctor.checks.is_empty());
    assert!(
        report.log.iter().any(|line| line.message == marker),
        "the log tail is missing the line that was recorded"
    );
}

#[tokio::test]
async fn a_rejected_request_names_which_part_refused() {
    let app = test_router();

    let (status, body) = request(app.clone(), "GET", "/api/presets/999999/apply", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(body.is_empty() || serde_json::from_slice::<ApiError>(&body).is_ok());

    let (status, body) = request(app.clone(), "DELETE", "/api/presets/999999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert_eq!(error.code, Some(sdrmm_wire::ErrorCode::NotFound));

    let (status, body) =
        request(app.clone(), "POST", "/api/devicesets", Some("{\"device\":")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert_eq!(error.code, Some(sdrmm_wire::ErrorCode::Request));
    assert!(error.detail.is_some());

    let (status, body) = request(app, "GET", "/api/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert_eq!(error.code, Some(sdrmm_wire::ErrorCode::NotFound));
}
