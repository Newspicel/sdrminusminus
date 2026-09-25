use sdrmm_wire::patch::PatchGraph;
use zgui::prelude::*;

use super::feed::Track;
use crate::ui::map::{
    ACCENT, Geo,
    geo::unwrap_trail,
    heat::Ramp,
    overlay::{Dot, EDGE, Heat, Line, Overlay},
};

#[must_use]
pub fn positions_of(graph: &PatchGraph, node: &str) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.to.node == node && edge.to.port == "position")
        .map(|edge| edge.from.node.clone())
        .collect()
}

#[must_use]
pub fn points(tracks: &[&Track]) -> Vec<Geo> {
    tracks
        .iter()
        .flat_map(|track| track.history.iter().map(|sample| sample.at))
        .collect()
}

#[must_use]
pub fn overlay(tracks: &[&Track]) -> Overlay {
    let mut out = Overlay::default();
    let all = points(tracks);
    if all.is_empty() {
        return out;
    }
    out.heat.push(Heat {
        points: all.iter().map(|at| (*at, 1.0)).collect(),
        radius: vec![(0.0, 3.0), (14.0, 22.0)],
        intensity: vec![(0.0, 0.5), (14.0, 2.0)],
        opacity: vec![(10.0, 0.65), (16.0, 0.2)],
        ramp: Ramp {
            stops: vec![
                (0.0, 0x000000, 0.0),
                (0.35, ACCENT, 1.0),
                (1.0, 0xef_62_62, 1.0),
            ],
        },
    });
    for track in tracks {
        if track.history.len() >= 2 {
            let route: Vec<Geo> = track.history.iter().map(|sample| sample.at).collect();
            out.lines.push(Line {
                points: unwrap_trail(&route),
                colour: ACCENT,
                alpha: 0.8,
                width: 2.0,
                dash: None,
            });
        }
        if track.fix.is_some()
            && let Some(last) = track.history.last()
        {
            out.dots.push(Dot {
                stroke: EDGE,
                stroke_width: 2.0,
                ..Dot::plain(last.at, 6.0, ACCENT)
            });
        }
    }
    out
}

pub fn legend_row(
    colour: u32,
    name: &'static str,
    count: impl Fn() -> String + 'static,
) -> impl IntoView {
    view! {
        row(class = "geo__legend-row") {
            box(class = "geo__swatch", style:background-color = Some(format!("#{colour:06x}")))
            text(class = "geo__legend-name") {{name}}
            text(class = "geo__legend-count") {{count}}
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        patch::{PatchEdge, PortRef},
        position::PositionFix,
    };

    use super::*;
    use crate::ui::kit_maps::feed::Sample;

    fn sample(lat: f64, lon: f64) -> Sample {
        Sample {
            at: Geo::new(lat, lon),
            altitude_m: None,
            accuracy_m: None,
            received_at: 0,
        }
    }

    #[test]
    fn a_live_track_draws_its_route_and_its_current_fix() {
        let fix = PositionFix {
            latitude: 52.6,
            longitude: 13.5,
            altitude_m: None,
            accuracy_m: None,
            speed_mps: None,
            track_deg: None,
            time: "2026-08-15T10:00:00Z".to_owned(),
        };
        let live = Track {
            fix: Some(fix),
            error: None,
            history: vec![sample(52.5, 13.4), sample(52.6, 13.5)],
        };
        let drawn = overlay(&[&live]);
        assert_eq!(drawn.lines.len(), 1);
        assert_eq!(drawn.dots.len(), 1);
        assert_eq!(drawn.dots[0].at, Geo::new(52.6, 13.5));
        let lost = Track { fix: None, ..live };
        assert!(overlay(&[&lost]).dots.is_empty());
        assert!(overlay(&[]).heat.is_empty());
    }

    #[test]
    fn a_route_across_the_antimeridian_is_unwrapped() {
        let track = Track {
            fix: None,
            error: None,
            history: vec![sample(10.0, 179.8), sample(11.0, -179.9)],
        };
        let drawn = overlay(&[&track]);
        assert!((drawn.lines[0].points[1].lon - 180.1).abs() < 1e-9);
    }

    #[test]
    fn position_sources_are_the_nodes_wired_into_the_position_port() {
        let edge = |from: &str, to: &str, port: &str| PatchEdge {
            from: PortRef {
                node: from.to_owned(),
                port: "position".to_owned(),
            },
            to: PortRef {
                node: to.to_owned(),
                port: port.to_owned(),
            },
        };
        let graph = PatchGraph {
            edges: vec![
                edge("gps", "map", "position"),
                edge("other", "map", "events"),
                edge("gps2", "else", "position"),
            ],
            ..PatchGraph::default()
        };
        assert_eq!(positions_of(&graph, "map"), ["gps"]);
    }
}
