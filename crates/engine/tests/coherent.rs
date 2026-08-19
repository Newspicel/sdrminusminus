#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::{VirtualDriver, array};
use sdrmm_engine::{Engine, coherent::CoherentUpdate};
use sdrmm_wire::{
    ArrayGeometry, Coherence, CoherentParams, DeviceSettings, DfAlgorithm, DfParams, DfReading,
    ExtraValue, PassiveRadarParams,
};
use tokio::sync::broadcast;

const ARRAY: &str = "virtual:array4";
const RATE: f64 = 1_024_000.0;
const CENTRE_HZ: f64 = 300_000_000.0;
const BEARING_DEG: f64 = 137.0;

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

fn number(name: &str, value: f64) -> ExtraValue {
    ExtraValue {
        name: name.to_owned(),
        value: serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, Into::into),
    }
}

fn flag(name: &str, value: bool) -> ExtraValue {
    ExtraValue {
        name: name.to_owned(),
        value: serde_json::Value::Bool(value),
    }
}

fn tuned(extra: Vec<ExtraValue>) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(CENTRE_HZ),
        sample_rate: Some(RATE),
        extra,
        ..DeviceSettings::default()
    }
}

fn df_params(algorithm: DfAlgorithm) -> CoherentParams {
    CoherentParams::Df(DfParams {
        geometry: ArrayGeometry::Uca {
            radius_m: 0.35,
            count: 4,
        },
        algorithm,
        report_ms: 100,
        offset_hz: array::WAVEFRONT_OFFSET_HZ,
        bandwidth_hz: 20_000.0,
        sources: 1,
        cal: sdrmm_wire::CalParams::default(),
    })
}

async fn next_update(rx: &mut broadcast::Receiver<CoherentUpdate>) -> CoherentUpdate {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a coherent update within the timeout")
        {
            Ok(update) => return update,
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("the update stream closed"),
        }
    }
}

async fn first_reading(rx: &mut broadcast::Receiver<CoherentUpdate>) -> DfReading {
    for _ in 0..200 {
        if let Some(reading) = next_update(rx).await.reading {
            return reading;
        }
    }
    panic!("no bearing after two hundred updates");
}

#[tokio::test]
async fn a_steered_wavefront_reads_back_as_the_bearing_it_was_set_to() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    assert_eq!(engine.coherence_of(ds), Coherence::PhaseCoherent);
    engine
        .patch_device(
            ds,
            tuned(vec![
                number(array::BEARING_SETTING, BEARING_DEG),
                number(array::RADIUS_SETTING, 0.35),
            ]),
        )
        .unwrap();

    let node = engine
        .add_coherent(ds, df_params(DfAlgorithm::Music), vec![0, 1, 2, 3])
        .unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("an update channel");
    let reading = first_reading(&mut updates).await;
    let error = (f64::from(reading.bearing_deg) - BEARING_DEG).abs();
    assert!(
        error.min(360.0 - error) < 2.0,
        "wanted {BEARING_DEG}, read {reading:?}"
    );
    assert_eq!(reading.pseudospectrum.len(), 360);

    engine.remove_coherent(ds, node).unwrap();
    assert!(engine.coherent_nodes(ds).is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn the_beamformer_finds_the_same_bearing_the_subspace_estimator_does() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    engine
        .patch_device(
            ds,
            tuned(vec![
                number(array::BEARING_SETTING, BEARING_DEG),
                number(array::RADIUS_SETTING, 0.35),
            ]),
        )
        .unwrap();
    engine
        .add_coherent(ds, df_params(DfAlgorithm::Correlative), vec![0, 1, 2, 3])
        .unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("an update channel");
    let reading = first_reading(&mut updates).await;
    let error = (f64::from(reading.bearing_deg) - BEARING_DEG).abs();
    assert!(
        error.min(360.0 - error) < 8.0,
        "wanted {BEARING_DEG}, read {reading:?}"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_scrambled_array_without_a_pilot_reports_unknown_phase_and_no_bearing() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    engine
        .patch_device(
            ds,
            tuned(vec![
                number(array::BEARING_SETTING, BEARING_DEG),
                number(array::RADIUS_SETTING, 0.35),
                flag(array::SCRAMBLE_SETTING, true),
            ]),
        )
        .unwrap();
    assert_eq!(engine.coherence_of(ds), Coherence::TimeSync);
    engine
        .add_coherent(ds, df_params(DfAlgorithm::Music), vec![0, 1, 2, 3])
        .unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("an update channel");

    for _ in 0..8 {
        let update = next_update(&mut updates).await;
        assert!(update.reading.is_none(), "a bearing was reported anyway");
        assert!(update.cal.phase_unknown, "{:?}", update.cal);
        assert_eq!(update.cal.tier, Coherence::TimeSync);
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn an_injected_echo_shows_up_as_a_radar_detection() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    engine
        .patch_device(
            ds,
            tuned(vec![
                number(array::ECHO_DELAY_SETTING, 60.0),
                number(array::ECHO_DOPPLER_SETTING, 120.0),
                number(array::ECHO_GAIN_SETTING, -12.0),
            ]),
        )
        .unwrap();
    let radar = CoherentParams::PassiveRadar(PassiveRadarParams {
        cpi_ms: 20,
        max_range_bins: 256,
        doppler_span_hz: 1_200.0,
        eca: sdrmm_wire::EcaParams {
            delay_taps: 8,
            doppler_bins: 0,
            batch_samples: 20_480,
            loading: 1e-6,
        },
        cfar: sdrmm_wire::CfarParams::default(),
        illuminator: None,
    });
    engine.add_coherent(ds, radar, vec![0, 1]).unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("an update channel");
    let mut surfaces = engine.subscribe_surfaces(ds).expect("a surface channel");

    let mut found = None;
    for _ in 0..60 {
        let update = next_update(&mut updates).await;
        if let Some(detection) = update
            .detections
            .iter()
            .find(|detection| detection.range_bin == 60)
        {
            found = Some(*detection);
            break;
        }
    }
    let detection = found.expect("the injected echo was never detected");
    assert!((detection.doppler_hz - 120.0).abs() < 80.0, "{detection:?}");
    let surface = tokio::time::timeout(Duration::from_secs(10), surfaces.recv())
        .await
        .expect("a surface within the timeout")
        .expect("a surface");
    assert_eq!(surface.surface.ranges, 256);
    assert_eq!(
        surface.surface.cells.len(),
        surface.surface.ranges * surface.surface.dopplers
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_radio_whose_lanes_share_nothing_refuses_a_coherent_node() {
    let engine = engine();
    let ds = engine.create_device_set("virtual:transceiver").unwrap();
    assert_eq!(engine.coherence_of(ds), Coherence::None);
    let error = engine
        .add_coherent(ds, df_params(DfAlgorithm::Music), vec![0, 1, 2, 3])
        .expect_err("a non-coherent radio must be refused");
    assert!(error.is_bad_request(), "{error}");
    engine.remove_device_set(ds).unwrap();
}
