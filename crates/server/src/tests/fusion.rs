use sdrmm_wire::{
    DfBearing, DfFusionState, ServerEvent, TriangulationParams,
    geo::{self, LatLon},
};

use super::*;

const HOME: LatLon = LatLon {
    lat: 51.5,
    lon: 7.0,
};

fn bearing(station: &str, from: LatLon, bearing_deg: f64) -> DfBearing {
    DfBearing {
        bearing_deg: bearing_deg as f32,
        confidence: 0.95,
        lat: Some(from.lat),
        lon: Some(from.lon),
        station_id: Some(station.to_owned()),
        node: String::new(),
        sigma_deg: 3.0,
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
    let (status, _) = request(app.clone(), "DELETE", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    state
        .fusion
        .configure("cross", &TriangulationParams::default());
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let east = geo::destination(HOME, 135.0, 6_000.0);
    for tick in 0..4u32 {
        for (station, from) in [("north", HOME), ("east", east)] {
            let seen = geo::bearing_deg(from, target);
            state
                .fusion
                .observe(
                    "cross",
                    &bearing(station, from, seen),
                    f64::from(tick),
                    "2026-01-01T00:00:00Z",
                )
                .expect("accepted");
        }
    }
    let (status, body) = request(app.clone(), "GET", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::OK);
    let fused: DfFusionState = serde_json::from_slice(&body).expect("json");
    let estimate = fused.estimate.expect("two stations give an estimate");
    let error = geo::distance_m(
        LatLon {
            lat: estimate.lat,
            lon: estimate.lon,
        },
        target,
    );
    assert!(error < 600.0, "{error} m away: {estimate:?}");
    assert_eq!(fused.stations.len(), 2);

    let mut events = state.engine.subscribe_events();
    let (status, _) = request(app.clone(), "DELETE", "/api/fusion/cross", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let told = events.try_recv().expect("the clear is announced");
    assert!(matches!(
        told,
        ServerEvent::DfFusionUpdate { ref node, ref state } if node == "cross" && state.samples == 0
    ));
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
