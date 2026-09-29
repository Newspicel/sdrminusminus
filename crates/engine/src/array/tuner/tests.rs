use sdrmm_wire::{
    Agc, AgcSetting, Coherence, DcArtifact, Duplex, GainStage, NoiseSource, StreamScope,
};

use super::*;

const RTL_STEPS: [f64; 6] = [0.0, 9.0, 19.7, 29.7, 40.2, 49.6];

fn caps(streams: u32, per_stream: StreamScope, steps: &[f64]) -> Capabilities {
    Capabilities {
        freq_ranges: Vec::new(),
        sample_rates: vec![2_400_000.0],
        sample_rate_ranges: Vec::new(),
        gains: vec![
            GainStage::new(
                GainKind::Tuner,
                Range {
                    min: 0.0,
                    max: 49.6,
                    step: None,
                },
            )
            .with_values(steps.to_vec()),
        ],
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::Switch,
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: streams,
        tx_streams: 0,
        per_stream,
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: Coherence::TimeSync,
        noise_source: NoiseSource::Isolated,
        retune_keeps_phase: false,
    }
}

const BANK: StreamScope = StreamScope {
    tuning: true,
    gain: true,
    antenna: false,
    agc: false,
};

const SHARED: StreamScope = StreamScope {
    tuning: false,
    gain: true,
    antenna: false,
    agc: false,
};

fn rated() -> DeviceSettings {
    DeviceSettings {
        sample_rate: Some(2_400_000.0),
        ..DeviceSettings::default()
    }
}

fn lanes(device_set: u32, streams: u32) -> Vec<Option<LaneRef>> {
    (0..streams)
        .map(|stream| Some(LaneRef { device_set, stream }))
        .collect()
}

fn one(device_set: u32, caps: Capabilities) -> BTreeMap<u32, Capabilities> {
    BTreeMap::from([(device_set, caps)])
}

fn tune(center_hz: f64, db: f64) -> ArrayTune {
    ArrayTune {
        center_hz,
        gain: ArrayGain::Manual { db },
    }
}

#[test]
fn together_puts_every_held_lane_on_the_center() {
    let kraken = one(1, caps(5, BANK, &RTL_STEPS));
    let current = BTreeMap::from([(1, rated())]);
    let planned = plan(
        &lanes(1, 5),
        &kraken,
        &current,
        ArrayTuningMode::Together,
        &tune(433.92e6, 30.0),
    )
    .expect("a kraken tunes together");
    assert_eq!(planned.lane_centers_hz, vec![433.92e6; 5]);
    let [(ds, delta)] = planned.deltas.as_slice() else {
        panic!("one radio, one delta: {:?}", planned.deltas);
    };
    assert_eq!(*ds, 1);
    assert_eq!(delta.streams.len(), 5);
    for (stream, entry) in delta.streams.iter().enumerate() {
        assert_eq!(entry.stream, stream as u32);
        assert_eq!(entry.center_hz, Some(433.92e6));
        assert_eq!(entry.tuning, Some(Tuning::Manual));
    }
    let shared = one(2, caps(4, SHARED, &RTL_STEPS));
    let planned = plan(
        &lanes(2, 4),
        &shared,
        &BTreeMap::from([(2, rated())]),
        ArrayTuningMode::Together,
        &tune(100e6, 30.0),
    )
    .expect("one tuner for all lanes");
    let (_, delta) = &planned.deltas[0];
    assert_eq!(delta.center_hz, Some(100e6));
    assert_eq!(delta.tuning, Some(Tuning::Manual));
    assert!(delta.streams.iter().all(|entry| entry.center_hz.is_none()));
}

#[test]
fn spread_uses_auto_offsets() {
    let kraken = one(1, caps(5, BANK, &RTL_STEPS));
    let current = BTreeMap::from([(1, rated())]);
    let planned = plan(
        &lanes(1, 5),
        &kraken,
        &current,
        ArrayTuningMode::Spread,
        &tune(433.92e6, 30.0),
    )
    .expect("a bank tunes every lane on its own");
    let offsets = sdrmm_dsp::stitch::auto_offsets(5, 2_400_000.0);
    for (lane, center) in planned.lane_centers_hz.iter().enumerate() {
        assert!((center - (433.92e6 + offsets[lane])).abs() < 1e-6);
        assert_eq!(planned.deltas[0].1.streams[lane].center_hz, Some(*center));
    }
    assert!(offsets[0] < 0.0 && offsets[4] > 0.0);
}

