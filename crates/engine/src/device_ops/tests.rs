use sdrmm_device::{DeviceRegistry, RxSink, SdrDevice};
use sdrmm_wire::{
    Agc, ChannelParams, ChannelSettings, Coherence, DcArtifact, DeviceInfo, Duplex, GainKind,
    GainStage, HeldLane, NfmParams, NoiseSource, Range, StreamSettings,
};

use super::*;

const CENTER_HZ: f64 = 100e6;
const ARRAY: &str = "array-1";

const BANK: StreamScope = StreamScope {
    tuning: true,
    gain: true,
    antenna: false,
    agc: false,
};

const LANE_AGC: StreamScope = StreamScope {
    tuning: true,
    gain: true,
    antenna: false,
    agc: true,
};

struct Lanes {
    capabilities: Capabilities,
    settings: DeviceSettings,
    sinks: Vec<RxSink>,
}

impl SdrDevice for Lanes {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.sinks = sinks;
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.sinks.clear();
    }
}

fn capabilities(per_stream: StreamScope) -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: 24e6,
            max: 1.7e9,
            step: None,
        }],
        sample_rates: vec![1_200_000.0, 2_400_000.0],
        sample_rate_ranges: Vec::new(),
        gains: vec![GainStage::new(
            GainKind::Tuner,
            Range {
                min: 0.0,
                max: 50.0,
                step: Some(1.0),
            },
        )],
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::Switch,
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 2,
        tx_streams: 0,
        per_stream,
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: Coherence::TimeSync,
        noise_source: NoiseSource::None,
        retune_keeps_phase: false,
        rx_stream_choices: Vec::new(),
    }
}

fn engine() -> Arc<Engine> {
    Engine::with_registry(DeviceRegistry::new(), None)
}

fn open(engine: &Engine, per_stream: StreamScope) -> u32 {
    let info = DeviceInfo {
        driver: "mock".to_owned(),
        key: "lanes".to_owned(),
        label: "Mock lanes".to_owned(),
        serial: None,
        profile: None,
    };
    let device = Lanes {
        capabilities: capabilities(per_stream),
        settings: DeviceSettings {
            center_hz: Some(CENTER_HZ),
            sample_rate: Some(2_400_000.0),
            ..DeviceSettings::default()
        },
        sinks: Vec::new(),
    };
    engine
        .create_opened_set(info, Box::new(device))
        .expect("the mock radio opens")
}

fn hold(engine: &Engine, ds: u32, stream: u32) {
    engine
        .lock()
        .device_sets
        .get_mut(&ds)
        .expect("the radio is open")
        .held
        .insert(stream, ARRAY.to_owned());
}

fn lane(stream: u32, entry: StreamSettings) -> DeviceSettings {
    DeviceSettings {
        streams: vec![StreamSettings { stream, ..entry }],
        ..DeviceSettings::default()
    }
}

fn held_by(result: Result<(), EngineError>) -> Option<String> {
    match result {
        Err(EngineError::Held { array }) => Some(array),
        _ => None,
    }
}

fn center_of_lane(engine: &Engine, ds: u32, stream: u32) -> Option<f64> {
    let inner = engine.lock();
    let state = &inner.device_sets[&ds];
    state
        .settings
        .for_stream(stream, &state.capabilities.per_stream)
        .center_hz
}

fn far_decoder() -> ChannelSettings {
    ChannelSettings {
        frequency_hz: CENTER_HZ + 5e6,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Nfm(NfmParams::default()),
        blanker: Default::default(),
    }
}

