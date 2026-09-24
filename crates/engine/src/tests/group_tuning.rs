use sdrmm_wire::{StreamSettings, Tuning};

use super::*;

fn lane(stream: u32, center_hz: Option<f64>) -> StreamSettings {
    StreamSettings {
        stream,
        center_hz,
        ..StreamSettings::default()
    }
}

fn centers(delta: &DeviceSettings) -> Vec<(u32, Option<f64>)> {
    let mut out: Vec<_> = delta
        .streams
        .iter()
        .map(|entry| (entry.stream, entry.center_hz))
        .collect();
    out.sort_by_key(|(stream, _)| *stream);
    out
}

#[test]
fn retuning_one_lane_of_a_group_moves_the_group() {
    let mut delta = DeviceSettings {
        streams: vec![lane(2, Some(433.92e6))],
        ..DeviceSettings::default()
    };
    crate::tune_together(&mut delta, &[1, 2, 3]);
    assert_eq!(
        centers(&delta),
        [
            (1, Some(433.92e6)),
            (2, Some(433.92e6)),
            (3, Some(433.92e6))
        ]
    );
}

#[test]
fn a_free_lane_tunes_alone() {
    let mut delta = DeviceSettings {
        streams: vec![lane(4, Some(100e6))],
        ..DeviceSettings::default()
    };
    crate::tune_together(&mut delta, &[0, 1, 2, 3]);
    assert_eq!(centers(&delta), [(4, Some(100e6))]);
}

#[test]
fn the_lowest_lane_decides_when_a_group_is_asked_two_ways() {
    let mut delta = DeviceSettings {
        streams: vec![lane(3, Some(868e6)), lane(1, Some(433.92e6))],
        ..DeviceSettings::default()
    };
    crate::tune_together(&mut delta, &[1, 3]);
    assert_eq!(centers(&delta), [(1, Some(433.92e6)), (3, Some(433.92e6))]);
}

#[test]
fn a_gain_on_one_lane_does_not_spread() {
    let mut delta = DeviceSettings {
        streams: vec![StreamSettings {
            stream: 1,
            gains: vec![sdrmm_wire::GainValue::new(
                sdrmm_wire::GainKind::Tuner,
                12.5,
            )],
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };
    crate::tune_together(&mut delta, &[0, 1, 2]);
    assert_eq!(delta.streams.len(), 1);
}

#[test]
fn switching_a_group_lane_to_auto_switches_the_group() {
    let mut delta = DeviceSettings {
        streams: vec![StreamSettings {
            stream: 0,
            tuning: Some(Tuning::Auto),
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };
    crate::tune_together(&mut delta, &[0, 1]);
    assert!(
        delta
            .streams
            .iter()
            .all(|entry| entry.tuning == Some(Tuning::Auto))
    );
    assert_eq!(delta.streams.len(), 2);
}
