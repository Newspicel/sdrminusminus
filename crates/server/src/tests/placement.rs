use std::sync::Arc;

use sdrmm_wire::{ChannelParams, ChannelSettings, DeviceSettings, NfmParams, Tuning};

use super::*;

fn app() -> AppState {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    AppState::new(
        sdrmm_engine::Engine::with_registry(registry, None),
        Arc::new(crate::store::Store::open(None).unwrap()),
    )
}

#[test]
fn a_failed_move_keeps_the_original_decoder() {
    let app = app();
    let a = app.engine.create_device_set("virtual:siggen").unwrap();
    let b = app.engine.create_device_set("virtual:halfduplex").unwrap();
    for (id, center) in [(a, 100e6), (b, 400e6)] {
        app.engine
            .patch_device(
                id,
                DeviceSettings {
                    center_hz: Some(center),
                    tuning: Some(Tuning::Manual),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let settings = ChannelSettings {
        frequency_hz: 100e6,
        ..ChannelSettings::default_for("nfm").unwrap()
    };
    let channel = app
        .engine
        .add_channel_for(a, 0, settings.clone(), Some("voice"))
        .unwrap();
    let lane = Lane {
        device_set: a,
        stream: 0,
    };
    let plan = Plan {
        slots: vec![Slot {
            decoder: Placeable {
                node: "voice".to_owned(),
                settings: ChannelSettings {
                    frequency_hz: 400e6,
                    params: ChannelParams::Nfm(NfmParams {
                        bandwidth_hz: -1.0,
                        ..Default::default()
                    }),
                    ..settings.clone()
                },
                lanes: vec![
                    lane,
                    Lane {
                        device_set: b,
                        stream: 0,
                    },
                ],
                held: Some(lane),
                pinned: false,
            },
            carried: Some(Carried { lane, channel }),
        }],
        refused: Vec::new(),
    };
    let mut report = PatchApplyReport::default();
    settle(&app, &plan, &mut report);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(report.closed, 0);
    assert!(report.placement.is_none());
    let snapshot = app.engine.snapshot();
    let original = snapshot.device_sets.iter().find(|set| set.id == a).unwrap();
    assert_eq!(original.channels.len(), 1);
    assert_eq!(original.channels[0].id, channel);
    assert_eq!(original.channels[0].settings, settings);
}

#[test]
fn an_exporting_decoder_is_pinned_until_the_export_stops() {
    let app = app();
    let set = app.engine.create_device_set("virtual:siggen").unwrap();
    let id = app
        .engine
        .add_channel(set, 0, ChannelSettings::default_for("nfm").unwrap())
        .unwrap();
    let snapshot = app.engine.snapshot();
    let set = snapshot
        .device_sets
        .iter()
        .find(|candidate| candidate.id == set)
        .unwrap();
    let channel = set
        .channels
        .iter()
        .find(|channel| channel.id == id)
        .unwrap();
    assert!(!holds_channel(set, channel));
    let mut channel = channel.clone();
    channel.network_export = Some(sdrmm_wire::NetworkExportStatus {
        node: "export".to_owned(),
        stream: 0,
        settings: Default::default(),
        sample_rate: 48_000,
        center_hz: 100_000_000,
        samples: 0,
        bytes: 0,
        packets: 0,
        clients: 0,
        overruns: 0,
        error: None,
    });
    assert!(holds_channel(set, &channel));
}
