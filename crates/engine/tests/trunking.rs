#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use num_complex::Complex;
use sdrmm_channels::testgen;
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::{Engine, TrunkSystem, trunking::TrunkRadio};
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{DmrDiscovery, DmrTrunkProtocol, TrunkChannelSource};
use tempfile::TempDir;

const DEVICE_RATE: f64 = 240_000.0;
const CENTER_HZ: f64 = 460_300_000.0;
const CONTROL_HZ: u64 = 460_262_500;
const TRAFFIC_HZ: u64 = 460_312_500;
const LOGICAL_CHANNEL: u16 = 42;
const COLOR_CODE: u8 = 10;
const DESTINATION: u32 = 9_995;
const SOURCE: u32 = 9_999;

const FOUND_TIMEOUT: Duration = Duration::from_secs(30);

fn band(dir: &Path) -> String {
    let mut control = testgen::dv::dmr::tier_three_grant(
        COLOR_CODE,
        LOGICAL_CHANNEL,
        1,
        DESTINATION,
        SOURCE,
        24,
        DEVICE_RATE,
    );
    let call = testgen::dv::dmr::Call {
        color_code: COLOR_CODE,
        group: true,
        encrypted: false,
        destination: DESTINATION,
        source: SOURCE,
    };
    let mut traffic = Vec::new();
    while traffic.len() < control.len() {
        traffic.extend(testgen::dv::dmr::repeater_transmission(
            &call,
            1,
            DEVICE_RATE,
        ));
    }
    traffic.truncate(control.len());

    testgen::shift(&mut control, CONTROL_HZ as f64 - CENTER_HZ, DEVICE_RATE);
    testgen::shift(&mut traffic, TRAFFIC_HZ as f64 - CENTER_HZ, DEVICE_RATE);
    let mut iq: Vec<Complex<f32>> = control
        .iter()
        .zip(&traffic)
        .map(|(control, traffic)| control + traffic)
        .collect();
    testgen::scale(&mut iq, 0.5);

    let path = dir.join("dmr_tier3_band");
    let mut writer = SigmfWriter::create(&path, DEVICE_RATE, CENTER_HZ, "trunk fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    format!("virtual:file:{}", path.display())
}

fn engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_probe_follows_the_band_to_the_channel_a_grant_points_at() {
    let dir = TempDir::new().unwrap();
    let device_id = band(dir.path());
    let engine = engine_for(dir.path());
    let device_set = engine.create_device_set(&device_id).unwrap();

    engine.configure_trunking(vec![TrunkSystem {
        node: "trunk".to_owned(),
        protocol: DmrTrunkProtocol::Auto,
        discovery: DmrDiscovery {
            enabled: true,
            ranges: Vec::new(),
            max_probes: 1,
        },
        channel_map: Vec::new(),
        learned: Vec::new(),
        radio: Some(TrunkRadio {
            device_set,
            stream: 0,
            control_hz: CONTROL_HZ,
            ignore_crc: false,
        }),
    }]);

    let learned = tokio::time::timeout(FOUND_TIMEOUT, async {
        loop {
            let found = engine.trunk_systems().into_iter().find_map(|system| {
                system
                    .channel_map
                    .into_iter()
                    .find(|channel| channel.source == TrunkChannelSource::Learned)
            });
            if let Some(found) = found {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the search never placed the granted channel");

    assert_eq!(learned.logical_channel, LOGICAL_CHANNEL);
    assert_eq!(learned.freq_hz, TRAFFIC_HZ);
}

const REPEATER_HZ: u64 = 460_312_500;
const REST_ONE: u8 = 3;
const REST_TWO: u8 = 4;

/// Both repeaters of a Capacity Plus system, saying the same thing at the same time the way they
/// do on the air: the rest channel the system is parked on, and the move when it changes.
fn capacity_plus_band(dir: &Path) -> String {
    let mut announcement =
        testgen::dv::dmr::capacity_plus_status(COLOR_CODE, REST_ONE, 8, DEVICE_RATE);
    announcement.extend(testgen::dv::dmr::capacity_plus_status(
        COLOR_CODE,
        REST_TWO,
        8,
        DEVICE_RATE,
    ));
    let mut control = announcement.clone();
    let mut repeater = announcement;
    testgen::shift(&mut control, CONTROL_HZ as f64 - CENTER_HZ, DEVICE_RATE);
    testgen::shift(&mut repeater, REPEATER_HZ as f64 - CENTER_HZ, DEVICE_RATE);
    let mut iq: Vec<Complex<f32>> = control
        .iter()
        .zip(&repeater)
        .map(|(control, repeater)| control + repeater)
        .collect();
    testgen::scale(&mut iq, 0.5);

    let path = dir.join("dmr_capacity_plus_band");
    let mut writer = SigmfWriter::create(&path, DEVICE_RATE, CENTER_HZ, "trunk fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    format!("virtual:file:{}", path.display())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_search_finds_the_other_repeater_of_a_capacity_plus_system() {
    let dir = TempDir::new().unwrap();
    let device_id = capacity_plus_band(dir.path());
    let engine = engine_for(dir.path());
    let device_set = engine.create_device_set(&device_id).unwrap();

    engine.configure_trunking(vec![TrunkSystem {
        node: "trunk".to_owned(),
        protocol: DmrTrunkProtocol::CapacityPlus,
        discovery: DmrDiscovery {
            enabled: true,
            ranges: Vec::new(),
            max_probes: 1,
        },
        channel_map: Vec::new(),
        learned: Vec::new(),
        radio: Some(TrunkRadio {
            device_set,
            stream: 0,
            control_hz: CONTROL_HZ,
            ignore_crc: false,
        }),
    }]);

    let followed = tokio::time::timeout(FOUND_TIMEOUT, async {
        loop {
            let found = engine.trunk_systems().into_iter().any(|system| {
                system
                    .followers
                    .iter()
                    .any(|follower| follower.freq_hz == REPEATER_HZ)
            });
            if found {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(
        followed.is_ok(),
        "the search never placed the system's other repeater"
    );
}
