use std::time::Duration;

use jiff::Timestamp;
use sdrmm_wire::coherent::{DfEstimate, DfFusionState, DfStation, GuidanceMode};
use zgui::prelude::*;

use super::df::{SHEET, finder_signal};
use crate::{
    binding,
    store::Store,
    ui::kit_raster::{button, readout},
};

const AGE_TICK: Duration = Duration::from_secs(1);

#[must_use]
pub fn guidance_text(mode: GuidanceMode) -> &'static str {
    match mode {
        GuidanceMode::Cross => "Drive across",
        GuidanceMode::Approach => "Drive at it",
    }
}

fn metres(value: f64) -> String {
    if value >= 1_000.0 {
        format!("{:.1} km", value / 1_000.0)
    } else {
        format!("{:.0} m", value.round())
    }
}

#[must_use]
pub fn spread_label(estimate: Option<&DfEstimate>) -> String {
    estimate.map_or_else(
        || "-".to_owned(),
        |estimate| {
            format!(
                "{} \u{d7} {}",
                metres(estimate.ellipse_major_m),
                metres(estimate.ellipse_minor_m)
            )
        },
    )
}

#[must_use]
pub fn station_age(station: &DfStation, now: Timestamp) -> String {
    let Ok(seen) = station.last_seen.parse::<Timestamp>() else {
        return "just now".to_owned();
    };
    let seconds = (now.as_millisecond() - seen.as_millisecond()).max(0) as f64 / 1_000.0;
    let seconds = seconds.round();
    if seconds < 5.0 {
        "just now".to_owned()
    } else if seconds < 90.0 {
        format!("{seconds:.0}s ago")
    } else {
        format!("{:.0}m ago", (seconds / 60.0).round())
    }
}

#[must_use]
pub fn fusion_rows(
    fusion: Option<&DfFusionState>,
    reporting: usize,
    finders: usize,
) -> Vec<(String, String)> {
    let estimate = fusion.and_then(|fusion| fusion.estimate.as_ref());
    let guidance = fusion.and_then(|fusion| fusion.guidance.as_ref());
    vec![
        ("Reporting".into(), format!("{reporting} of {finders}")),
        (
            "Estimate".into(),
            estimate.map_or_else(
                || "-".into(),
                |estimate| format!("{:.5}, {:.5}", estimate.lat, estimate.lon),
            ),
        ),
        ("Spread".into(), spread_label(estimate)),
        (
            "Guidance".into(),
            guidance.map_or_else(
                || "-".into(),
                |guidance| {
                    format!(
                        "{} \u{b7} {:.0}\u{b0}",
                        guidance_text(guidance.mode),
                        guidance.heading_deg.round()
                    )
                },
            ),
        ),
        (
            "Bearings".into(),
            fusion.map_or(0, |fusion| fusion.samples).to_string(),
        ),
    ]
}

fn clear(store: Store, node: String) {
    zgui::task::spawn_local(async move {
        if let Err(error) = store
            .api()
            .delete(&format!("/api/coherent/{node}/fusion"))
            .await
        {
            store.say(format!("cannot clear the bearings: {error}"));
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-coherent", SHEET);
    let finder = finder_signal(store, node.clone());
    let fusion = Signal::derive(move || finder.get().and_then(|finder| finder.fusion));
    let now = RwSignal::new(Timestamp::now());
    let tick = set_interval(AGE_TICK, move || now.set(Timestamp::now()));
    on_cleanup_local(move || drop(tick));
    let finders = {
        let node = node.clone();
        Signal::derive(move || binding::sources_of(&store.graph.get(), &node, "events").len())
    };
    let rows = move || {
        let fusion = fusion.get();
        let reporting = fusion.as_ref().map_or(0, |fusion| fusion.stations.len());
        fusion_rows(fusion.as_ref(), reporting, finders.get())
    };
    let stations = move || {
        let at = now.get();
        fusion
            .get()
            .map(|fusion| fusion.stations)
            .unwrap_or_default()
            .into_iter()
            .map(|station| {
                let seen = format!("{} \u{b7} {}", station.bearings, station_age(&station, at));
                view! {
                    row(class = "co__station") {
                        text {{station.station_id}}
                        text(class = "co__station_seen") {{seen}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "face") {
            {readout(rows)}
            column {{stations}}
            row(class = "section") {
                {button("Clear", Signal::stored(false), move || clear(store, node.clone()))}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(last_seen: &str) -> DfStation {
        DfStation {
            station_id: "north".into(),
            lat: 51.5,
            lon: 7.0,
            bearings: 3,
            last_seen: last_seen.into(),
        }
    }

    fn estimate() -> DfEstimate {
        DfEstimate {
            lat: 51.5,
            lon: 7.0,
            ellipse_major_m: 2_400.0,
            ellipse_minor_m: 180.0,
            ellipse_bearing_deg: 45.0,
            converged: false,
            samples: 6,
        }
    }

    #[test]
    fn reads_the_ellipse_in_units_a_driver_can_judge() {
        assert_eq!(spread_label(Some(&estimate())), "2.4 km \u{d7} 180 m");
        assert_eq!(spread_label(None), "-");
    }

    #[test]
    fn says_how_long_ago_a_finder_last_reported() {
        let now: Timestamp = "2026-01-01T00:10:00Z".parse().expect("a time");
        assert_eq!(
            station_age(&station("2026-01-01T00:09:58Z"), now),
            "just now"
        );
        assert_eq!(
            station_age(&station("2026-01-01T00:09:30Z"), now),
            "30s ago"
        );
        assert_eq!(station_age(&station("2026-01-01T00:05:00Z"), now), "5m ago");
    }

    #[test]
    fn does_not_pretend_to_know_an_unreadable_time() {
        let now: Timestamp = "2026-01-01T00:10:00Z".parse().expect("a time");
        assert_eq!(station_age(&station("who knows"), now), "just now");
    }

    #[test]
    fn a_crossing_reads_its_estimate_and_how_many_report() {
        let fusion = DfFusionState {
            estimate: Some(estimate()),
            samples: 6,
            ..DfFusionState::default()
        };
        let rows = fusion_rows(Some(&fusion), 1, 2);
        assert_eq!(rows[0].1, "1 of 2");
        assert_eq!(rows[1].1, "51.50000, 7.00000");
        assert_eq!(rows[3].1, "-");
        assert_eq!(rows[4].1, "6");
        assert_eq!(fusion_rows(None, 0, 0)[1].1, "-");
    }
}
