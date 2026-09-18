use super::*;
use crate::placement::{Lane, Placeable, Placement, Radio, place};

const A: u32 = 1;
const B: u32 = 2;

fn lane(device_set: u32) -> Lane {
    Lane {
        device_set,
        stream: 0,
    }
}

fn tuned(center_hz: f64) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(center_hz),
        sample_rate: Some(2_400_000.0),
        ..DeviceSettings::default()
    }
}

fn held_by_hand(center_hz: f64) -> DeviceSettings {
    DeviceSettings {
        tuning: Some(Tuning::Manual),
        ..tuned(center_hz)
    }
}

fn radio(device_set: u32, settings: DeviceSettings) -> Radio {
    Radio {
        device_set,
        capabilities: tuner_caps(),
        settings,
        tunes_freely: true,
        fixed: Vec::new(),
    }
}

fn decoder(node: &str, hz: f64, lanes: &[u32]) -> Placeable {
    Placeable {
        node: node.to_owned(),
        settings: ChannelSettings {
            frequency_hz: hz,
            ..nfm_settings(0.0)
        },
        lanes: lanes.iter().copied().map(lane).collect(),
        held: None,
        pinned: false,
    }
}

fn on(placed: &[Placement], node: &str) -> Option<Lane> {
    placed.iter().find(|p| p.node == node).map(|p| p.lane)
}

fn heard_by(radio: &Radio, carried: &[&Placeable], decoder: &Placeable) -> bool {
    let channels: Vec<ChannelInfo> = carried
        .iter()
        .map(|d| ChannelInfo {
            settings: d.settings.clone(),
            ..parked(0, 0.0)
        })
        .collect();
    let mut settings = radio.settings.clone();
    if let Some(delta) = crate::planning::plan_center(&radio.capabilities, &settings, &channels) {
        settings.merge_from(&delta);
    }
    crate::planning::hears(&radio.capabilities, &settings, 0, &decoder.settings)
}

fn unheard<'a>(radios: &[Radio], decoders: &'a [Placeable], placed: &[Placement]) -> Vec<&'a str> {
    decoders
        .iter()
        .filter(|decoder| {
            let Some(lane) = on(placed, &decoder.node) else {
                return true;
            };
            let radio = radios
                .iter()
                .find(|r| r.device_set == lane.device_set)
                .expect("a placed radio");
            let carried: Vec<&Placeable> = decoders
                .iter()
                .filter(|d| on(placed, &d.node) == Some(lane))
                .collect();
            !heard_by(radio, &carried, decoder)
        })
        .map(|decoder| decoder.node.as_str())
        .collect()
}

#[test]
fn a_decoder_wired_to_one_radio_goes_there_and_the_rest_fill_around_it() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [
        decoder("flex1", 100e6, &[A, B]),
        decoder("flex2", 100.5e6, &[A, B]),
        decoder("only_a", 150e6, &[A]),
        decoder("flex3", 150.3e6, &[A, B]),
    ];
    let placed = place(&decoders, &radios);
    assert_eq!(on(&placed, "only_a"), Some(lane(A)));
    assert_eq!(on(&placed, "flex3"), Some(lane(A)));
    assert_eq!(on(&placed, "flex1"), Some(lane(B)));
    assert_eq!(on(&placed, "flex2"), Some(lane(B)));
    assert!(unheard(&radios, &decoders, &placed).is_empty());
}

#[test]
fn a_radio_tuned_by_hand_takes_only_what_it_already_hears() {
    let radios = [radio(A, held_by_hand(100e6)), radio(B, tuned(500e6))];
    let decoders = [
        decoder("near", 100.2e6, &[A, B]),
        decoder("far", 400e6, &[A, B]),
    ];
    let placed = place(&decoders, &radios);
    assert_eq!(on(&placed, "near"), Some(lane(A)));
    assert_eq!(on(&placed, "far"), Some(lane(B)));
    assert!(unheard(&radios, &decoders, &placed).is_empty());
}

