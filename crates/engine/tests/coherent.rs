#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

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
        beam_bearing_deg: None,
        station_id: None,
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

/// The lane the aggregator sums the array into: one past the antennas the radio actually has.
fn beam_lane(engine: &Engine, ds: u32) -> u32 {
    engine
        .snapshot()
        .device_sets
        .iter()
        .find(|set| set.id == ds)
        .map_or(0, |set| set.capabilities.rx_streams)
}

/// How far one antenna's own marker stands above everything else on a lane.
///
/// A beam is not louder than an antenna — weights that add to one leave the wanted signal exactly
/// where it was. What a beam does is change the balance between what the elements share and what
/// they do not, and this is that balance: the marker belongs to one antenna alone, so the lower
/// this reads, the more of what is left is the wavefront every element heard.
async fn unshared_margin(engine: &Arc<Engine>, ds: u32, stream: u32) -> f64 {
    let mut rx = engine.subscribe_spectrum(ds, stream).expect("a lane");
    let mut margin = f64::NEG_INFINITY;
    for _ in 0..24 {
        let snapshot = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a spectrum frame")
            .expect("a frame");
        let bins = snapshot.db.len();
        let marker = sdrmm_device_virtual::stream_marker_offset_hz(0);
        let bin = (marker / f64::from(snapshot.span_hz) + 0.5) * bins as f64;
        let at = (bin.round() as usize).min(bins - 1);
        let peak = snapshot.db[at.saturating_sub(4)..(at + 5).min(bins)]
            .iter()
            .fold(f32::NEG_INFINITY, |a, b| a.max(*b));
        let mut sorted: Vec<f32> = snapshot.db.to_vec();
        sorted.sort_by(f32::total_cmp);
        margin = margin.max(f64::from(peak - sorted[bins / 2]));
    }
    margin
}

#[tokio::test]
async fn the_beam_lane_carries_the_array_summed_towards_its_bearing() {
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
    let beam = beam_lane(&engine, ds);
    assert_eq!(beam, 4, "four antennas, then the beam");

    let params = |aimed: Option<f64>| {
        let CoherentParams::Df(df) = df_params(DfAlgorithm::Music) else {
            unreachable!("df params")
        };
        CoherentParams::Df(DfParams {
            beam_bearing_deg: aimed,
            ..df
        })
    };

    let node = engine
        .add_coherent(ds, params(Some(BEARING_DEG)), vec![0, 1, 2, 3])
        .unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("updates");
    first_reading(&mut updates).await;
    let lane = unshared_margin(&engine, ds, 0).await;
    let aimed = unshared_margin(&engine, ds, beam).await;
    assert!(
        aimed < lane - 3.0,
        "the beam must favour what the elements share: one antenna {lane:.1} dB, \
         beam {aimed:.1} dB"
    );

    // A quarter turn off a four-element circle of this size is where its pattern goes to nothing,
    // which is what makes the beam a beam rather than an average.
    engine
        .apply_coherent(ds, node, params(Some(BEARING_DEG + 90.0)), vec![0, 1, 2, 3])
        .unwrap();
    first_reading(&mut updates).await;
    let away = unshared_margin(&engine, ds, beam).await;
    assert!(
        away > aimed + 6.0,
        "steering into the null must throw the shared signal away: on target {aimed:.1} dB, \
         null {away:.1} dB"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_channel_on_the_beam_lane_hears_what_the_array_is_pointed_at() {
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
    let beam = beam_lane(&engine, ds);
    engine
        .add_coherent(ds, df_params(DfAlgorithm::Music), vec![0, 1, 2, 3])
        .unwrap();
    let mut updates = engine.subscribe_coherent(ds).expect("updates");
    first_reading(&mut updates).await;

    let channel = engine
        .add_channel(
            ds,
            beam,
            sdrmm_wire::ChannelSettings {
                offset_hz: array::WAVEFRONT_OFFSET_HZ,
                squelch_db: None,
                squelch_auto_db: None,
                params: sdrmm_wire::ChannelParams::Nfm(sdrmm_wire::NfmParams::default()),
                audio: Default::default(),
            },
        )
        .expect("a channel on the beam");
    let mut audio = engine.subscribe_audio(ds, channel).expect("audio");
    let decoded = common::settle_then_collect_second(&mut audio).await;
    let heard = decoded
        .iter()
        .map(|plane| {
            let power: f64 = plane.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
            (power / plane.len().max(1) as f64).sqrt()
        })
        .fold(0.0f64, f64::max);
    assert!(
        heard > 1e-4,
        "the beam lane fed a channel no audio at all: {heard}"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_combiner_puts_the_beam_on_what_every_antenna_hears() {
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
    let beam = beam_lane(&engine, ds);
    engine
        .add_coherent(
            ds,
            CoherentParams::Combiner(sdrmm_wire::CombinerParams {
                mode: sdrmm_wire::CombineMode::Diversity,
                lanes: 4,
                offset_hz: array::WAVEFRONT_OFFSET_HZ,
                bandwidth_hz: 20_000.0,
                update_ms: 100,
                cal: sdrmm_wire::CalParams::default(),
            }),
            vec![0, 1, 2, 3],
        )
        .unwrap();

    let lane = unshared_margin(&engine, ds, 0).await;
    let combined = unshared_margin(&engine, ds, beam).await;
    assert!(
        combined < lane - 3.0,
        "the combiner adds what the antennas share and leaves what only one hears behind: \
         one antenna {lane:.1} dB, combined {combined:.1} dB"
    );
    engine.remove_device_set(ds).unwrap();
}
