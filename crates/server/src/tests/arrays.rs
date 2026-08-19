use sdrmm_wire::{ArrayDefinition, ArraysResponse, Coherence, DevicesResponse};

use super::*;

const ARRAY: &str = r#"{
    "key": "bench",
    "label": "Bench pair",
    "members": ["virtual:siggen", "virtual:halfduplex"],
    "coherence": "time_sync",
    "shared_tuning": true
}"#;

async fn arrays(app: &Router) -> Vec<ArrayDefinition> {
    let (status, body) = request(app.clone(), "GET", "/api/arrays", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<ArraysResponse>(&body)
        .expect("json")
        .arrays
}

#[tokio::test]
async fn an_array_is_described_once_and_read_back() {
    let (app, _state) = test_router_with_state();
    assert!(arrays(&app).await.is_empty());

    let (status, body) = request(app.clone(), "PUT", "/api/arrays/bench", Some(ARRAY)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let stored = arrays(&app).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].key, "bench");
    assert_eq!(stored[0].coherence, Coherence::TimeSync);
    assert_eq!(stored[0].members.len(), 2);
}

#[tokio::test]
async fn the_key_in_the_address_is_the_one_that_counts() {
    let (app, state) = test_router_with_state();
    let (status, _) = request(app.clone(), "PUT", "/api/arrays/roof", Some(ARRAY)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(arrays(&app).await[0].key, "roof");
    assert_eq!(state.engine.arrays().all()[0].key, "roof");
}

#[tokio::test]
async fn an_array_that_is_not_one_is_refused() {
    let (app, _state) = test_router_with_state();
    for bad in [
        r#"{"key":"x","label":"","members":["virtual:siggen"],"coherence":"time_sync"}"#,
        r#"{"key":"x","label":"","members":["virtual:siggen","virtual:siggen"],"coherence":"time_sync"}"#,
        r#"{"key":"x","label":"","members":["virtual:siggen","virtual:halfduplex"],"coherence":"none"}"#,
    ] {
        let (status, body) = request(app.clone(), "PUT", "/api/arrays/x", Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        let error: ApiError = serde_json::from_slice(&body).expect("json");
        assert!(error.error.contains("array"), "{error:?}");
    }
    assert!(arrays(&app).await.is_empty());
}

#[tokio::test]
async fn a_described_array_shows_up_as_one_radio_with_every_members_lanes() {
    let (app, _state) = test_router_with_state();
    let (status, _) = request(app.clone(), "PUT", "/api/arrays/bench", Some(ARRAY)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(app.clone(), "GET", "/api/devices", None).await;
    assert_eq!(status, StatusCode::OK);
    let devices: DevicesResponse = serde_json::from_slice(&body).expect("json");
    let composite = devices
        .devices
        .iter()
        .find(|device| device.id() == "array:bench")
        .expect("the composite is offered like any other radio");
    assert_eq!(composite.label, "Bench pair");
    assert_eq!(
        composite.profile.as_ref().expect("a profile").rx_streams,
        2,
        "one lane per member"
    );
}

#[tokio::test]
async fn removing_an_array_takes_the_radio_away_with_it() {
    let (app, state) = test_router_with_state();
    let (status, _) = request(app.clone(), "PUT", "/api/arrays/bench", Some(ARRAY)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(app.clone(), "DELETE", "/api/arrays/bench", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(arrays(&app).await.is_empty());
    assert!(state.engine.arrays().all().is_empty());

    let (status, body) = request(app.clone(), "DELETE", "/api/arrays/bench", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert!(error.error.contains("bench"), "{error:?}");
}
