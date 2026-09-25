use std::f64::consts::TAU;

use sdrmm_wire::{
    coherent::{DfEstimate, DfGuidance, DfStation},
    patch::{NodeBody, PatchGraph},
};

use crate::ui::{
    kit_maps::feed::Finder,
    map::{
        ACCENT, Geo,
        geo::{bearing_deg, destination, great_circle_km},
        overlay::{Dot, Line, Overlay, Polygon, WHITE},
    },
};

pub const RAY_LENGTH_M: f64 = 25_000.0;
pub const ELLIPSE_POINTS: usize = 48;
pub const BEARING_MAX_AGE_MS: i64 = 5 * 60_000;
const CONVERGED: u32 = 0x3f_ae_7a;
const STATION: u32 = 0xb0_7d_e0;
const BISTATIC: u32 = 0x7f_b2_e0;
const NAV: u32 = 0xe0_a4_58;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub from: Geo,
    pub bearing_deg: f64,
    pub confidence: f64,
    pub age_ms: i64,
}

#[must_use]
pub fn ray_weight(ray: &Ray, max_age_ms: i64) -> Option<f64> {
    (ray.age_ms <= max_age_ms)
        .then(|| (ray.confidence * (1.0 - ray.age_ms as f64 / max_age_ms.max(1) as f64)).max(0.05))
}

#[must_use]
pub fn ray_line(ray: &Ray) -> [Geo; 2] {
    [
        ray.from,
        destination(ray.from, ray.bearing_deg, RAY_LENGTH_M),
    ]
}

fn ring(centre: Geo, along: f64, across: f64, axis_deg: f64) -> Vec<Geo> {
    let (sin, cos) = axis_deg.to_radians().sin_cos();
    (0..=ELLIPSE_POINTS)
        .map(|step| {
            let angle = step as f64 / ELLIPSE_POINTS as f64 * TAU;
            let forward = along * angle.cos();
            let sideways = across * angle.sin();
            let east = forward * sin + sideways * cos;
            let north = forward * cos - sideways * sin;
            destination(centre, east.atan2(north).to_degrees(), east.hypot(north))
        })
        .collect()
}

#[must_use]
pub fn ellipse(estimate: &DfEstimate) -> Vec<Geo> {
    ring(
        Geo::new(estimate.lat, estimate.lon),
        estimate.ellipse_major_m / 2.0,
        estimate.ellipse_minor_m / 2.0,
        estimate.ellipse_bearing_deg,
    )
}

