#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_wire::{ArrayDefinition, ChannelSettings, Coherence, DeviceSettings};

fn engine() -> Arc<Engine> {
    engine_at(None)
}

fn engine_at(recordings: Option<std::path::PathBuf>) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, recordings);
    engine.arrays().replace(vec![definition()]);
    engine
}

#[test]
fn a_recording_array_refuses_rate_changes_before_touching_any_member() {
    for recording_member in [false, true] {
        let dir = tempfile::TempDir::new().expect("recordings");
        let engine = engine_at(Some(dir.path().to_owned()));
        let sources = members(&engine);
        let array = engine.create_array_set("pair").expect("array");
        let recording = if recording_member { sources[1] } else { array };
        engine.start_recording(recording, 0).expect("record");
        assert_rate_change_is_inert(&engine, array, "locked while recording");
    }
}

fn assert_rate_change_is_inert(engine: &Engine, array: u32, reason: &str) {
    let before = engine.snapshot();
    let error = engine
        .patch_device(
            array,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .expect_err("rate change refused");
    let after = engine.snapshot();
    engine.shutdown();
    assert!(error.to_string().contains(reason), "{error}");
    assert_eq!(
        after.revision, before.revision,
        "a rejected patch must not retune and restore radios"
    );
    for (original, current) in before.device_sets.iter().zip(&after.device_sets) {
        assert_eq!(current.settings, original.settings);
    }
}

#[test]
fn every_array_rate_lock_is_checked_before_retuning_sources() {
    use sdrmm_wire::{
        ArrayGeometry, CoherentParams, DfParams, NetworkExportSettings, TimeMachineAction,
        TimeMachineNode,
    };

    for owner in ["export", "history", "coherent", "channel"] {
        let dir = tempfile::TempDir::new().expect("recordings");
        let engine = engine_at(Some(dir.path().to_owned()));
        members(&engine);
        let array = engine.create_array_set("pair").expect("array");
        let destination = std::net::UdpSocket::bind("127.0.0.1:0").expect("local destination");
        let reason = match owner {
            "export" => {
                engine
                    .start_network_export(
                        array,
                        "export".into(),
                        0,
                        NetworkExportSettings {
                            address: destination.local_addr().expect("address").to_string(),
                            ..Default::default()
                        },
                    )
                    .expect("export");
                "locked while exporting"
            }
            "history" => {
                engine
                    .control_time_machine(
                        array,
                        "history".into(),
                        0,
                        TimeMachineAction::Arm,
                        TimeMachineNode { history_seconds: 1 },
                    )
                    .expect("history");
                "holds history"
            }
            "coherent" => {
                engine
                    .add_coherent(
                        array,
                        CoherentParams::Df(DfParams {
                            geometry: ArrayGeometry::Ula {
                                spacing_m: 0.35,
                                count: 2,
                            },
                            ..Default::default()
                        }),
                        vec![0, 1],
                    )
                    .expect("coherent processor");
                "stop coherent processors"
            }
            _ => {
                let mut channel = ChannelSettings::default_for("nfm").expect("nfm");
                channel.offset_hz = 400_000.0;
                engine.add_channel(array, 0, channel).expect("channel");
                "exceeds"
            }
        };
        assert_rate_change_is_inert(&engine, array, reason);
    }
}

fn definition() -> ArrayDefinition {
    ArrayDefinition {
        key: "pair".into(),
        label: "Pair".into(),
        members: vec!["virtual:siggen".into(), "virtual:halfduplex".into()],
        coherence: Coherence::TimeSync,
        shared_tuning: true,
    }
}

fn members(engine: &Engine) -> [u32; 2] {
    [
        engine
            .create_device_set("virtual:siggen")
            .expect("source one"),
        engine
            .create_device_set("virtual:halfduplex")
            .expect("source two"),
    ]
}

#[test]
fn arrays_never_open_their_own_member_radios() {
    let engine = engine();
    assert!(engine.create_array_set("pair").is_err());
    assert!(engine.snapshot().device_sets.is_empty());
    engine.shutdown();
}

#[tokio::test]
async fn existing_channels_and_member_streams_survive_array_creation_and_removal() {
    let engine = engine();
    let [one, two] = members(&engine);
    let channel = engine
        .add_channel(one, 0, ChannelSettings::default_for("nfm").expect("nfm"))
        .expect("existing channel");
    let mut original = engine.subscribe_spectrum(one, 0).expect("source spectrum");
    let array = engine
        .create_array_set("pair")
        .expect("compose open sources");
    let mut combined = engine
        .subscribe_spectrum(array, 1)
        .expect("second member lane");
    for rx in [&mut original, &mut combined] {
        let packet = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("stream remains live")
            .expect("spectrum");
        assert!(!packet.db.is_empty());
    }
    assert_eq!(engine.snapshot().device_sets.len(), 3);
    engine.remove_device_set(array).expect("remove array only");
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.device_sets.len(), 2);
    assert!(snapshot.device_sets.iter().any(|set| set.id == two));
    assert!(
        snapshot
            .device_sets
            .iter()
            .find(|set| set.id == one)
            .expect("original source")
            .channels
            .iter()
            .any(|live| live.id == channel)
    );
    tokio::time::timeout(Duration::from_secs(5), original.recv())
        .await
        .expect("source still running")
        .expect("spectrum");
    engine.shutdown();
}

#[test]
fn removing_a_member_removes_its_array_but_preserves_the_other_radio() {
    let engine = engine();
    let [one, two] = members(&engine);
    engine.create_array_set("pair").expect("array");
    engine
        .remove_device_set(one)
        .expect("remove source and dependent array");
    let live = engine.snapshot();
    assert_eq!(live.device_sets.len(), 1);
    assert_eq!(live.device_sets[0].id, two);
    engine.shutdown();
}

