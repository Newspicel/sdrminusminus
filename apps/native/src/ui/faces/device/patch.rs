use std::{cell::RefCell, collections::HashMap, sync::Arc};

use sdrmm_wire::device::DeviceSettings;
use zgui::prelude::*;

use crate::store::Store;

#[must_use]
pub fn follow_offset(current: &DeviceSettings, delta: DeviceSettings) -> DeviceSettings {
    let (Some(offset_hz), None, Some(center_hz)) =
        (delta.offset_hz, delta.center_hz, current.center_hz)
    else {
        return delta;
    };
    let moved = offset_hz - current.offset_hz.unwrap_or(0.0);
    DeviceSettings {
        center_hz: Some(center_hz + moved),
        ..delta
    }
}

#[derive(Default)]
struct Lane {
    waiting: Option<DeviceSettings>,
    busy: bool,
}

thread_local! {
    static LANES: RefCell<HashMap<u32, Lane>> = RefCell::new(HashMap::new());
}

fn enqueue(set: u32, delta: DeviceSettings) -> bool {
    LANES.with_borrow_mut(|lanes| {
        let lane = lanes.entry(set).or_default();
        match lane.waiting.as_mut() {
            Some(waiting) => waiting.merge_from(&delta),
            None => lane.waiting = Some(delta),
        }
        !lane.busy
    })
}

fn next(set: u32) -> Option<DeviceSettings> {
    LANES.with_borrow_mut(|lanes| {
        let lane = lanes.entry(set).or_default();
        let taken = lane.waiting.take();
        lane.busy = taken.is_some();
        taken
    })
}

fn run(store: Store, set: u32) {
    let Some(settings) = next(set) else {
        store.refresh_state();
        return;
    };
    let pending = store.send_device(set, settings);
    zgui::task::spawn_local(async move {
        if let Err(error) = pending.await {
            store.say(format!("cannot change the radio: {error}"));
        }
        run(store, set);
    });
}

pub fn apply(store: Store, set: u32, delta: DeviceSettings) {
    let state = store.state.get_untracked();
    let Some(current) = state.device_sets.iter().find(|found| found.id == set) else {
        return;
    };
    let followed = follow_offset(&current.settings, delta);
    let mut next_state = (*state).clone();
    if let Some(found) = next_state
        .device_sets
        .iter_mut()
        .find(|found| found.id == set)
    {
        found.settings.merge_from(&followed);
    }
    store.state.set(Arc::new(next_state));
    if enqueue(set, followed) {
        run(store, set);
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::device::GainValue;

    use super::*;

    fn tuned(center_hz: f64, offset_hz: Option<f64>) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(center_hz),
            offset_hz,
            ..DeviceSettings::default()
        }
    }

    fn offset(offset_hz: f64) -> DeviceSettings {
        DeviceSettings {
            offset_hz: Some(offset_hz),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn what_is_shown_moves_when_only_the_offset_changes() {
        assert_eq!(
            follow_offset(&tuned(100e6, None), offset(9.75e9)).center_hz,
            Some(9.85e9)
        );
    }

    #[test]
    fn the_move_is_relative_to_the_offset_already_in_place() {
        assert_eq!(
            follow_offset(&tuned(9.85e9, Some(9.75e9)), offset(10.6e9)).center_hz,
            Some(10.7e9)
        );
    }

    #[test]
    fn a_delta_naming_a_frequency_or_no_offset_is_left_alone() {
        let named = tuned(1e9, Some(9.75e9));
        assert_eq!(follow_offset(&tuned(100e6, None), named.clone()), named);
        let gain = DeviceSettings {
            gains: vec![GainValue {
                stage: "LNA".to_owned(),
                value_db: 10.0,
            }],
            ..DeviceSettings::default()
        };
        assert_eq!(follow_offset(&tuned(100e6, None), gain.clone()), gain);
    }

    #[test]
    fn deltas_that_arrive_while_one_is_in_flight_merge_into_one() {
        assert!(enqueue(77, tuned(1e6, None)));
        assert_eq!(next(77), Some(tuned(1e6, None)));
        assert!(!enqueue(77, tuned(2e6, None)));
        assert!(!enqueue(77, offset(5.0)));
        assert_eq!(next(77), Some(tuned(2e6, Some(5.0))));
        assert_eq!(next(77), None);
        assert!(enqueue(77, tuned(3e6, None)));
    }
}
