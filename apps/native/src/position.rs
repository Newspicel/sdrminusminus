use std::{collections::HashMap, sync::Arc, time::SystemTime};

use sdrmm_wire::{position::PositionFix, ws::ServerEvent};
use zgui::prelude::*;

use crate::store::Store;

pub const HISTORY_CAPACITY: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub struct PositionSample {
    pub fix: PositionFix,
    pub received_at: SystemTime,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PositionState {
    pub fix: Option<PositionFix>,
    pub error: Option<String>,
    pub history: Vec<PositionSample>,
}

fn same_place(a: &PositionFix, b: &PositionFix) -> bool {
    a.latitude == b.latitude && a.longitude == b.longitude && a.altitude_m == b.altitude_m
}

pub fn append_sample(history: &mut Vec<PositionSample>, sample: PositionSample) {
    if let Some(last) = history.last_mut()
        && same_place(&last.fix, &sample.fix)
    {
        *last = sample;
        return;
    }
    history.push(sample);
    let overflow = history.len().saturating_sub(HISTORY_CAPACITY);
    history.drain(..overflow);
}

pub fn observe(sources: &mut HashMap<String, PositionState>, event: &ServerEvent) -> bool {
    let ServerEvent::PositionChanged { node, fix, error } = event else {
        return false;
    };
    let state = sources.entry(node.clone()).or_default();
    if let Some(fix) = fix {
        append_sample(
            &mut state.history,
            PositionSample {
                fix: fix.clone(),
                received_at: SystemTime::now(),
            },
        );
    }
    state.fix.clone_from(fix);
    state.error.clone_from(error);
    true
}

#[derive(Clone, Copy)]
pub struct Positions(pub RwSignal<Arc<HashMap<String, PositionState>>>);

impl Positions {
    pub fn provide(store: Store) -> Self {
        let positions = Self(RwSignal::new(Arc::new(HashMap::new())));
        store.on_event(move |event| {
            let mut next = (*positions.0.get_untracked()).clone();
            if observe(&mut next, event) {
                positions.0.set(Arc::new(next));
            }
        });
        provide_context(positions);
        positions
    }

    #[must_use]
    pub fn of(self, node: &str) -> Option<PositionState> {
        self.0.with(|sources| sources.get(node).cloned())
    }
}

#[must_use]
pub fn positions() -> Option<Positions> {
    use_context::<Positions>()
}

#[must_use]
pub fn grid_locator(latitude: f64, longitude: f64) -> String {
    let lon = (longitude + 180.0).clamp(0.0, 359.999_999);
    let lat = (latitude + 90.0).clamp(0.0, 179.999_999);
    let letter = |base: u8, index: f64| char::from(base + index.floor() as u8);
    [
        letter(b'A', lon / 20.0),
        letter(b'A', lat / 10.0),
        letter(b'0', (lon % 20.0) / 2.0),
        letter(b'0', lat % 10.0),
        letter(b'a', (lon % 2.0) / 2.0 * 24.0),
        letter(b'a', (lat % 1.0) * 24.0),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(latitude: f64, accuracy_m: f64) -> PositionFix {
        PositionFix {
            latitude,
            longitude: 13.405,
            altitude_m: None,
            accuracy_m: Some(accuracy_m),
            speed_mps: None,
            track_deg: None,
            time: "2026-08-14T12:00:00Z".to_owned(),
        }
    }

    fn changed(fix: PositionFix) -> ServerEvent {
        ServerEvent::PositionChanged {
            node: "gps".to_owned(),
            fix: Some(fix),
            error: None,
        }
    }

    #[test]
    fn known_stations_map_to_six_character_locators() {
        assert_eq!(grid_locator(52.52, 13.405), "JO62qm");
        assert_eq!(grid_locator(37.7749, -122.4194), "CM87ss");
    }

    #[test]
    fn exact_world_edges_stay_inside_the_final_field() {
        assert_eq!(grid_locator(90.0, 180.0), "RR99xx");
        assert_eq!(grid_locator(-90.0, -180.0), "AA00aa");
    }

    #[test]
    fn a_repeated_location_keeps_only_its_newest_measurement() {
        let mut sources = HashMap::new();
        observe(&mut sources, &changed(fix(52.52, 4.0)));
        observe(&mut sources, &changed(fix(52.52, 2.0)));
        let history = &sources["gps"].history;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].fix.accuracy_m, Some(2.0));
    }

    #[test]
    fn each_source_keeps_at_most_five_thousand_samples() {
        let mut sources = HashMap::new();
        for step in 0..=HISTORY_CAPACITY {
            observe(&mut sources, &changed(fix(step as f64 / 10_000.0, 4.0)));
        }
        let history = &sources["gps"].history;
        assert_eq!(history.len(), HISTORY_CAPACITY);
        assert_eq!(history[0].fix.latitude, 1.0 / 10_000.0);
    }

    #[test]
    fn an_error_clears_the_fix_but_keeps_the_trail() {
        let mut sources = HashMap::new();
        observe(&mut sources, &changed(fix(52.52, 4.0)));
        observe(
            &mut sources,
            &ServerEvent::PositionChanged {
                node: "gps".to_owned(),
                fix: None,
                error: Some("lost".to_owned()),
            },
        );
        let state = &sources["gps"];
        assert_eq!(state.fix, None);
        assert_eq!(state.error.as_deref(), Some("lost"));
        assert_eq!(state.history.len(), 1);
    }
}