#[test]
fn tuning_an_array_updates_the_original_device_sets() {
    let engine = engine();
    let members = members(&engine);
    let array = engine.create_array_set("pair").expect("array");
    engine
        .patch_device(
            array,
            DeviceSettings {
                center_hz: Some(110e6),
                ..Default::default()
            },
        )
        .expect("shared tuning");
    let live = engine.snapshot();
    for id in members.into_iter().chain([array]) {
        assert_eq!(
            live.device_sets
                .iter()
                .find(|set| set.id == id)
                .expect("set")
                .settings
                .center_hz,
            Some(110e6)
        );
    }
    assert!(
        engine
            .patch_device(
                members[0],
                DeviceSettings {
                    center_hz: Some(120e6),
                    ..Default::default()
                }
            )
            .is_err()
    );
    engine.shutdown();
}

#[test]
fn mismatched_rates_and_duplicate_ownership_are_refused() {
    let engine = engine();
    let [one, two] = members(&engine);
    engine
        .patch_device(
            one,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .expect("change rate");
    assert!(engine.create_array_set("pair").is_err());
    engine
        .patch_device(
            two,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .expect("match rate");
    engine.create_array_set("pair").expect("matched streams");
    engine.arrays().replace(vec![
        definition(),
        ArrayDefinition {
            key: "other".into(),
            ..definition()
        },
    ]);
    assert!(engine.create_array_set("other").is_err());
    engine.shutdown();
}

#[test]
fn scans_cannot_break_an_active_arrays_tuning() {
    let engine = engine();
    let [one, two] = members(&engine);
    let array = engine.create_array_set("pair").expect("array");
    for id in [one, two, array] {
        assert!(
            engine
                .start_scan(id, Default::default())
                .expect_err("array scan refused")
                .to_string()
                .contains("disconnect the array")
        );
        assert!(
            engine
                .start_scan_session(&[id], Default::default())
                .expect_err("array session refused")
                .to_string()
                .contains("disconnect the array")
        );
        assert!(
            engine
                .start_hunt(id, Default::default())
                .expect_err("array hunt refused")
                .to_string()
                .contains("disconnect the array")
        );
    }
    engine.shutdown();
}

#[tokio::test]
async fn rate_changes_keep_composed_streams_live_and_definitions_detach_cleanly() {
    let engine = engine();
    let sources = members(&engine);
    let array = engine.create_array_set("pair").expect("array");
    engine
        .patch_device(
            array,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .expect("change every rate");
    for id in sources.into_iter().chain([array]) {
        assert_eq!(
            engine
                .snapshot()
                .device_sets
                .iter()
                .find(|set| set.id == id)
                .expect("set")
                .settings
                .sample_rate,
            Some(250_000.0)
        );
    }
    let mut spectrum = engine.subscribe_spectrum(array, 1).expect("spectrum");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = spectrum.recv().await.expect("frame");
            if frame.span_hz == 250_000.0 {
                break;
            }
        }
    })
    .await
    .expect("new rate reaches the array");
    engine.arrays().replace(Vec::new());
    engine.reconcile_arrays().expect("remove stale array");
    assert_eq!(engine.snapshot().device_sets.len(), 2);
    engine.shutdown();
}

#[test]
fn independent_lane_tuning_updates_only_the_corresponding_source() {
    let engine = engine();
    engine.arrays().replace(vec![ArrayDefinition {
        shared_tuning: false,
        ..definition()
    }]);
    let [one, two] = members(&engine);
    let before = engine
        .snapshot()
        .device_sets
        .iter()
        .find(|set| set.id == one)
        .expect("first source")
        .settings
        .center_hz;
    let array = engine.create_array_set("pair").expect("array");
    engine
        .patch_device(
            array,
            DeviceSettings {
                streams: vec![sdrmm_wire::StreamSettings {
                    stream: 1,
                    center_hz: Some(110e6),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("tune second lane");
    let snapshot = engine.snapshot();
    assert_eq!(
        snapshot
            .device_sets
            .iter()
            .find(|set| set.id == one)
            .expect("first")
            .settings
            .center_hz,
        before
    );
    assert_eq!(
        snapshot
            .device_sets
            .iter()
            .find(|set| set.id == two)
            .expect("second")
            .settings
            .center_hz,
        Some(110e6)
    );
    let composed = snapshot
        .device_sets
        .iter()
        .find(|set| set.id == array)
        .expect("array");
    assert_eq!(
        composed
            .settings
            .for_stream(1, &composed.capabilities.per_stream)
            .center_hz,
        Some(110e6)
    );
    engine.shutdown();
}

#[test]
fn invalid_array_patches_do_not_partially_retune_sources() {
    let engine = engine();
    members(&engine);
    let array = engine.create_array_set("pair").expect("array");
    let before = engine.snapshot();
    assert!(
        engine
            .patch_device(
                array,
                DeviceSettings {
                    center_hz: Some(110e6),
                    streams: vec![sdrmm_wire::StreamSettings {
                        stream: 9,
                        center_hz: Some(120e6),
                        ..Default::default()
                    }],
                    ..Default::default()
                }
            )
            .is_err()
    );
    for set in engine.snapshot().device_sets {
        assert_eq!(
            set.settings,
            before
                .device_sets
                .iter()
                .find(|original| original.id == set.id)
                .expect("original")
                .settings
        );
    }
    engine.shutdown();
}
