use sdrmm_wire::{DfBearing, DfFusionState, PositionFix};

use super::*;

fn fix(at: (f64, f64)) -> PositionFix {
    PositionFix {
        latitude: at.0,
        longitude: at.1,
        altitude_m: None,
        accuracy_m: None,
        speed_mps: None,
        track_deg: None,
        time: "2026-01-01T00:00:00Z".to_owned(),
        attitude: sdrmm_wire::Attitude::default(),
    }
}

fn bearing(bearing_deg: f32) -> DfBearing {
    DfBearing {
        bearing_deg,
        confidence: 0.95,
        lat: None,
        lon: None,
        station_id: None,
        node: String::new(),
        sigma_deg: 10.0,
        accuracy_m: None,
        heading_deg: None,
        heading_sigma_deg: None,
        relative_deg: None,
        mirror_deg: None,
        freq_hz: None,
        source: sdrmm_wire::fusion::BearingSource::Array,
        moving: false,
        others: Vec::new(),
        likelihood: Vec::new(),
    }
}

#[tokio::test]
async fn the_fusion_route_serves_and_clears_a_triangulation_grid() {
    let (app, state) = test_router_with_state();
    let (status, _) = request(app.clone(), "GET", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let target = crate::df_fusion::destination(51.5, 7.0, 45.0, 6_000.0);
    let east = crate::df_fusion::destination(51.5, 7.0, 135.0, 6_000.0);
    for (station, from) in [("north", (51.5, 7.0)), ("east", east)] {
        let seen = crate::df_fusion::bearing_between(from, target) as f32;
        for _ in 0..4 {
            state.fusion.observe(
                "cross",
                station,
                &bearing(seen),
                Some(&fix(from)),
                "2026-01-01T00:00:00Z",
            );
        }
    }
    let (status, body) = request(app.clone(), "GET", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::OK);
    let fused: DfFusionState = serde_json::from_slice(&body).expect("json");
    let estimate = fused.estimate.expect("two stations give an estimate");
    let error = crate::df_fusion::distance_m((estimate.lat, estimate.lon), target);
    assert!(error < 800.0, "{error} m away: {estimate:?}");
    assert_eq!(fused.stations.len(), 2);

    let (status, _) = request(app.clone(), "DELETE", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = request(app, "GET", "/api/fusion/cross", None).await;
    let cleared: DfFusionState = serde_json::from_slice(&body).expect("json");
    assert_eq!(cleared.samples, 0);
}

#[tokio::test]
async fn the_old_coherent_routes_are_gone() {
    let (app, _) = test_router_with_state();
    for (method, uri) in [
        ("POST", "/api/coherent/df/calibrate"),
        ("GET", "/api/coherent/cross/fusion"),
    ] {
        let (status, _) = request(app.clone(), method, uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}
