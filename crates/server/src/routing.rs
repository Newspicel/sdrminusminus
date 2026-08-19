use sdrmm_wire::{
    MAX_ROUTE_LEG_M, Maneuver, ManeuverKind, Route, RoutePoint, RouteRequest, RoutingBackend,
};

/// Where the server sends a route request, and with what key.
///
/// The key never leaves this process: the field client asks the server for a route, and the
/// server is the only thing that has ever seen the credential.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutingOptions {
    pub backend: RoutingBackend,
    pub base_url: Option<String>,
    pub key: Option<String>,
}

impl RoutingOptions {
    #[must_use]
    pub fn configured(&self) -> bool {
        self.key.is_some() || self.base_url.is_some()
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        match (&self.base_url, self.backend) {
            (Some(base), _) => base.clone(),
            (None, RoutingBackend::OpenRouteService) => {
                "https://api.openrouteservice.org/v2/directions/driving-car/geojson".to_owned()
            }
            (None, RoutingBackend::GraphHopper) => "https://graphhopper.com/api/1/route".to_owned(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("no routing backend is configured")]
    NotConfigured,
    #[error("{0}")]
    BadRequest(String),
    #[error("the routing service could not be reached: {0}")]
    Unreachable(String),
    #[error("the routing service answered {status}")]
    Refused { status: u16 },
    #[error("the routing service answered something this build cannot read: {0}")]
    Unreadable(String),
}

/// Asks the configured service for one route, and hands back the one shape the client knows.
pub async fn route(
    options: &RoutingOptions,
    request: &RouteRequest,
) -> Result<Route, RoutingError> {
    if !options.configured() {
        return Err(RoutingError::NotConfigured);
    }
    check(request)?;
    let client = reqwest::Client::new();
    let builder = match options.backend {
        RoutingBackend::OpenRouteService => {
            let body = serde_json::json!({
                "coordinates": [
                    [request.from.lon, request.from.lat],
                    [request.to.lon, request.to.lat],
                ],
                "instructions": true,
            });
            let builder = client.post(options.endpoint()).json(&body);
            match &options.key {
                Some(key) => builder.header("Authorization", key),
                None => builder,
            }
        }
        RoutingBackend::GraphHopper => {
            let body = serde_json::json!({
                "points": [
                    [request.from.lon, request.from.lat],
                    [request.to.lon, request.to.lat],
                ],
                "profile": "car",
                "points_encoded": false,
                "instructions": true,
            });
            let mut url = options.endpoint();
            if let Some(key) = &options.key {
                let separator = if url.contains('?') { '&' } else { '?' };
                url = format!("{url}{separator}key={key}");
            }
            client.post(url).json(&body)
        }
    };
    let response = builder
        .send()
        .await
        .map_err(|error| RoutingError::Unreachable(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(RoutingError::Refused {
            status: status.as_u16(),
        });
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| RoutingError::Unreadable(error.to_string()))?;
    parse(options.backend, &body)
}

pub fn check(request: &RouteRequest) -> Result<(), RoutingError> {
    if !request.valid() {
        return Err(RoutingError::BadRequest(
            "a route needs two points on the globe".to_owned(),
        ));
    }
    if straight_line_m(request) > MAX_ROUTE_LEG_M {
        return Err(RoutingError::BadRequest(
            "that leg is longer than this build will ask a routing service for".to_owned(),
        ));
    }
    Ok(())
}

fn straight_line_m(request: &RouteRequest) -> f64 {
    crate::df_fusion::distance_m(
        (request.from.lat, request.from.lon),
        (request.to.lat, request.to.lon),
    )
}

/// Turns whichever shape the service answered with into the one shape the client knows.
pub fn parse(backend: RoutingBackend, body: &serde_json::Value) -> Result<Route, RoutingError> {
    match backend {
        RoutingBackend::OpenRouteService => parse_ors(body),
        RoutingBackend::GraphHopper => parse_graphhopper(body),
    }
}

fn unreadable(what: &str) -> RoutingError {
    RoutingError::Unreadable(format!("no {what} in the answer"))
}

fn points(coordinates: &serde_json::Value) -> Vec<RoutePoint> {
    coordinates
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some(RoutePoint {
                        lon: pair.first()?.as_f64()?,
                        lat: pair.get(1)?.as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_ors(body: &serde_json::Value) -> Result<Route, RoutingError> {
    let feature = body
        .get("features")
        .and_then(|features| features.get(0))
        .ok_or_else(|| unreadable("route"))?;
    let polyline = points(
        feature
            .pointer("/geometry/coordinates")
            .ok_or_else(|| unreadable("geometry"))?,
    );
    let summary = feature
        .pointer("/properties/summary")
        .ok_or_else(|| unreadable("summary"))?;
    let steps = feature
        .pointer("/properties/segments/0/steps")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let maneuvers = steps
        .iter()
        .filter_map(|step| {
            let at = step
                .get("way_points")
                .and_then(serde_json::Value::as_array)
                .and_then(|points| points.first())
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| polyline.get(index as usize).copied())?;
            Some(Maneuver {
                at,
                kind: ors_kind(step.get("type").and_then(serde_json::Value::as_u64)),
                instruction: step
                    .get("instruction")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                distance_m: step
                    .get("distance")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_default(),
            })
        })
        .collect();
    Ok(Route {
        polyline,
        distance_m: summary
            .get("distance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default(),
        duration_s: summary
            .get("duration")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default(),
        maneuvers,
    })
}

const fn ors_kind(code: Option<u64>) -> ManeuverKind {
    match code {
        Some(0) => ManeuverKind::SharpLeft,
        Some(1) => ManeuverKind::SharpRight,
        Some(2) => ManeuverKind::Left,
        Some(3) => ManeuverKind::Right,
        Some(4) => ManeuverKind::SlightLeft,
        Some(5) => ManeuverKind::SlightRight,
        Some(6) => ManeuverKind::Continue,
        Some(7 | 8) => ManeuverKind::Roundabout,
        Some(9) => ManeuverKind::UTurn,
        Some(10) => ManeuverKind::Arrive,
        Some(11) => ManeuverKind::Depart,
        _ => ManeuverKind::Continue,
    }
}

fn parse_graphhopper(body: &serde_json::Value) -> Result<Route, RoutingError> {
    let path = body
        .get("paths")
        .and_then(|paths| paths.get(0))
        .ok_or_else(|| unreadable("route"))?;
    let polyline = points(
        path.pointer("/points/coordinates")
            .ok_or_else(|| unreadable("geometry"))?,
    );
    let maneuvers = path
        .get("instructions")
        .and_then(serde_json::Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| {
                    let at = step
                        .get("interval")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|interval| interval.first())
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|index| polyline.get(index as usize).copied())?;
                    Some(Maneuver {
                        at,
                        kind: graphhopper_kind(
                            step.get("sign").and_then(serde_json::Value::as_i64),
                        ),
                        instruction: step
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        distance_m: step
                            .get("distance")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Route {
        polyline,
        distance_m: path
            .get("distance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default(),
        duration_s: path
            .get("time")
            .and_then(serde_json::Value::as_f64)
            .map(|ms| ms / 1_000.0)
            .unwrap_or_default(),
        maneuvers,
    })
}

const fn graphhopper_kind(sign: Option<i64>) -> ManeuverKind {
    match sign {
        Some(-98) => ManeuverKind::UTurn,
        Some(-3) => ManeuverKind::SharpLeft,
        Some(-2) => ManeuverKind::Left,
        Some(-1) => ManeuverKind::SlightLeft,
        Some(1) => ManeuverKind::SlightRight,
        Some(2) => ManeuverKind::Right,
        Some(3) => ManeuverKind::SharpRight,
        Some(4) => ManeuverKind::Arrive,
        Some(5) => ManeuverKind::Depart,
        Some(6) => ManeuverKind::Roundabout,
        _ => ManeuverKind::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORS: &str = r#"{
        "features": [{
            "geometry": { "coordinates": [[7.0, 51.5], [7.01, 51.51], [7.02, 51.52]] },
            "properties": {
                "summary": { "distance": 1500.0, "duration": 180.0 },
                "segments": [{
                    "steps": [
                        { "type": 11, "instruction": "Head north", "distance": 500.0, "way_points": [0, 1] },
                        { "type": 2, "instruction": "Turn left", "distance": 1000.0, "way_points": [1, 2] },
                        { "type": 10, "instruction": "Arrive", "distance": 0.0, "way_points": [2, 2] }
                    ]
                }]
            }
        }]
    }"#;

    const GRAPHHOPPER: &str = r#"{
        "paths": [{
            "distance": 1500.0,
            "time": 180000,
            "points": { "coordinates": [[7.0, 51.5], [7.01, 51.51]] },
            "instructions": [
                { "sign": 0, "text": "Continue", "distance": 500.0, "interval": [0, 1] },
                { "sign": 4, "text": "Arrive", "distance": 0.0, "interval": [1, 1] }
            ]
        }]
    }"#;

    fn leg() -> RouteRequest {
        RouteRequest {
            from: RoutePoint {
                lat: 51.5,
                lon: 7.0,
            },
            to: RoutePoint {
                lat: 51.52,
                lon: 7.02,
            },
        }
    }

    #[test]
    fn an_open_route_service_answer_becomes_the_shape_the_client_knows() {
        let body: serde_json::Value = serde_json::from_str(ORS).expect("fixture");
        let route = parse(RoutingBackend::OpenRouteService, &body).expect("parsed");
        assert_eq!(route.polyline.len(), 3);
        assert!((route.polyline[0].lat - 51.5).abs() < 1e-9);
        assert!((route.distance_m - 1_500.0).abs() < 1e-9);
        assert!((route.duration_s - 180.0).abs() < 1e-9);
        assert_eq!(route.maneuvers.len(), 3);
        assert_eq!(route.maneuvers[0].kind, ManeuverKind::Depart);
        assert_eq!(route.maneuvers[1].kind, ManeuverKind::Left);
        assert_eq!(route.maneuvers[2].kind, ManeuverKind::Arrive);
        assert_eq!(route.maneuvers[1].instruction, "Turn left");
    }

    #[test]
    fn a_graphhopper_answer_becomes_the_same_shape() {
        let body: serde_json::Value = serde_json::from_str(GRAPHHOPPER).expect("fixture");
        let route = parse(RoutingBackend::GraphHopper, &body).expect("parsed");
        assert_eq!(route.polyline.len(), 2);
        assert!((route.duration_s - 180.0).abs() < 1e-9);
        assert_eq!(route.maneuvers[0].kind, ManeuverKind::Continue);
        assert_eq!(route.maneuvers[1].kind, ManeuverKind::Arrive);
    }

    #[test]
    fn an_answer_with_no_route_in_it_is_reported_rather_than_guessed_at() {
        let empty: serde_json::Value = serde_json::json!({ "features": [] });
        assert!(matches!(
            parse(RoutingBackend::OpenRouteService, &empty),
            Err(RoutingError::Unreadable(_))
        ));
        assert!(matches!(
            parse(RoutingBackend::GraphHopper, &empty),
            Err(RoutingError::Unreadable(_))
        ));
    }

    #[test]
    fn a_leg_off_the_globe_or_across_it_never_reaches_the_service() {
        let mut off = leg();
        off.to.lat = 200.0;
        assert!(matches!(check(&off), Err(RoutingError::BadRequest(_))));
        let far = RouteRequest {
            from: RoutePoint {
                lat: 51.5,
                lon: 7.0,
            },
            to: RoutePoint {
                lat: -33.0,
                lon: 151.0,
            },
        };
        assert!(matches!(check(&far), Err(RoutingError::BadRequest(_))));
        check(&leg()).expect("an ordinary leg is fine");
    }

    #[tokio::test]
    async fn nothing_configured_means_nothing_is_asked() {
        assert!(matches!(
            route(&RoutingOptions::default(), &leg()).await,
            Err(RoutingError::NotConfigured)
        ));
    }

    async fn stub(
        answer: axum::response::Response,
        seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let answer = std::sync::Arc::new(std::sync::Mutex::new(Some(answer)));
        let app = axum::Router::new().route(
            "/route",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, uri: axum::http::Uri, _body: String| {
                    let answer = answer.clone();
                    let seen = seen.clone();
                    async move {
                        let auth = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned();
                        seen.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push((uri.to_string(), auth));
                        answer
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                            .unwrap_or_else(|| {
                                axum::response::IntoResponse::into_response(
                                    axum::http::StatusCode::NOT_FOUND,
                                )
                            })
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{address}/route"), handle)
    }

    fn json_response(body: &str) -> axum::response::Response {
        axum::response::IntoResponse::into_response((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body.to_owned(),
        ))
    }

    #[tokio::test]
    async fn a_backend_answer_comes_back_normalized_and_the_key_never_leaves_the_server() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (url, server) = stub(json_response(ORS), seen.clone()).await;
        let options = RoutingOptions {
            backend: RoutingBackend::OpenRouteService,
            base_url: Some(url),
            key: Some("secret-key".to_owned()),
        };
        let answer = route(&options, &leg()).await.expect("a route");
        assert_eq!(answer.polyline.len(), 3);
        assert_eq!(answer.maneuvers.len(), 3);
        let calls = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "secret-key");
        assert!(!calls[0].0.contains("secret-key"), "{}", calls[0].0);
        server.abort();
    }

    #[tokio::test]
    async fn a_refusal_from_the_service_is_reported_with_its_status() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (url, server) = stub(
            axum::response::IntoResponse::into_response(axum::http::StatusCode::TOO_MANY_REQUESTS),
            seen,
        )
        .await;
        let options = RoutingOptions {
            backend: RoutingBackend::OpenRouteService,
            base_url: Some(url),
            key: Some("secret-key".to_owned()),
        };
        assert!(matches!(
            route(&options, &leg()).await,
            Err(RoutingError::Refused { status: 429 })
        ));
        server.abort();
    }

    #[test]
    fn the_endpoint_follows_the_backend_unless_one_was_given() {
        let ors = RoutingOptions {
            backend: RoutingBackend::OpenRouteService,
            base_url: None,
            key: Some("secret".to_owned()),
        };
        assert!(ors.endpoint().contains("openrouteservice"));
        let graphhopper = RoutingOptions {
            backend: RoutingBackend::GraphHopper,
            base_url: None,
            key: Some("secret".to_owned()),
        };
        assert!(graphhopper.endpoint().contains("graphhopper"));
        let custom = RoutingOptions {
            backend: RoutingBackend::GraphHopper,
            base_url: Some("http://127.0.0.1:9999/route".to_owned()),
            key: None,
        };
        assert_eq!(custom.endpoint(), "http://127.0.0.1:9999/route");
        assert!(custom.configured());
    }
}