#[must_use]
pub fn bistatic_ring(receiver: Geo, illuminator: Geo, range_km: f64) -> Option<Vec<Geo>> {
    let range_m = range_km * 1_000.0;
    if range_m.is_nan() || range_m <= 0.0 {
        return None;
    }
    let baseline_m = great_circle_km(receiver, illuminator) * 1_000.0;
    let along = (baseline_m + range_m) / 2.0;
    let across = (range_m * (range_m + 2.0 * baseline_m)).sqrt() / 2.0;
    let axis = bearing_deg(receiver, illuminator);
    let centre = destination(receiver, axis, baseline_m / 2.0);
    Some(ring(centre, along, across, axis))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Radar {
    pub node: String,
    pub illuminator: Geo,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    pub finders: Vec<String>,
    pub crossings: Vec<String>,
    pub radars: Vec<Radar>,
}

#[must_use]
pub fn sources_of(graph: &PatchGraph, node: &str) -> Sources {
    let mut out = Sources::default();
    for edge in &graph.edges {
        if edge.to.node != node || edge.to.port != "events" {
            continue;
        }
        let Some(found) = graph.node(&edge.from.node) else {
            continue;
        };
        match &found.body {
            NodeBody::Df(_) => out.finders.push(found.id.clone()),
            NodeBody::Triangulation => out.crossings.push(found.id.clone()),
            NodeBody::PassiveRadar(radar) => {
                if let Some(illuminator) = radar.settings.illuminator {
                    out.radars.push(Radar {
                        node: found.id.clone(),
                        illuminator: Geo::new(illuminator.lat, illuminator.lon),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picture {
    pub rays: Vec<Ray>,
    pub estimate: Option<DfEstimate>,
    pub guidance: Option<DfGuidance>,
    pub stations: Vec<DfStation>,
    pub bistatic: Vec<(Geo, Geo, f64)>,
    pub from: Option<Geo>,
}

#[must_use]
pub fn picture(
    sources: &Sources,
    finder: impl Fn(&str) -> Option<Finder>,
    now: i64,
    from: Option<Geo>,
) -> Picture {
    let mut out = Picture {
        from,
        ..Picture::default()
    };
    for node in &sources.finders {
        let (Some(held), Some(at)) = (finder(node), from) else {
            continue;
        };
        out.rays.extend(held.history.iter().map(|bearing| Ray {
            from: at,
            bearing_deg: bearing.deg,
            confidence: bearing.confidence,
            age_ms: (now - bearing.at).max(0),
        }));
    }
    for node in &sources.crossings {
        let Some(fusion) = finder(node).and_then(|held| held.fusion) else {
            continue;
        };
        out.estimate = out.estimate.or(fusion.estimate);
        out.guidance = out.guidance.or(fusion.guidance);
        out.stations.extend(fusion.stations);
    }
    for radar in &sources.radars {
        let (Some(held), Some(receiver)) = (finder(&radar.node), from) else {
            continue;
        };
        out.bistatic.extend(
            held.detections
                .iter()
                .map(|hit| (receiver, radar.illuminator, f64::from(hit.range_km))),
        );
    }
    out
}

#[must_use]
pub fn overlay(picture: &Picture) -> Overlay {
    let mut out = Overlay::default();
    for ray in &picture.rays {
        let Some(weight) = ray_weight(ray, BEARING_MAX_AGE_MS) else {
            continue;
        };
        out.lines.push(Line {
            points: ray_line(ray).to_vec(),
            colour: ACCENT,
            alpha: (0.08 + 0.82 * weight.clamp(0.0, 1.0)) as f32,
            width: 1.5,
            dash: None,
        });
    }
    if let Some(estimate) = &picture.estimate {
        out.polygons.push(Polygon {
            ring: ellipse(estimate),
            fill: ACCENT,
            fill_alpha: 0.12,
            stroke: Some((ACCENT, 0.6, 1.0)),
        });
    }
    for (receiver, illuminator, range_km) in &picture.bistatic {
        if let Some(points) = bistatic_ring(*receiver, *illuminator, *range_km) {
            out.lines.push(Line {
                points,
                colour: BISTATIC,
                alpha: 0.7,
                width: 1.0,
                dash: Some([3.0, 2.0]),
            });
        }
    }
    if let (Some(from), Some(guidance)) = (picture.from, &picture.guidance) {
        out.lines.push(Line {
            points: vec![
                from,
                Geo::new(guidance.nav_target.lat, guidance.nav_target.lon),
            ],
            colour: NAV,
            alpha: 1.0,
            width: 2.0,
            dash: Some([2.0, 2.0]),
        });
    }
    if let Some(estimate) = &picture.estimate {
        out.dots.push(Dot {
            stroke: WHITE,
            stroke_width: 1.5,
            ..Dot::plain(
                Geo::new(estimate.lat, estimate.lon),
                6.0,
                if estimate.converged {
                    CONVERGED
                } else {
                    ACCENT
                },
            )
        });
    }
    for station in &picture.stations {
        out.dots.push(Dot {
            stroke: WHITE,
            ..Dot::plain(Geo::new(station.lat, station.lon), 4.0, STATION)
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::coherent::{GuidanceMode, NavTarget, NavTargetKind};

    use super::*;

    const HOME: Geo = Geo::new(51.5, 7.0);

    fn ray(age_ms: i64) -> Ray {
        Ray {
            from: HOME,
            bearing_deg: 45.0,
            confidence: 0.9,
            age_ms,
        }
    }

    fn estimate(converged: bool) -> DfEstimate {
        DfEstimate {
            lat: 51.55,
            lon: 7.05,
            ellipse_major_m: 800.0,
            ellipse_minor_m: 200.0,
            ellipse_bearing_deg: 45.0,
            converged,
            samples: 8,
        }
    }

    #[test]
    fn a_bearing_is_drawn_out_to_the_ray_length() {
        let [start, end] = ray_line(&ray(0));
        assert_eq!(start, HOME);
        assert_eq!(end, destination(HOME, 45.0, RAY_LENGTH_M));
        assert_eq!(
            overlay(&Picture {
                rays: vec![ray(0)],
                ..Picture::default()
            })
            .lines
            .len(),
            1
        );
    }

    #[test]
    fn an_older_bearing_fades_and_one_past_its_age_is_dropped() {
        let fresh = ray_weight(&ray(0), 60_000).expect("fresh");
        let old = ray_weight(&ray(45_000), 60_000).expect("old");
        assert!(old < fresh);
        assert!(ray_weight(&ray(90_000), 60_000).is_none());
    }

    #[test]
    fn nothing_is_marked_until_there_is_an_estimate() {
        assert!(overlay(&Picture::default()).dots.is_empty());
        let marked = overlay(&Picture {
            estimate: Some(estimate(true)),
            ..Picture::default()
        });
        assert_eq!(marked.dots[0].fill, CONVERGED);
        assert_eq!(marked.dots[0].at, Geo::new(51.55, 7.05));
        assert_eq!(marked.polygons.len(), 1);
    }

    #[test]
    fn the_ellipse_closes_and_is_longer_along_its_major_axis() {
        let points = ellipse(&estimate(false));
        assert!(points.len() > 8);
        let (first, last) = (points[0], points[points.len() - 1]);
        assert!((first.lat - last.lat).abs() < 1e-9 && (first.lon - last.lon).abs() < 1e-9);
        let spans: Vec<f64> = points
            .iter()
            .map(|at| (at.lon - 7.05).hypot(at.lat - 51.55))
            .collect();
        let (low, high) = spans
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(low, high), span| {
                (low.min(*span), high.max(*span))
            });
        assert!(high > low * 1.5);
    }

    #[test]
    fn every_station_that_reported_is_placed() {
        let drawn = overlay(&Picture {
            stations: vec![DfStation {
                station_id: "north".to_owned(),
                lat: 51.5,
                lon: 7.0,
                bearings: 3,
                last_seen: "now".to_owned(),
            }],
            ..Picture::default()
        });
        assert_eq!(drawn.dots[0].fill, STATION);
    }

    #[test]
    fn the_leg_to_the_nav_target_needs_both_ends() {
        let guidance = DfGuidance {
            heading_deg: 135.0,
            mode: GuidanceMode::Cross,
            distance_m: 1_500.0,
            nav_target: NavTarget {
                lat: 51.51,
                lon: 7.02,
                kind: NavTargetKind::Cross,
            },
        };
        let drawn = overlay(&Picture {
            from: Some(HOME),
            guidance: Some(guidance),
            ..Picture::default()
        });
        assert_eq!(drawn.lines[0].points, [HOME, Geo::new(51.51, 7.02)]);
        assert!(
            overlay(&Picture {
                guidance: Some(guidance),
                ..Picture::default()
            })
            .lines
            .is_empty()
        );
        assert!(
            overlay(&Picture {
                from: Some(HOME),
                ..Picture::default()
            })
            .lines
            .is_empty()
        );
    }

    #[test]
    fn every_point_of_a_bistatic_ring_adds_up_to_the_echo_path() {
        let illuminator = Geo::new(51.5, 7.3);
        let points = bistatic_ring(HOME, illuminator, 4.0).expect("a ring");
        assert!(points.len() > 8);
        let baseline = great_circle_km(HOME, illuminator);
        for at in &points {
            let total = great_circle_km(HOME, *at) + great_circle_km(illuminator, *at);
            assert!((total - (baseline + 4.0)).abs() < 0.05, "{total}");
        }
        let (first, last) = (points[0], points[points.len() - 1]);
        assert!((first.lat - last.lat).abs() < 1e-9 && (first.lon - last.lon).abs() < 1e-9);
        assert!(bistatic_ring(HOME, illuminator, 0.0).is_none());
    }

    #[test]
    fn one_contour_per_echo_and_none_without_a_receiver() {
        let echoes = |from| Picture {
            from,
            bistatic: vec![
                (HOME, Geo::new(51.5, 7.3), 4.0),
                (HOME, Geo::new(51.5, 7.3), 0.0),
            ],
            ..Picture::default()
        };
        assert_eq!(overlay(&echoes(Some(HOME))).lines.len(), 1);
        let sources = Sources {
            radars: vec![Radar {
                node: "radar".to_owned(),
                illuminator: Geo::new(51.5, 7.3),
            }],
            ..Sources::default()
        };
        let held = |_: &str| {
            Some(Finder {
                detections: vec![sdrmm_wire::coherent::RadarDetection {
                    range_bin: 1,
                    range_km: 4.0,
                    doppler_hz: 0.0,
                    snr_db: 12.0,
                    track_id: None,
                }],
                ..Finder::default()
            })
        };
        assert_eq!(picture(&sources, held, 0, Some(HOME)).bistatic.len(), 1);
        assert!(picture(&sources, held, 0, None).bistatic.is_empty());
    }
}