#[test]
fn a_held_lane_refuses_a_device_retune() {
    let engine = engine();
    let ds = open(&engine, BANK);
    hold(&engine, ds, 0);
    let moved = DeviceSettings {
        center_hz: Some(CENTER_HZ + 1e6),
        ..DeviceSettings::default()
    };
    assert_eq!(
        held_by(engine.patch_device(ds, moved)),
        Some(ARRAY.to_owned())
    );
    let own = StreamSettings {
        center_hz: Some(CENTER_HZ + 2e6),
        ..StreamSettings::default()
    };
    assert_eq!(
        held_by(engine.patch_device(ds, lane(0, own.clone()))),
        Some(ARRAY.to_owned())
    );
    engine
        .patch_device(ds, lane(1, own))
        .expect("a lane no array holds still tunes");
    assert_eq!(center_of_lane(&engine, ds, 1), Some(CENTER_HZ + 2e6));
    let gain = StreamSettings {
        gains: vec![GainValue::new(GainKind::Tuner, 20.0)],
        ..StreamSettings::default()
    };
    assert_eq!(
        held_by(engine.patch_device(ds, lane(0, gain))),
        Some(ARRAY.to_owned())
    );
    let rate = DeviceSettings {
        sample_rate: Some(1_200_000.0),
        ..DeviceSettings::default()
    };
    engine
        .patch_device(ds, rate)
        .expect("a rate change passes and resyncs the array");
    assert_eq!(center_of_lane(&engine, ds, 0), Some(CENTER_HZ));
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.held,
        vec![HeldLane {
            stream: 0,
            array: ARRAY.to_owned()
        }]
    );
    assert_eq!(
        EngineError::Held {
            array: ARRAY.to_owned()
        }
        .to_string(),
        "Tuned by array-1"
    );
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn per_lane_agc_on_a_held_lane_is_refused() {
    let engine = engine();
    let ds = open(&engine, LANE_AGC);
    hold(&engine, ds, 1);
    let agc = StreamSettings {
        agc: Some(AgcSetting::switched(true)),
        ..StreamSettings::default()
    };
    assert_eq!(
        held_by(engine.patch_device(ds, lane(1, agc.clone()))),
        Some(ARRAY.to_owned())
    );
    engine
        .patch_device(ds, lane(0, agc))
        .expect("agc on a free lane passes");
    let radio_wide = DeviceSettings {
        agc: Some(AgcSetting::switched(true)),
        ..DeviceSettings::default()
    };
    assert_eq!(
        held_by(engine.patch_device(ds, radio_wide)),
        Some(ARRAY.to_owned()),
        "a radio-wide AGC reaches the held lane too"
    );
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn a_radio_wide_setting_is_refused_when_it_would_reach_a_held_lane_past_its_own() {
    let engine = engine();
    let ds = open(&engine, LANE_AGC);
    let array_owned = StreamSettings {
        center_hz: Some(CENTER_HZ),
        gains: vec![GainValue::new(GainKind::Tuner, 20.0)],
        agc: Some(AgcSetting::switched(false)),
        ..StreamSettings::default()
    };
    engine
        .patch_device(ds, lane(1, array_owned))
        .expect("the array tunes its lane");
    hold(&engine, ds, 1);
    let wide = [
        DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        },
        DeviceSettings {
            gains: vec![GainValue::new(GainKind::Tuner, 40.0)],
            ..DeviceSettings::default()
        },
        DeviceSettings {
            center_hz: Some(CENTER_HZ + 3e6),
            ..DeviceSettings::default()
        },
    ];
    for delta in wide {
        assert_eq!(
            held_by(engine.patch_device(ds, delta.clone())),
            Some(ARRAY.to_owned()),
            "{delta:?} reaches the held lane in the radio"
        );
    }
    let kept = DeviceSettings {
        gains: vec![GainValue::new(GainKind::Tuner, 20.0)],
        streams: vec![StreamSettings {
            stream: 0,
            gains: vec![GainValue::new(GainKind::Tuner, 35.0)],
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };
    engine
        .patch_device(ds, kept)
        .expect("a value the held lane already has changes nothing there");
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn per_stream_agc_is_a_front_end_change() {
    let with = |on: bool| {
        lane(
            1,
            StreamSettings {
                agc: Some(AgcSetting::switched(on)),
                ..StreamSettings::default()
            },
        )
    };
    assert_ne!(front_end(&with(true)), front_end(&with(false)));
    assert_ne!(
        lane_setup(&with(true), 1, &LANE_AGC),
        lane_setup(&with(false), 1, &LANE_AGC)
    );
    assert_eq!(
        lane_setup(&with(true), 0, &LANE_AGC),
        lane_setup(&with(false), 0, &LANE_AGC),
        "the other lane keeps its front end"
    );
    let retuned = DeviceSettings {
        center_hz: Some(CENTER_HZ),
        ..with(true)
    };
    assert_eq!(front_end(&retuned), front_end(&with(true)));
}

#[test]
fn a_device_with_held_lanes_is_never_auto_centered() {
    let engine = engine();
    let ds = open(&engine, BANK);
    hold(&engine, ds, 0);
    engine
        .add_channel(ds, 1, far_decoder())
        .expect("a decoder on the other lane");
    assert_eq!(center_of_lane(&engine, ds, 1), Some(CENTER_HZ));
    assert_eq!(center_of_lane(&engine, ds, 0), Some(CENTER_HZ));
    assert!(!engine.lock().device_sets[&ds].tunes_freely());
    let allocation = engine.place_channels(&[crate::Placeable {
        node: "decoder".to_owned(),
        settings: far_decoder(),
        lanes: vec![crate::Lane {
            device_set: ds,
            stream: 1,
        }],
        held: None,
        pinned: false,
    }]);
    assert_eq!(
        allocation.coverage.heard, 0,
        "the planner does not count on moving a held radio"
    );
    engine
        .lock()
        .device_sets
        .get_mut(&ds)
        .expect("open")
        .held
        .clear();
    engine
        .add_channel(ds, 1, far_decoder())
        .expect("a second decoder once the array lets go");
    assert_ne!(
        center_of_lane(&engine, ds, 1),
        Some(CENTER_HZ),
        "a free radio follows its decoders"
    );
    engine.remove_device_set(ds).expect("closes");
}