#[test]
fn two_self_tuning_radios_split_a_crowd_too_wide_for_one_window() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders: Vec<Placeable> = [0.0, 0.5e6, 1.0e6, 20e6, 20.5e6, 21e6]
        .iter()
        .enumerate()
        .map(|(i, offset)| decoder(&format!("d{i}"), 100e6 + offset, &[A, B]))
        .collect();
    let placed = place(&decoders, &radios);
    assert!(unheard(&radios, &decoders, &placed).is_empty());
    let on_a = placed.iter().filter(|p| p.lane == lane(A)).count();
    assert_eq!(on_a, 3, "{placed:?}");
}

#[test]
fn a_decoder_stays_where_it_is_while_that_radio_still_hears_it() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [Placeable {
        held: Some(lane(B)),
        ..decoder("d", 100.1e6, &[A, B])
    }];
    assert_eq!(on(&place(&decoders, &radios), "d"), Some(lane(B)));
}

#[test]
fn a_decoder_leaves_a_radio_that_can_no_longer_hear_it() {
    let radios = [radio(A, held_by_hand(100e6)), radio(B, tuned(100e6))];
    let decoders = [Placeable {
        held: Some(lane(A)),
        ..decoder("d", 300e6, &[A, B])
    }];
    assert_eq!(on(&place(&decoders, &radios), "d"), Some(lane(B)));
}

#[test]
fn a_decoder_no_radio_reaches_keeps_its_first_lane() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [decoder("d", 12e9, &[B, A])];
    let placed = place(&decoders, &radios);
    assert_eq!(on(&placed, "d"), Some(lane(B)));
    assert_eq!(unheard(&radios, &decoders, &placed), vec!["d"]);
}

#[test]
fn a_scanned_decoder_never_leaves_its_radio() {
    let radios = [radio(A, held_by_hand(100e6)), radio(B, tuned(100e6))];
    let decoders = [Placeable {
        held: Some(lane(A)),
        pinned: true,
        ..decoder("d", 300e6, &[A, B])
    }];
    assert_eq!(on(&place(&decoders, &radios), "d"), Some(lane(A)));
}

#[test]
fn a_lane_on_a_stream_the_radio_lacks_is_skipped() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [Placeable {
        lanes: vec![
            Lane {
                device_set: A,
                stream: 2,
            },
            lane(B),
        ],
        ..decoder("d", 100e6, &[])
    }];
    assert_eq!(on(&place(&decoders, &radios), "d"), Some(lane(B)));
}

#[test]
fn a_decoder_wired_to_a_radio_nobody_opened_is_left_unplaced() {
    let radios = [radio(A, tuned(100e6))];
    let decoders = [decoder("d", 100e6, &[7])];
    assert!(place(&decoders, &radios).is_empty());
}

#[test]
fn a_decoder_already_on_a_radio_is_not_dropped_for_a_newcomer() {
    let mut a = radio(A, tuned(100e6));
    a.fixed.push(parked(9, 0.0));
    let radios = [a, radio(B, tuned(100e6))];
    let decoders = [decoder("new", 120e6, &[A, B])];
    assert_eq!(on(&place(&decoders, &radios), "new"), Some(lane(B)));
}

#[test]
fn thirty_two_decoders_over_two_radios_are_all_heard() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let mut decoders: Vec<Placeable> = (0..31)
        .map(|i| {
            let cluster = if i % 2 == 0 { 100e6 } else { 400e6 };
            decoder(&format!("d{i}"), cluster + f64::from(i) * 25_000.0, &[A, B])
        })
        .collect();
    decoders.push(decoder("only_b", 400.9e6, &[B]));
    let started = Instant::now();
    let placed = place(&decoders, &radios);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "placing took {:?}",
        started.elapsed()
    );
    assert_eq!(placed.len(), 32);
    assert_eq!(on(&placed, "only_b"), Some(lane(B)));
    assert!(unheard(&radios, &decoders, &placed).is_empty());
    let placed_again = place(&decoders, &radios);
    assert_eq!(placed, placed_again, "placement is deterministic");
}

#[test]
fn placements_come_back_in_the_order_the_decoders_were_given() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [
        decoder("first", 100e6, &[A, B]),
        decoder("second", 100e6, &[A]),
    ];
    let placed = place(&decoders, &radios);
    let nodes: Vec<&str> = placed.iter().map(|p| p.node.as_str()).collect();
    assert_eq!(nodes, vec!["first", "second"]);
}

