use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_channels::array_processor::{
    ArrayBlock, CalView, CorrectionView, LaneFormat, OutputSlots, ProcessorOutput, create_processor,
};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_wire::{
    ArrayGeometry, ArrayNode, ArrayOrientation, ArrayRecordingRequest, ArrayTuneRequest,
    ArrayTuningMode, Attitude, ChannelParams, ChannelSettings, Coherence, DecoderEvent,
    DeviceSettings, DfParams, DfReading, GainValue, HeldLane, NfmParams, PositionFix,
    ProcessorParams, ProcessorReading, RdsUpdate, StreamSettings, VirtualLane, Winding,
    array::ArrayElement,
};

use super::{pose::PoseTrack, tuning::restore, *};
#[cfg(feature = "probe")]
use crate::array::ProcessorSpec;
use crate::array::{COMMAND_SLOTS, LaneRef, PoseRing, oriented};

const ARRAY: &str = "array-1";
const KRAKEN_RADIUS_M: f64 = 0.2939;
const EPOCH_NS: i128 = 1_790_000_000_000_000_000;
const HOST_NS: i64 = 5_000_000_000;
const DELIVERY_NS: i64 = 50_000_000;
const RATE: f64 = 2_400_000.0;
const BLOCK: usize = 24_000;
const BLOCK_NS: i64 = 10_000_000;
const WAIT: Duration = Duration::from_secs(10);

fn bench() -> (Arc<Engine>, u32) {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let ds = engine
        .create_device_set("virtual:kraken5")
        .expect("the bench kraken opens");
    (engine, ds)
}

fn kraken() -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m: KRAKEN_RADIUS_M,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    }
}

fn lanes(ds: u32, streams: impl IntoIterator<Item = u32>) -> Vec<Option<LaneRef>> {
    streams
        .into_iter()
        .map(|stream| {
            Some(LaneRef {
                device_set: ds,
                stream,
            })
        })
        .collect()
}

fn kraken_spec(ds: u32) -> ArraySpec {
    ArraySpec {
        node: ARRAY.to_owned(),
        lanes: lanes(ds, 0..5),
        settings: ArrayNode {
            geometry: kraken(),
            ..ArrayNode::default()
        },
        tune: None,
        warm: None,
    }
}

fn hold(engine: &Engine, ds: u32, stream: u32, array: &str) {
    engine
        .lock()
        .device_sets
        .get_mut(&ds)
        .expect("the radio is open")
        .held
        .insert(stream, array.to_owned());
}

fn refusal(result: Result<(), EngineError>) -> String {
    match result {
        Err(error) => error.to_string(),
        Ok(()) => "applied".to_owned(),
    }
}

fn wait_for<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = found() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fix(at_ms: i64, heading_deg: f64, yaw_rate_dps: Option<f64>) -> PositionFix {
    let stamp = jiff::Timestamp::from_nanosecond(EPOCH_NS + i128::from(at_ms) * 1_000_000)
        .expect("a valid instant");
    PositionFix {
        latitude: 48.1,
        longitude: 11.5,
        altitude_m: None,
        accuracy_m: Some(4.0),
        speed_mps: Some(0.0),
        track_deg: None,
        time: stamp.to_string(),
        attitude: Attitude {
            heading_deg: Some(heading_deg),
            yaw_rate_dps,
            ..Attitude::default()
        },
    }
}

fn host_ns(at_ms: i64) -> i64 {
    HOST_NS + at_ms * 1_000_000 + DELIVERY_NS
}

fn nfm_at(frequency_hz: f64) -> ChannelSettings {
    ChannelSettings {
        frequency_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Nfm(NfmParams::default()),
        blanker: Default::default(),
    }
}

