use std::sync::{Arc, Mutex};

use num_complex::Complex;
use sdrmm_device::{DeviceRegistry, RxSink, lock};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_wire::{Coherence, Range};

use super::*;

fn definition(members: &[&str], coherence: Coherence) -> ArrayDefinition {
    ArrayDefinition {
        key: "bench".into(),
        label: "Bench".into(),
        members: members.iter().map(|id| (*id).into()).collect(),
        coherence,
        shared_tuning: true,
    }
}

fn pair() -> (StreamArray, ArrayIngress) {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    let (_, one) = registry.open("virtual:siggen").expect("first source");
    let (_, two) = registry.open("virtual:halfduplex").expect("second source");
    StreamArray::new(
        &definition(
            &["virtual:siggen", "virtual:halfduplex"],
            Coherence::TimeSync,
        ),
        &[
            (one.capabilities(), one.settings()),
            (two.capabilities(), two.settings()),
        ],
    )
    .expect("compose")
}

#[test]
fn a_definition_needs_at_least_two_named_radios_and_a_shared_clock() {
    let mut good = definition(
        &["virtual:siggen", "virtual:halfduplex"],
        Coherence::TimeSync,
    );
    assert!(good.valid());
    good.members.pop();
    assert!(!good.valid(), "one radio is not an array");
    let duplicated = definition(&["virtual:siggen", "virtual:siggen"], Coherence::TimeSync);
    assert!(!duplicated.valid(), "the same radio cannot be two lanes");
    let unlocked = definition(&["virtual:siggen", "virtual:halfduplex"], Coherence::None);
    assert!(
        !unlocked.valid(),
        "a bank with no shared clock is not an array"
    );
    let named = ArrayDefinition {
        key: "a key with spaces".to_owned(),
        ..definition(
            &["virtual:siggen", "virtual:halfduplex"],
            Coherence::TimeSync,
        )
    };
    assert!(!named.valid());
}

#[test]
fn composing_exposes_the_member_lanes_without_opening_any_radio() {
    let (device, _) = pair();
    assert_eq!(device.capabilities().rx_streams, 2);
    assert_eq!(device.capabilities().tx_streams, 0);
    assert_eq!(device.capabilities().coherence, Coherence::TimeSync);
}

#[test]
fn stream_gaps_survive_composition_and_stopping_detaches_the_inputs() {
    let (mut device, ingress) = pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sinks = (0..2)
        .map(|lane| {
            let seen = seen.clone();
            RxSink::new(move |samples, index| lock(&seen).push((lane, index, samples[0].re)))
        })
        .collect();
    device.rx_start(sinks).expect("start");
    let mut inputs = ingress.take();
    inputs[0].push(&[Complex::new(1.0, 0.0); 4]);
    inputs[1].push(&[Complex::new(2.0, 0.0); 4]);
    inputs[0].push(&[Complex::new(3.0, 0.0); 4]);
    inputs[0].dropped(7);
    inputs[0].push(&[Complex::new(4.0, 0.0); 4]);
    assert_eq!(*lock(&seen), [(1, 0, 2.0), (0, 0, 3.0), (0, 11, 4.0)]);
    device.rx_stop();
    inputs[0].push(&[Complex::new(5.0, 0.0); 4]);
    assert_eq!(lock(&seen).len(), 3);
    device
        .rx_start(vec![RxSink::new(|_, _| {}), RxSink::new(|_, _| {})])
        .expect("restart");
    inputs[0].push(&[Complex::new(6.0, 0.0); 4]);
    assert_eq!(lock(&seen).len(), 3, "old inputs must stay detached");
}

#[test]
fn a_member_failure_reaches_the_array() {
    let (mut device, ingress) = pair();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let sinks = (0..2)
        .map(|lane| {
            let failures = failures.clone();
            RxSink::with_fatal_handler(
                |_, _| {},
                move |error| lock(&failures).push((lane, error.to_string())),
            )
        })
        .collect();
    device.rx_start(sinks).expect("start");
    ingress.take()[1].fail(DeviceError::Io("lost member".into()));
    assert_eq!(lock(&failures)[0].0, 1);
    assert!(lock(&failures)[0].1.contains("lost member"));
}

#[test]
fn the_composite_reaches_only_what_every_member_reaches() {
    let narrow = [Range {
        min: 100e6,
        max: 200e6,
        step: None,
    }];
    let wide = [Range {
        min: 50e6,
        max: 150e6,
        step: None,
    }];
    let both = intersect(&[&narrow, &wide]);
    assert_eq!(both.len(), 1);
    assert!((both[0].min - 100e6).abs() < 1.0);
    assert!((both[0].max - 150e6).abs() < 1.0);

    let apart = [Range {
        min: 400e6,
        max: 500e6,
        step: None,
    }];
    assert!(intersect(&[&narrow, &apart]).is_empty());
}

#[test]
fn composed_settings_preserve_each_members_gain_and_antenna() {
    let (device, _) = pair();
    let mut caps = device.capabilities().clone();
    caps.rx_streams = 1;
    caps.gains = vec![sdrmm_wire::GainStage {
        name: "RF".into(),
        range: Range {
            min: 0.0,
            max: 40.0,
            step: None,
        },
        values: Vec::new(),
    }];
    caps.antennas = vec!["A".into(), "B".into()];
    caps.per_stream = Default::default();
    let first = DeviceSettings {
        gains: vec![sdrmm_wire::GainValue {
            stage: "RF".into(),
            value_db: 10.0,
        }],
        antenna: Some("A".into()),
        ..device.settings().clone()
    };
    let second = DeviceSettings {
        gains: vec![sdrmm_wire::GainValue {
            stage: "RF".into(),
            value_db: 20.0,
        }],
        antenna: Some("B".into()),
        ..first.clone()
    };
    let (array, _) = StreamArray::new(
        &definition(&["virtual:one", "virtual:two"], Coherence::TimeSync),
        &[(&caps, &first), (&caps, &second)],
    )
    .expect("compose");
    for (lane, original) in [first, second].into_iter().enumerate() {
        let settings = array
            .settings()
            .for_stream(lane as u32, &array.capabilities().per_stream);
        assert_eq!(settings.gains, original.gains);
        assert_eq!(settings.antenna, original.antenna);
    }
}
