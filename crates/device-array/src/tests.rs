use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use num_complex::Complex;
use sdrmm_device::{DeviceRegistry, RxSink};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_wire::{Coherence, StreamSettings};

use super::*;

const RATE: f64 = 250_000.0;

fn members() -> Arc<DeviceRegistry> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Arc::new(registry)
}

fn definition(members: &[&str], coherence: Coherence) -> ArrayDefinition {
    ArrayDefinition {
        key: "bench".to_owned(),
        label: "Bench array".to_owned(),
        members: members.iter().map(|member| (*member).to_owned()).collect(),
        coherence,
        shared_tuning: true,
    }
}

fn driver(definition: ArrayDefinition) -> ArrayDriver {
    let catalog = ArrayCatalog::new();
    catalog.replace(vec![definition]);
    ArrayDriver::new(catalog, members())
}

fn pair() -> ArrayDriver {
    driver(definition(
        &["virtual:siggen", "virtual:halfduplex"],
        Coherence::TimeSync,
    ))
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
fn the_array_is_probed_only_once_every_member_is_attached() {
    let present = pair();
    let found = present.probe();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id(), "array:bench");
    let profile = found[0].profile.as_ref().expect("a composite profile");
    assert_eq!(profile.rx_streams, 2);

    let absent = driver(definition(
        &["virtual:siggen", "virtual:nosuchradio"],
        Coherence::TimeSync,
    ));
    assert!(absent.probe().is_empty());
}

#[test]
fn opening_presents_one_radio_with_every_members_lanes() {
    let driver = driver(definition(
        &["virtual:array4", "virtual:halfduplex"],
        Coherence::TimeSync,
    ));
    let info = driver.probe().into_iter().next().expect("probed");
    let device = driver.open(&info).expect("opens");
    let capabilities = device.capabilities();
    assert_eq!(capabilities.rx_streams, 5, "four lanes plus one");
    assert_eq!(capabilities.tx_streams, 0);
    assert_eq!(capabilities.coherence, Coherence::TimeSync);
    assert!(!capabilities.freq_ranges.is_empty());
    assert!(!capabilities.sample_rates.is_empty());
}

#[test]
fn settings_reach_every_member_and_a_lane_reaches_only_its_own() {
    let driver = driver(ArrayDefinition {
        shared_tuning: false,
        ..definition(
            &["virtual:transceiver", "virtual:halfduplex"],
            Coherence::TimeSync,
        )
    });
    let info = driver.probe().into_iter().next().expect("probed");
    let mut device = driver.open(&info).expect("opens");
    device
        .apply(&DeviceSettings {
            center_hz: Some(200_000_000.0),
            sample_rate: Some(RATE),
            ..DeviceSettings::default()
        })
        .expect("global settings reach every member");
    assert_eq!(device.settings().sample_rate, Some(RATE));

    device
        .apply(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(200_100_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .expect("a lane of a member that tunes per stream");
    assert!(
        device
            .apply(&DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 9,
                    center_hz: Some(200_000_000.0),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            })
            .is_err(),
        "a lane this array does not have is refused"
    );
}

#[test]
fn every_lane_starts_together_and_carries_its_own_members_samples() {
    let driver = pair();
    let info = driver.probe().into_iter().next().expect("probed");
    let mut device = driver.open(&info).expect("opens");
    device
        .apply(&DeviceSettings {
            sample_rate: Some(RATE),
            ..DeviceSettings::default()
        })
        .expect("rate");
    let mut receivers = Vec::new();
    let sinks = (0..2)
        .map(|_| {
            let (tx, rx) = mpsc::channel::<(u64, usize)>();
            receivers.push(rx);
            RxSink::new(move |samples: &[Complex<f32>], index| {
                let _ = tx.send((index, samples.len()));
            })
        })
        .collect();
    device.rx_start(sinks).expect("starts");
    for receiver in &receivers {
        let (index, count) = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("every lane delivers");
        assert_eq!(
            index, 0,
            "the first block of every lane is the first sample"
        );
        assert!(count > 0);
    }
    device.rx_stop();
}

#[test]
fn a_member_that_dies_is_reported_on_the_lane_that_belongs_to_it() {
    let driver = pair();
    let info = driver.probe().into_iter().next().expect("probed");
    let mut device = driver.open(&info).expect("opens");
    let (tx, rx) = mpsc::channel::<String>();
    let mut sinks = Vec::new();
    for lane in 0..2 {
        let report = tx.clone();
        sinks.push(RxSink::with_fatal_handler(
            |_: &[Complex<f32>], _| {},
            move |error| {
                let _ = report.send(format!("lane {lane}: {error}"));
            },
        ));
    }
    let handles: Vec<_> = sinks.iter_mut().map(RxSink::share_failure).collect();
    handles[1].fail(DeviceError::Disconnected("unplugged".to_owned()));
    let seen = rx.recv_timeout(Duration::from_secs(1)).expect("reported");
    assert!(seen.starts_with("lane 1"), "{seen}");
    device.rx_stop();
}

#[test]
fn the_lanes_of_a_member_are_numbered_after_the_members_before_it() {
    let registry = members();
    let mut children: Vec<Box<dyn SdrDevice>> = Vec::new();
    for member in ["virtual:array4", "virtual:halfduplex"] {
        children.push(registry.open(member).expect("member opens").1);
    }
    let array = ArrayDevice::new(
        definition(
            &["virtual:array4", "virtual:halfduplex"],
            Coherence::TimeSync,
        ),
        children,
    );
    assert_eq!(array.locate(0), Some((0, 0)));
    assert_eq!(array.locate(3), Some((0, 3)));
    assert_eq!(array.locate(4), Some((1, 0)));
    assert_eq!(array.locate(5), None);
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