#[test]
fn spread_on_a_radio_that_tunes_lanes_together_is_refused() {
    let shared = one(2, caps(4, SHARED, &RTL_STEPS));
    let refused = plan(
        &lanes(2, 4),
        &shared,
        &BTreeMap::from([(2, rated())]),
        ArrayTuningMode::Spread,
        &tune(100e6, 30.0),
    );
    assert_eq!(refused, Err(ArrayFailure::SpreadUnsupported));
    let dongles = BTreeMap::from([
        (3, caps(1, StreamScope::default(), &RTL_STEPS)),
        (4, caps(1, StreamScope::default(), &RTL_STEPS)),
    ]);
    let pair = vec![
        Some(LaneRef {
            device_set: 3,
            stream: 0,
        }),
        Some(LaneRef {
            device_set: 4,
            stream: 0,
        }),
    ];
    let planned = plan(
        &pair,
        &dongles,
        &BTreeMap::from([(3, rated()), (4, rated())]),
        ArrayTuningMode::Spread,
        &tune(100e6, 30.0),
    )
    .expect("one lane per radio spreads");
    assert_eq!(
        planned.deltas[0].1.center_hz,
        Some(planned.lane_centers_hz[0])
    );
    assert_eq!(
        planned.deltas[1].1.center_hz,
        Some(planned.lane_centers_hz[1])
    );
    assert!(planned.lane_centers_hz[0] < planned.lane_centers_hz[1]);
}

#[test]
fn array_gain_reaches_every_held_lane_with_agc_off() {
    let kraken = one(1, caps(5, BANK, &RTL_STEPS));
    let held = vec![
        Some(LaneRef {
            device_set: 1,
            stream: 1,
        }),
        Some(LaneRef {
            device_set: 1,
            stream: 3,
        }),
    ];
    let planned = plan(
        &held,
        &kraken,
        &BTreeMap::from([(1, rated())]),
        ArrayTuningMode::Together,
        &tune(100e6, 31.0),
    )
    .expect("gain plan");
    assert_eq!(planned.gain_db, Some(29.7));
    let (_, delta) = &planned.deltas[0];
    assert_eq!(delta.agc, Some(AgcSetting::off()));
    let streams: Vec<u32> = delta.streams.iter().map(|entry| entry.stream).collect();
    assert_eq!(streams, vec![1, 3]);
    for entry in &delta.streams {
        assert_eq!(entry.gains.len(), 1);
        assert_eq!(entry.gains[0].value_db, 29.7);
    }
    let mixed = BTreeMap::from([
        (3, caps(1, StreamScope::default(), &RTL_STEPS)),
        (
            4,
            caps(1, StreamScope::default(), &[0.0, 10.0, 20.0, 29.7, 40.0]),
        ),
    ]);
    let pair = vec![
        Some(LaneRef {
            device_set: 3,
            stream: 0,
        }),
        Some(LaneRef {
            device_set: 4,
            stream: 0,
        }),
    ];
    let planned = plan(
        &pair,
        &mixed,
        &BTreeMap::from([(3, rated()), (4, rated())]),
        ArrayTuningMode::Together,
        &tune(100e6, 22.0),
    )
    .expect("common gain");
    assert_eq!(planned.gain_db, Some(29.7), "only steps both radios hold");
    for (_, delta) in &planned.deltas {
        assert_eq!(delta.gains[0].value_db, 29.7);
        assert_eq!(delta.agc, Some(AgcSetting::off()));
    }
    let auto = plan(
        &pair,
        &mixed,
        &BTreeMap::new(),
        ArrayTuningMode::Together,
        &ArrayTune {
            center_hz: 100e6,
            gain: ArrayGain::Auto,
        },
    )
    .expect("auto gain keeps the gain and still turns agc off");
    assert_eq!(auto.gain_db, None);
    assert!(auto.deltas.iter().all(|(_, delta)| delta.gains.is_empty()));
    assert!(
        auto.deltas
            .iter()
            .all(|(_, delta)| delta.agc == Some(AgcSetting::off()))
    );
}