#[test]
fn a_bad_array_is_refused_with_its_failure() {
    let (engine, ds) = bench();
    let mut spec = kraken_spec(ds);
    spec.lanes = lanes(ds, [0, 1, 1, 2, 3]);
    assert_eq!(refusal(engine.apply_array(spec)), "Lane 3 twice");
    let mut spec = kraken_spec(ds);
    spec.lanes = lanes(ds, [0]);
    assert_eq!(refusal(engine.apply_array(spec)), "Wire lanes");
    let mut spec = kraken_spec(ds);
    spec.lanes = (0..17).map(|_| None).collect();
    assert_eq!(refusal(engine.apply_array(spec)), "Max 16 lanes");
    let mut spec = kraken_spec(ds);
    spec.settings.geometry = ArrayGeometry::Explicit {
        positions: vec![ArrayElement::default(); 3],
    };
    assert_eq!(refusal(engine.apply_array(spec)), "Geometry has 3, wired 5");
    let mut spec = kraken_spec(ds);
    spec.lanes = lanes(ds, [0, 1, 7]);
    assert!(matches!(
        engine.apply_array(spec),
        Err(EngineError::StreamOutOfRange { stream: 7, .. })
    ));
    let mut spec = kraken_spec(ds);
    spec.lanes = lanes(ds + 40, 0..2);
    assert!(matches!(
        engine.apply_array(spec),
        Err(EngineError::DeviceSetNotFound(_))
    ));
    hold(&engine, ds, 2, "array-2");
    assert_eq!(
        refusal(engine.apply_array(kraken_spec(ds))),
        "Lane 3 in array-2"
    );
    let spec = ArraySpec {
        tune: Some(ArrayTune {
            center_hz: 433.92e6,
            gain: ArrayGain::Manual { db: 500.0 },
        }),
        lanes: lanes(ds, [0, 1]),
        ..kraken_spec(ds)
    };
    assert_eq!(refusal(engine.apply_array(spec)), "Gain out of range");
    assert!(engine.array_statuses().is_empty());
    assert!(matches!(
        engine.tune_array(ARRAY, ArrayTuneRequest::default()),
        Err(EngineError::ArrayNotFound(_))
    ));
    assert!(matches!(
        engine.update_array_pose(ARRAY, None, 0),
        Err(EngineError::ArrayNotFound(_))
    ));
    assert!(matches!(
        engine.remove_array(ARRAY),
        Err(EngineError::ArrayNotFound(_))
    ));
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn snapshot_lists_virtual_lanes_and_held_lanes() {
    let (engine, ds) = bench();
    for stream in 0..5 {
        hold(&engine, ds, stream, ARRAY);
    }
    let format = LaneFormat {
        center_hz: 433.92e6,
        sample_rate: 240_000.0,
        capacity: 4_096,
    };
    let (beam, sink) = engine
        .open_lane(ds, None, "beam-1", "beam", format)
        .expect("a lane output opens");
    let (other, other_sink) = engine
        .open_lane(ds, None, "beam-2", "beam", format)
        .expect("a second lane output opens");
    assert_eq!((beam, other), (5, 6));
    let channel = engine
        .add_channel(ds, beam, nfm_at(433.92e6))
        .expect("a channel listens on a lane output");
    engine
        .subscribe_spectrum(ds, beam)
        .expect("a lane output has a spectrum");
    let set = engine.snapshot().device_sets.remove(0);
    assert_eq!(
        set.held,
        (0..5)
            .map(|stream| HeldLane {
                stream,
                array: ARRAY.to_owned(),
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        set.virtual_lanes,
        vec![
            VirtualLane {
                stream: 5,
                node: "beam-1".to_owned(),
                port: "beam".to_owned(),
                center_hz: 433.92e6,
                sample_rate: 240_000.0,
            },
            VirtualLane {
                stream: 6,
                node: "beam-2".to_owned(),
                port: "beam".to_owned(),
                center_hz: 433.92e6,
                sample_rate: 240_000.0,
            },
        ]
    );
    let heard = set
        .channels
        .iter()
        .find(|info| info.id == channel)
        .expect("the channel is listed");
    assert_eq!(heard.stream, beam);
    assert!(!heard.out_of_band);
    assert!(matches!(
        engine.add_channel(ds, 9, nfm_at(433.92e6)),
        Err(EngineError::StreamOutOfRange {
            stream: 9,
            streams: 5
        })
    ));
    drop(sink);
    engine.close_lane(ds, beam);
    let set = engine.snapshot().device_sets.remove(0);
    assert!(set.channels.iter().all(|info| info.stream != beam));
    assert_eq!(
        set.virtual_lanes
            .iter()
            .map(|lane| lane.stream)
            .collect::<Vec<_>>(),
        vec![6]
    );
    drop(other_sink);
    engine.close_lane(ds, other);
    assert!(engine.snapshot().device_sets[0].virtual_lanes.is_empty());
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn a_pose_without_a_yaw_rate_takes_one_from_its_headings() {
    let mut track = PoseTrack::default();
    let first = track.sample(Some(&fix(0, 10.0, None)), host_ns(0));
    assert_eq!(first.host_ns, host_ns(0));
    assert_eq!(first.yaw_rate_dps, None);
    let close = track.sample(Some(&fix(10, 11.0, None)), host_ns(10));
    assert_eq!(close.yaw_rate_dps, None, "10 ms is too short to tell");
    let turned = track.sample(Some(&fix(100, 19.0, None)), host_ns(100) + 80_000_000);
    assert_eq!(
        turned.host_ns,
        host_ns(100),
        "late delivery keeps the fix time"
    );
    assert!((turned.yaw_rate_dps.expect("derived") - 90.0).abs() < 0.01);
    let given = track.sample(Some(&fix(200, 19.0, Some(-5.0))), host_ns(200));
    assert_eq!(given.yaw_rate_dps, Some(-5.0), "a measured rate wins");
    track.sample(Some(&fix(300, 355.0, None)), host_ns(300));
    let wrapped = track.sample(Some(&fix(400, 5.0, None)), host_ns(400));
    assert!((wrapped.yaw_rate_dps.expect("derived") - 100.0).abs() < 0.01);
    let stale = track.sample(Some(&fix(3_400, 50.0, None)), host_ns(3_400));
    assert_eq!(stale.yaw_rate_dps, None, "headings 3 s apart say nothing");
    let lost = track.sample(None, host_ns(3_500));
    assert_eq!((lost.heading_deg, lost.fix), (None, None));
    assert_eq!(track.last(), None);
}

#[test]
fn a_plausible_phone_clock_places_a_late_pose_at_its_own_time() {
    let wall_ns = |at: std::time::SystemTime| {
        i64::try_from(
            at.duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_nanos(),
        )
        .expect("fits")
    };
    let stamped = |at: std::time::SystemTime| PositionFix {
        time: jiff::Timestamp::from_nanosecond(i128::from(wall_ns(at)))
            .expect("a valid instant")
            .to_string(),
        ..fix(0, 10.0, None)
    };
    let mut track = PoseTrack::default();
    let received = i64::try_from(sdrmm_device::now_ns()).expect("fits");
    let late = std::time::SystemTime::now() - Duration::from_millis(400);
    let placed = track.sample(Some(&stamped(late)), received);
    let lag = received - placed.host_ns;
    assert!((390_000_000..410_000_000).contains(&lag), "lag {lag}");
    let ahead = std::time::SystemTime::now() + Duration::from_secs(30);
    let received = i64::try_from(sdrmm_device::now_ns()).expect("fits");
    let clamped = track.sample(Some(&stamped(ahead)), received);
    assert!(
        clamped.host_ns <= received,
        "a pose never lands in the future"
    );
}

#[test]
fn a_failed_retune_puts_back_only_what_it_touched() {
    let previous = DeviceSettings {
        center_hz: Some(100e6),
        gains: vec![GainValue::new(sdrmm_wire::GainKind::Tuner, 30.0)],
        agc: Some(sdrmm_wire::AgcSetting::switched(true)),
        streams: vec![StreamSettings {
            stream: 0,
            center_hz: Some(101e6),
            gains: vec![GainValue::new(sdrmm_wire::GainKind::Tuner, 20.0)],
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };
    let delta = DeviceSettings {
        center_hz: Some(433e6),
        agc: Some(sdrmm_wire::AgcSetting::off()),
        streams: vec![
            StreamSettings {
                stream: 0,
                center_hz: Some(433e6),
                tuning: Some(sdrmm_wire::Tuning::Manual),
                gains: vec![GainValue::new(sdrmm_wire::GainKind::Tuner, 40.0)],
                ..StreamSettings::default()
            },
            StreamSettings {
                stream: 3,
                center_hz: Some(433e6),
                ..StreamSettings::default()
            },
        ],
        ..DeviceSettings::default()
    };
    let restored = restore(&previous, &delta);
    assert_eq!(restored.center_hz, Some(100e6));
    assert_eq!(restored.agc, Some(sdrmm_wire::AgcSetting::switched(true)));
    assert!(restored.gains.is_empty());
    assert_eq!(restored.sample_rate, None);
    assert_eq!(restored.streams[0].center_hz, Some(101e6));
    assert_eq!(restored.streams[0].gains[0].value_db, 20.0);
    assert_eq!(restored.streams[0].tuning, None);
    assert_eq!(restored.streams[1].stream, 3);
    assert_eq!(
        restored.streams[1].center_hz,
        Some(100e6),
        "a lane that followed the radio goes back to the radio center"
    );
    let mut after = previous.clone();
    after.merge_from(&delta);
    after.merge_from(&restored);
    let scope = sdrmm_wire::StreamScope {
        tuning: true,
        gain: true,
        antenna: false,
        agc: true,
    };
    for stream in [0, 1, 3] {
        let (was, is) = (
            previous.for_stream(stream, &scope),
            after.for_stream(stream, &scope),
        );
        assert_eq!(
            (is.center_hz, is.gains, is.agc),
            (was.center_hz, was.gains, was.agc),
            "lane {stream}"
        );
    }
}

fn noise(lanes: usize, seed: &mut u64) -> Vec<Vec<Complex<f32>>> {
    let mut next = || {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    };
    (0..lanes)
        .map(|_| (0..BLOCK).map(|_| Complex::new(next(), next())).collect())
        .collect()
}

#[test]
fn df_yaw_gate_fires_on_derived_rates() {
    let geometry = kraken();
    let frame = LiveFrame {
        sample_rate: RATE,
        center_hz: 433.92e6,
        lane_centers_hz: vec![433.92e6; 5],
        orientation: ArrayOrientation::Heading {
            mount_offset_deg: 0.0,
        },
        tier: Coherence::PhaseCoherent,
        keeps_phase: true,
        needs_time: true,
        tuning: ArrayTuningMode::Together,
        dc_block: false,
        in_flight: 0,
        devices: [0; sdrmm_channels::array_processor::MAX_LANES],
    };
    let shape = ArrayShape {
        positions: geometry.positions(5).expect("kraken positions"),
        geometry,
        manifold: None,
        tuning: ArrayTuningMode::Together,
    };
    let ctx = shape.ctx("df-yaw", &frame);
    let params = ProcessorParams::Df(DfParams {
        report_ms: 100,
        ..DfParams::default()
    });
    let mut df = create_processor(&ctx, &params).expect("the df builds");
    let mut track = PoseTrack::default();
    let mut ring = PoseRing::new();
    for step in 0..=20 {
        let at_ms = step * 100;
        let heading = if at_ms <= 1_000 {
            at_ms as f64 * 0.06
        } else {
            60.0
        };
        let sample = track.sample(Some(&fix(at_ms, heading, None)), host_ns(at_ms));
        ring.push(sample);
    }
    let mut report = ProcessorReading::empty("df");
    let mut events = vec![DecoderEvent::Rds(RdsUpdate::default()); 2];
    let mut readings: Vec<DfReading> = Vec::new();
    let mut seed = 7;
    for index in 0..200i64 {
        let rendered = noise(5, &mut seed);
        let views: Vec<&[Complex<f32>]> = rendered.iter().map(Vec::as_slice).collect();
        let capture_ns = host_ns(0) + index * BLOCK_NS;
        let block = ArrayBlock {
            lanes: &views,
            corrected: true,
            correction: CorrectionView::identity(),
            first_index: index as u64 * BLOCK as u64,
            unix_ns: 1_790_000_000_000_000_000 + (index * BLOCK_NS) as u64,
            generation: 0,
            gap_before: index == 0,
            centers_hz: &frame.lane_centers_hz,
            cal: CalView {
                phase_ready: true,
                gain_ready: true,
                ..CalView::default()
            },
            pose: oriented(ring.at(capture_ns), frame.orientation),
        };
        let mut out = ProcessorOutput::new(OutputSlots {
            report: report.as_mut(),
            surface: None,
            events: &mut events,
            lanes: &mut [],
        });
        df.process(&block, &mut out);
        if out.tally().report
            && let Some(ProcessorReading::Df(reading)) = &report
        {
            readings.push(reading.clone());
        }
    }
    assert_eq!(readings.len(), 20);
    for turning in &readings[..10] {
        assert!(turning.rotating, "{turning:?}");
        assert_eq!(turning.gated_blocks, 10);
    }
    for steady in &readings[10..] {
        assert!(!steady.rotating, "{steady:?}");
        assert_eq!(steady.gated_blocks, 0);
    }
}

#[cfg(feature = "probe")]
fn probe_spec(node: &str) -> ProcessorSpec {
    ProcessorSpec {
        node: node.to_owned(),
        array: ARRAY.to_owned(),
        params: ProcessorParams::Df(DfParams::default()),
        lane_ports: Vec::new(),
        steer_from: None,
    }
}

#[cfg(feature = "probe")]
fn probe_params(lane_ports: u8) -> ProcessorParams {
    ProcessorParams::Probe(sdrmm_wire::processor::ProbeParams {
        lane_ports,
        ..sdrmm_wire::processor::ProbeParams::default()
    })
}

#[cfg(feature = "probe")]
#[test]
fn processor_action_reaches_the_runner() {
    use sdrmm_channels::array_processor::probe::take_probe_log;

    use crate::array::ProcessorAction;

    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let spec = ProcessorSpec {
        params: probe_params(0),
        ..probe_spec("probe-action")
    };
    engine.apply_processor(spec).expect("the probe starts");
    let mut log = take_probe_log("probe-action").expect("the probe logs its blocks");
    wait_for("a block", || log.pop());
    engine
        .processor_action("probe-action", ProcessorAction::ClearTracks)
        .expect("the probe takes actions");
    wait_for("the action", || {
        std::iter::from_fn(|| log.pop()).find(|block| block.actions == 1)
    });
    assert!(matches!(
        engine.processor_action("ghost", ProcessorAction::ClearTracks),
        Err(EngineError::ProcessorNotFound(node)) if node == "ghost"
    ));
    engine
        .apply_processor(probe_spec("df-action"))
        .expect("a df starts");
    assert!(matches!(
        engine.processor_action("df-action", ProcessorAction::ClearTracks),
        Err(EngineError::Processor(text)) if text == "No tracks here"
    ));
    engine.remove_array(ARRAY).expect("the array stops");
    assert!(matches!(
        engine.processor_action("probe-action", ProcessorAction::ClearTracks),
        Err(EngineError::ProcessorNotFound(_))
    ));
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn a_refused_pose_is_an_error_and_counted() {
    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (release, held) = mpsc::channel::<()>();
    engine.lock().arrays[ARRAY]
        .send(Command::Hold(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = held.recv();
        })))
        .expect("the hold is queued");
    entered.recv_timeout(WAIT).expect("the aggregator is held");
    let refused = (0..=COMMAND_SLOTS as i64).find_map(|at| {
        engine
            .update_array_pose(ARRAY, Some(fix(at, 90.0, None)), host_ns(at))
            .err()
    });
    assert!(
        matches!(
            refused,
            Some(EngineError::Array(sdrmm_wire::ArrayFailure::Busy))
        ),
        "{refused:?}"
    );
    let lost = engine.array_statuses()[0].events_lost;
    assert!(lost >= 1, "the refused pose is counted");
    release.send(()).expect("the aggregator lets go");
    wait_for("room for poses", || {
        engine
            .update_array_pose(ARRAY, Some(fix(100, 90.0, None)), host_ns(100))
            .ok()
    });
    engine
        .update_array_pose(ARRAY, None, host_ns(200))
        .expect("an unwired position is a pose too");
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn an_applied_array_holds_its_lanes_and_serves_lane_outputs() {
    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let spec = ProcessorSpec {
        params: probe_params(1),
        lane_ports: vec!["out".to_owned()],
        ..probe_spec("probe-lanes")
    };
    engine
        .apply_processor(spec.clone())
        .expect("the probe starts");
    let set = engine.snapshot().device_sets.remove(0);
    assert_eq!(set.held.len(), 5);
    assert!(set.held.iter().all(|lane| lane.array == ARRAY));
    let [lane] = set.virtual_lanes.as_slice() else {
        panic!("one lane output: {:?}", set.virtual_lanes);
    };
    assert_eq!(
        (lane.stream, lane.node.as_str(), lane.port.as_str()),
        (5, "probe-lanes", "out")
    );
    let channel = engine
        .add_channel(ds, lane.stream, nfm_at(lane.center_hz))
        .expect("a channel on the lane output");
    let status = &engine.array_statuses()[0];
    assert_eq!(status.anchor, Some(ds));
    assert_eq!(status.processors[0].node, "probe-lanes");
    assert!(status.processors[0].running);
    assert_eq!(
        engine.snapshot().arrays.len(),
        1,
        "the snapshot carries the array"
    );
    engine
        .apply_processor(ProcessorSpec {
            params: probe_params(0),
            lane_ports: Vec::new(),
            ..spec
        })
        .expect("the lane output is unwired");
    let set = engine.snapshot().device_sets.remove(0);
    assert!(set.virtual_lanes.is_empty());
    assert!(set.channels.iter().all(|info| info.id != channel));
    engine.remove_array(ARRAY).expect("the array stops");
    let set = engine.snapshot().device_sets.remove(0);
    assert!(set.held.is_empty());
    assert!(engine.array_statuses().is_empty());
    engine.remove_device_set(ds).expect("closes");
}

fn held_lane_center(engine: &Engine, ds: u32, stream: u32) -> Option<f64> {
    let set = engine.snapshot().device_sets.remove(0);
    assert_eq!(set.id, ds);
    set.settings
        .for_stream(stream, &set.capabilities.per_stream)
        .center_hz
}

#[test]
fn tuning_an_array_moves_every_held_lane() {
    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    engine
        .tune_array(
            ARRAY,
            ArrayTuneRequest {
                center_hz: Some(433.92e6),
                gain: Some(ArrayGain::Manual { db: 20.0 }),
            },
        )
        .expect("the array tunes");
    for stream in 0..5 {
        assert_eq!(held_lane_center(&engine, ds, stream), Some(433.92e6));
    }
    let status = engine.array_statuses().remove(0);
    assert_eq!(status.center_hz, 433.92e6);
    assert_eq!(status.gain, ArrayGain::Manual { db: 20.0 });
    assert_eq!(status.gain_db, Some(20.0));
    assert!(status.gain_range_db.is_some());
    assert_eq!(
        refusal(engine.tune_array(
            ARRAY,
            ArrayTuneRequest {
                gain: Some(ArrayGain::Manual { db: 500.0 }),
                ..ArrayTuneRequest::default()
            },
        )),
        "Gain out of range"
    );
    let moved = DeviceSettings {
        streams: vec![StreamSettings {
            stream: 2,
            center_hz: Some(100e6),
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };
    assert_eq!(
        refusal(engine.patch_device(ds, moved.clone())),
        "Tuned by array-1"
    );
    let mut uncalibrated = kraken_spec(ds);
    uncalibrated.settings.cal.source = sdrmm_wire::ArrayCalSource::Off;
    engine
        .apply_array(uncalibrated)
        .expect("the cal source changes in place");
    assert_eq!(
        refusal(engine.tune_array(
            ARRAY,
            ArrayTuneRequest {
                gain: Some(ArrayGain::Auto),
                ..ArrayTuneRequest::default()
            },
        )),
        "Auto gain needs cal"
    );
    assert_eq!(refusal(engine.calibrate_array(ARRAY)), "Cal is off");
    engine.remove_array(ARRAY).expect("the array stops");
    engine
        .patch_device(ds, moved)
        .expect("a released radio tunes again");
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn a_rate_change_keeps_the_array_and_rebuilds_its_processors() {
    use sdrmm_channels::array_processor::probe::take_probe_log;

    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let spec = ProcessorSpec {
        params: probe_params(1),
        lane_ports: vec!["out".to_owned()],
        ..probe_spec("probe-rate")
    };
    engine.apply_processor(spec).expect("the probe starts");
    let first = take_probe_log("probe-rate").expect("the probe logs");
    drop(first);
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect("a rate change passes");
    let status = engine.array_statuses().remove(0);
    assert_eq!(status.sample_rate, 1_024_000.0);
    assert!(status.processors[0].running);
    let set = engine.snapshot().device_sets.remove(0);
    let [lane] = set.virtual_lanes.as_slice() else {
        panic!("one lane output: {:?}", set.virtual_lanes);
    };
    assert_eq!((lane.stream, lane.sample_rate), (5, 1_024_000.0));
    let mut rebuilt = take_probe_log("probe-rate").expect("the rebuilt probe logs");
    let block = wait_for("a block at the new rate", || rebuilt.pop());
    assert_eq!(block.lanes, 5);
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn replugging_the_anchor_keeps_the_array_and_its_lane_outputs() {
    use sdrmm_channels::array_processor::probe::take_probe_log;
    use sdrmm_wire::ArrayFailure;

    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let spec = ProcessorSpec {
        params: probe_params(1),
        lane_ports: vec!["out".to_owned()],
        ..probe_spec("probe-replug")
    };
    engine.apply_processor(spec).expect("the probe starts");
    let mut log = take_probe_log("probe-replug").expect("the probe logs");
    let lane = engine.snapshot().device_sets[0].virtual_lanes[0].clone();
    let channel = engine
        .add_channel(ds, lane.stream, nfm_at(lane.center_hz))
        .expect("a channel on the lane output");
    engine.mark_device_fault(ds, sdrmm_device::DeviceError::Io("pulled".to_owned()));
    assert_eq!(
        engine.array_statuses()[0].failure,
        Some(ArrayFailure::DeviceDown { lane: 0 })
    );
    engine.reconnect(ds);
    let status = engine.array_statuses().remove(0);
    assert!(
        !matches!(status.failure, Some(ArrayFailure::DeviceDown { .. })),
        "{:?}",
        status.failure
    );
    let set = engine.snapshot().device_sets.remove(0);
    assert_eq!(set.virtual_lanes, vec![lane.clone()]);
    assert!(
        set.channels
            .iter()
            .any(|info| info.id == channel && info.stream == lane.stream)
    );
    while log.pop().is_some() {}
    wait_for("a block after the replug", || log.pop());
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn a_rewired_lane_swaps_in_place() {
    use sdrmm_channels::array_processor::probe::take_probe_log;

    let (engine, ds) = bench();
    let mut spec = kraken_spec(ds);
    spec.lanes = lanes(ds, 0..4);
    engine.apply_array(spec.clone()).expect("the array starts");
    engine
        .apply_processor(ProcessorSpec {
            params: probe_params(0),
            ..probe_spec("probe-slot")
        })
        .expect("the probe starts");
    let mut log = take_probe_log("probe-slot").expect("the probe logs");
    spec.lanes = lanes(ds, [0, 1, 2, 4]);
    engine.apply_array(spec.clone()).expect("a lane rewires");
    let held: Vec<u32> = engine.snapshot().device_sets[0]
        .held
        .iter()
        .map(|lane| lane.stream)
        .collect();
    assert_eq!(held, vec![0, 1, 2, 4]);
    assert_eq!(engine.array_statuses()[0].lanes[3].stream, 4);
    assert!(
        take_probe_log("probe-slot").is_none(),
        "a rewired lane keeps the processors"
    );
    while log.pop().is_some() {}
    wait_for("a block after the rewire", || log.pop());
    spec.lanes = lanes(ds, 0..5);
    engine
        .apply_array(spec)
        .expect("a fifth lane restarts the array");
    assert!(
        take_probe_log("probe-slot").is_some(),
        "a new lane count rebuilds the processors"
    );
    assert_eq!(engine.snapshot().device_sets[0].held.len(), 5);
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn an_array_recording_lands_as_a_collection() {
    let dir = tempfile::TempDir::new().expect("scratch");
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine
        .create_device_set("virtual:kraken5")
        .expect("the bench kraken opens");
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let named = |name: &str| ArrayRecordingRequest {
        name: Some(name.to_owned()),
    };
    assert!(matches!(
        engine.start_array_recording(ARRAY, named("a/b")),
        Err(EngineError::Recording(text)) if text == "Bad name"
    ));
    let stem = engine
        .start_array_recording(ARRAY, named("bank"))
        .expect("the array records");
    assert_eq!(stem, "bank");
    let recording = engine.array_statuses()[0]
        .recording
        .clone()
        .expect("the status shows the recording");
    assert_eq!(recording.stem, "bank");
    assert!(matches!(
        engine.start_array_recording(ARRAY, named("bank")),
        Err(EngineError::Recording(_))
    ));
    engine
        .stop_array_recording(ARRAY)
        .expect("the recording stops");
    assert!(engine.array_statuses()[0].recording.is_none());
    assert!(matches!(
        engine.stop_array_recording(ARRAY),
        Err(EngineError::Recording(_))
    ));
    let again = engine
        .start_array_recording(ARRAY, named("bank"))
        .expect("a second take records");
    assert_eq!(again, "bank-2", "a taken name gets a suffix");
    engine
        .remove_array(ARRAY)
        .expect("removing the array ends the take");
    let files: Vec<String> = std::fs::read_dir(dir.path())
        .expect("the recordings folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for stem in ["bank.", "bank-2."] {
        assert!(
            files.iter().any(|file| file.starts_with(stem)),
            "{stem} in {files:?}"
        );
    }
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn removing_an_array_never_blocks_the_engine() {
    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    let slow = ProcessorParams::Probe(sdrmm_wire::processor::ProbeParams {
        block_drop_ms: 500,
        ..sdrmm_wire::processor::ProbeParams::default()
    });
    engine
        .apply_processor(ProcessorSpec {
            params: slow,
            ..probe_spec("probe-slow")
        })
        .expect("the slow probe starts");
    let remover = {
        let engine = engine.clone();
        std::thread::spawn(move || engine.remove_array(ARRAY))
    };
    wait_for("the array to leave the engine", || {
        engine.array_statuses().is_empty().then_some(())
    });
    let started = Instant::now();
    let snapshot = engine.snapshot();
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "a snapshot waited {:?} on a dropping processor",
        started.elapsed()
    );
    assert!(snapshot.arrays.is_empty());
    assert!(
        !remover.is_finished(),
        "the slow drop is still running on the caller"
    );
    remover
        .join()
        .expect("the remover returns")
        .expect("the array stops");
    assert!(engine.snapshot().device_sets[0].held.is_empty());
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn a_failed_processor_build_leaves_the_array_running() {
    let (engine, ds) = bench();
    engine
        .apply_array(kraken_spec(ds))
        .expect("the array starts");
    engine
        .apply_processor(ProcessorSpec {
            params: probe_params(0),
            ..probe_spec("probe-ok")
        })
        .expect("the probe starts");
    let refused = engine.apply_processor(ProcessorSpec {
        params: probe_params(1),
        lane_ports: vec!["nope".to_owned()],
        ..probe_spec("probe-bad")
    });
    assert_eq!(refusal(refused), "No port nope");
    let status = engine.array_statuses().remove(0);
    let listed = |node: &str| {
        status
            .processors
            .iter()
            .find(|processor| processor.node == node)
            .cloned()
            .expect("the processor is listed")
    };
    let bad = listed("probe-bad");
    assert!(!bad.running);
    assert_eq!(bad.error.as_deref(), Some("No port nope"));
    assert!(
        listed("probe-ok").running,
        "the other processor keeps running"
    );
    assert!(
        !matches!(status.failure, Some(ArrayFailure::Stopped { .. })),
        "{:?}",
        status.failure
    );
    assert!(matches!(
        engine.processor_action("probe-bad", crate::array::ProcessorAction::ClearTracks),
        Err(EngineError::Processor(text)) if text == "No port nope"
    ));
    engine.retain_processors(&["probe-ok".to_owned()]);
    let nodes: Vec<String> = engine.array_statuses()[0]
        .processors
        .iter()
        .map(|processor| processor.node.clone())
        .collect();
    assert_eq!(nodes, vec!["probe-ok".to_owned()]);
    engine.retain_arrays(&[ARRAY.to_owned()]);
    assert_eq!(engine.array_statuses().len(), 1);
    engine.retain_arrays(&[]);
    assert!(engine.array_statuses().is_empty());
    assert!(engine.snapshot().device_sets[0].held.is_empty());
    assert!(matches!(
        engine.processor_action("probe-ok", crate::array::ProcessorAction::ClearTracks),
        Err(EngineError::ProcessorNotFound(_))
    ));
    engine.remove_device_set(ds).expect("closes");
}

#[cfg(feature = "probe")]
#[test]
fn a_tuning_mode_change_spreads_the_lanes_and_rebuilds_the_processors() {
    use sdrmm_channels::array_processor::probe::take_probe_log;

    let (engine, ds) = bench();
    let mut spec = kraken_spec(ds);
    spec.tune = Some(ArrayTune {
        center_hz: 433.92e6,
        gain: ArrayGain::Manual { db: 20.0 },
    });
    engine.apply_array(spec.clone()).expect("the array starts");
    engine
        .apply_processor(ProcessorSpec {
            params: probe_params(0),
            ..probe_spec("probe-mode")
        })
        .expect("the probe starts");
    take_probe_log("probe-mode").expect("the probe logs");
    spec.settings.tuning = ArrayTuningMode::Spread;
    engine.apply_array(spec).expect("the lanes spread in place");
    let status = engine.array_statuses().remove(0);
    assert_eq!(status.tuning, ArrayTuningMode::Spread);
    let centers: Vec<f64> = (0..5)
        .filter_map(|stream| held_lane_center(&engine, ds, stream))
        .collect();
    let offsets = sdrmm_dsp::stitch::auto_offsets(5, status.sample_rate);
    assert_eq!(centers.len(), 5);
    assert!(offsets[0] < 0.0 && offsets[4] > 0.0);
    for (center, offset) in centers.iter().zip(&offsets) {
        assert!((center - (433.92e6 + offset)).abs() < 1.0, "{centers:?}");
    }
    assert!(
        take_probe_log("probe-mode").is_some(),
        "a tuning mode change rebuilds the processors"
    );
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}

#[test]
fn the_controller_reads_the_array_through_its_sync_context() {
    let (engine, ds) = bench();
    engine
        .apply_array(ArraySpec {
            tune: Some(ArrayTune {
                center_hz: 433.92e6,
                gain: ArrayGain::Manual { db: 20.0 },
            }),
            ..kraken_spec(ds)
        })
        .expect("the array starts");
    engine
        .tune_array(
            ARRAY,
            ArrayTuneRequest {
                gain: Some(ArrayGain::Auto),
                ..ArrayTuneRequest::default()
            },
        )
        .expect("auto gain takes over");
    let status = engine.array_statuses().remove(0);
    assert_eq!(status.gain, ArrayGain::Auto);
    assert_eq!(status.gain_db, Some(20.0), "auto starts from the live gain");
    let context = engine.sync_context(ARRAY).expect("the array is known");
    assert_eq!(context.center_hz, 433.92e6);
    assert_eq!(context.gain_db, Some(20.0));
    assert!(context.gain_steps_db.contains(&20.0));
    assert_eq!(context.positions.len(), 5);
    assert_eq!(context.azimuth_deg, Some(0.0));
    let device = engine.snapshot().device_sets[0].device.id();
    assert!(
        context
            .lanes
            .iter()
            .enumerate()
            .all(|(stream, lane)| lane.device == device && lane.stream == stream as u32)
    );
    assert!(matches!(
        engine.sync_context("ghost"),
        Err(EngineError::ArrayNotFound(_))
    ));
    let tier = status.tier;
    assert_ne!(tier, Coherence::None);
    engine
        .clock_drift(ARRAY, Some(2.0))
        .expect("drift reaches the tier");
    assert_eq!(engine.array_statuses()[0].tier, Coherence::None);
    engine.clock_drift(ARRAY, None).expect("drift clears");
    assert_eq!(engine.array_statuses()[0].tier, tier);
    engine.remove_array(ARRAY).expect("the array stops");
    engine
        .apply_array(ArraySpec {
            tune: Some(ArrayTune {
                center_hz: 433.92e6,
                gain: ArrayGain::Auto,
            }),
            ..kraken_spec(ds)
        })
        .expect("an auto gain array starts");
    assert_eq!(
        engine.array_statuses()[0].gain_db,
        Some(20.0),
        "a fresh auto array starts from the radio gain"
    );
    engine.remove_array(ARRAY).expect("the array stops");
    engine.remove_device_set(ds).expect("closes");
}