#[test]
fn manual_fallback_prefers_a_capable_radio_over_the_previous_carrier() {
    let mut a = radio(A, held_by_hand(100e6));
    a.capabilities.freq_ranges = vec![sdrmm_wire::Range {
        min: 90e6,
        max: 110e6,
        step: None,
    }];
    let radios = [a, radio(B, held_by_hand(200e6))];
    let decoders = [Placeable {
        held: Some(lane(A)),
        ..decoder("d", 400e6, &[A, B])
    }];
    assert_eq!(on(&place(&decoders, &radios), "d"), Some(lane(B)));
}

#[test]
fn regrouping_heard_decoders_frees_a_radio_for_another_band() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let decoders = [
        Placeable {
            held: Some(lane(A)),
            ..decoder("low_a", 100e6, &[A, B])
        },
        Placeable {
            held: Some(lane(B)),
            ..decoder("low_b", 100.1e6, &[A, B])
        },
        decoder("high", 400e6, &[A, B]),
    ];
    let placed = place(&decoders, &radios);
    assert!(
        unheard(&radios, &decoders, &placed).is_empty(),
        "{placed:?}"
    );
}

#[test]
fn small_crowds_match_exhaustive_coverage() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    for seed in 0..32_u64 {
        let mut random = seed + 1;
        let decoders: Vec<_> = (0..5)
            .map(|i| {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                let hz = 100e6 + ((random >> 32) % 8) as f64 * 1e6;
                decoder(&format!("d{i}"), hz, &[A, B])
            })
            .collect();
        let optimum = (0..32)
            .map(|mask| {
                let placed: Vec<_> = decoders
                    .iter()
                    .enumerate()
                    .map(|(i, d)| Placement {
                        node: d.node.clone(),
                        lane: lane(if mask & (1 << i) == 0 { A } else { B }),
                    })
                    .collect();
                unheard(&radios, &decoders, &placed).len()
            })
            .min()
            .unwrap();
        let placed = place(&decoders, &radios);
        assert_eq!(
            unheard(&radios, &decoders, &placed).len(),
            optimum,
            "seed {seed}: {decoders:?}, {placed:?}"
        );
    }
}

#[test]
fn a_tuning_range_boundary_can_cover_a_channel_outside_the_center_range() {
    let mut a = radio(A, tuned(100e6));
    a.capabilities.freq_ranges = vec![sdrmm_wire::Range {
        min: 100e6,
        max: 110e6,
        step: None,
    }];
    let decoders = [decoder("edge", 110.5e6, &[A])];
    let placed = place(&decoders, &[a]);
    let mut a = radio(A, tuned(100e6));
    a.capabilities.freq_ranges = vec![sdrmm_wire::Range {
        min: 100e6,
        max: 110e6,
        step: None,
    }];
    assert!(unheard(&[a], &decoders, &placed).is_empty());
}

#[test]
fn independent_streams_share_a_radio_without_sharing_their_center() {
    let mut a = radio(A, tuned(100e6));
    a.capabilities.rx_streams = 2;
    a.capabilities.per_stream.tuning = true;
    let lanes = vec![
        lane(A),
        Lane {
            device_set: A,
            stream: 1,
        },
    ];
    let decoders = [
        Placeable {
            lanes: lanes.clone(),
            ..decoder("low", 100e6, &[])
        },
        Placeable {
            lanes,
            ..decoder("high", 400e6, &[])
        },
    ];
    let placed = place(&decoders, &[a]);
    assert_ne!(on(&placed, "low"), on(&placed, "high"));
}

#[test]
fn a_whole_group_can_move_aside_to_free_a_radio() {
    let radios = [radio(A, tuned(100e6)), radio(B, tuned(100e6))];
    let mut decoders: Vec<_> = (0..10)
        .map(|i| Placeable {
            held: Some(lane(if i % 2 == 0 { A } else { B })),
            ..decoder(&format!("low{i}"), 100e6, &[A, B])
        })
        .collect();
    decoders.push(decoder("high", 400e6, &[A, B]));
    let placed = place(&decoders, &radios);
    assert!(unheard(&radios, &decoders, &placed).is_empty());
}
