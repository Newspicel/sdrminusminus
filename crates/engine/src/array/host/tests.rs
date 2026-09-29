use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, Execution, Pose,
    ProcessorAction, ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput,
    ResetCause, Steer, TuningNeed, no_lane_format,
};
use sdrmm_wire::{
    ArrayGeometry, ArrayOrientation, ArrayTuningMode, Coherence, DecodedRecord, DecoderEvent,
    DfParams, DfPeak, NO_CHANNEL, ProcessorGate, ProcessorParams, ProcessorReading,
    processor::df::{DF_POINTS, MAX_DF_PEAKS},
};
use tokio::sync::broadcast;

use super::*;
use crate::array::{LiveFrame, correct::CorrectionSet};

pub(crate) const LANES: usize = 2;
pub(crate) const RATE: f64 = 48_000.0;

pub(crate) static TALKER: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "df",
    name: "Talker",
    min_lanes: 1,
    max_lanes: 16,
    lane_ports: &[],
    steer_port: Some("steer"),
    surface: None,
    needs: |_| ProcessorNeeds::default(),
    band: |_| None,
    tuning: |_| TuningNeed::Any,
    lane_format: no_lane_format,
    execution: |_, _| Execution::Inline,
    in_place: |_, _| true,
};

pub(crate) static BANDED: ProcessorDescriptor = ProcessorDescriptor {
    needs: |_| ProcessorNeeds::TIME,
    band: |_| Some((0.0, 1_000.0)),
    ..TALKER
};

pub(crate) static WIDE: ProcessorDescriptor = ProcessorDescriptor {
    needs: |_| ProcessorNeeds::TIME,
    ..TALKER
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Heard {
    pub(crate) first_index: u64,
    pub(crate) len: usize,
    pub(crate) corrected: bool,
    pub(crate) gap_before: bool,
    pub(crate) steered: Option<Steer>,
    pub(crate) heading: Option<f64>,
    pub(crate) first: [Complex<f32>; 4],
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Talk {
    pub(crate) report: bool,
    pub(crate) peaks: usize,
    pub(crate) event: bool,
    pub(crate) steer: Option<f64>,
    pub(crate) refuse_retune: bool,
}

pub(crate) struct Talker {
    talk: Talk,
    faults: ProcessorFaults,
    heard: Producer<Heard>,
    steered: Option<Steer>,
    pause: Option<Arc<Mutex<()>>>,
}

impl Talker {
    pub(crate) fn new(talk: Talk) -> (Box<Self>, Consumer<Heard>) {
        let (heard, log) = RingBuffer::new(4_096);
        (
            Box::new(Self {
                talk,
                faults: ProcessorFaults::default(),
                heard,
                steered: None,
                pause: None,
            }),
            log,
        )
    }

    pub(crate) fn paused(talk: Talk, pause: Arc<Mutex<()>>) -> (Box<Self>, Consumer<Heard>) {
        let (mut talker, log) = Self::new(talk);
        talker.pause = Some(pause);
        (talker, log)
    }

    fn fill(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if self.talk.report
            && let Some(ProcessorReading::Df(reading)) = out.report()
        {
            reading.peaks.clear();
            for peak in 0..self.talk.peaks {
                self.faults.push_capped(
                    &mut reading.peaks,
                    DfPeak {
                        relative_deg: peak as f32,
                        ..DfPeak::default()
                    },
                );
            }
            reading.pseudospectrum.clear();
            reading
                .pseudospectrum
                .extend((0..DF_POINTS).map(|at| at as u8));
            reading.likelihood.clear();
            reading.likelihood.extend((0..DF_POINTS).map(|at| at as u8));
            reading.eigenvalues_db.clear();
            reading.eigenvalues_db.extend((0..16).map(|at| at as f32));
            sdrmm_channels::array_processor::stamp_at(&mut reading.at, block.unix_ns);
            out.publish_report();
        }
        if self.talk.event
            && let Some(DecoderEvent::Df(bearing)) = out.event()
        {
            bearing.bearing_deg = 42.0;
            bearing.freq_hz = Some(433.92e6);
            bearing.node.clear();
            bearing.node.push_str("talker");
            out.publish_event();
        }
        if let Some(relative_deg) = self.talk.steer {
            out.steer(Steer {
                relative_deg,
                true_deg: Some(relative_deg + 90.0),
                sigma_deg: 2.0,
                ..Steer::default()
            });
        }
    }
}

impl ArrayProcessor for Talker {
    fn descriptor() -> &'static ProcessorDescriptor {
        &TALKER
    }

    fn new(_ctx: &ArrayCtx<'_>, _params: &ProcessorParams) -> Result<Self, ChannelError> {
        Err(ChannelError::Refused("built by the test"))
    }

    fn apply(&mut self, _params: &ProcessorParams) -> Result<(), ChannelError> {
        Ok(())
    }

    fn retune(&mut self, _ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        if self.talk.refuse_retune {
            Err(ChannelError::Refused("Array changed"))
        } else {
            Ok(())
        }
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.faults.resets += 1;
    }

    fn steer(&mut self, steer: &Steer) {
        self.steered = Some(*steer);
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Ok(())
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if let Some(pause) = &self.pause {
            drop(pause.lock().unwrap_or_else(PoisonError::into_inner));
        }
        let mut first = [Complex::default(); 4];
        for (slot, lane) in first.iter_mut().zip(block.lanes) {
            *slot = lane.first().copied().unwrap_or_default();
        }
        let _ = self.heard.push(Heard {
            first_index: block.first_index,
            len: block.len(),
            corrected: block.corrected,
            gap_before: block.gap_before,
            steered: self.steered,
            heading: block.pose.heading_deg,
            first,
        });
        self.fill(block, out);
    }

    fn faults(&self) -> ProcessorFaults {
        self.faults
    }
}

pub(crate) fn frame(lanes: usize) -> LiveFrame {
    LiveFrame {
        sample_rate: RATE,
        center_hz: 100e6,
        lane_centers_hz: vec![100e6; lanes],
        orientation: ArrayOrientation::Fixed { azimuth_deg: 0.0 },
        tier: Coherence::TimeSync,
        keeps_phase: false,
        needs_time: false,
        tuning: ArrayTuningMode::Together,
        dc_block: false,
        in_flight: 0,
        devices: [0; 16],
    }
}

pub(crate) struct Taps {
    pub(crate) sinks: HostSinks,
    pub(crate) events: broadcast::Receiver<ArrayEvent>,
    pub(crate) decoded: broadcast::Receiver<DecodedRecord>,
}

pub(crate) fn taps(anchor: u32) -> Taps {
    let (events_tx, events) = broadcast::channel(256);
    let (decoded_tx, decoded) = broadcast::channel(256);
    Taps {
        sinks: HostSinks {
            events: events_tx,
            decoded: decoded_tx,
            decoded_lost: Arc::new(AtomicU64::new(0)),
            anchor,
        },
        events,
        decoded,
    }
}

pub(crate) fn plan(
    node: &str,
    params: ProcessorParams,
    sinks: &HostSinks,
    lanes: usize,
) -> HostPlan {
    let geometry = ArrayGeometry::default();
    HostPlan {
        node: node.to_owned(),
        params,
        shape: ArrayShape {
            positions: geometry.positions(lanes).unwrap_or_default(),
            geometry,
            manifold: None,
            tuning: ArrayTuningMode::Together,
        },
        sinks: Vec::new(),
        outputs: sinks.clone(),
        steer_in: SteerInput::None,
        steer_out: Arc::new(SteerMailbox::default()),
        stats: Arc::new(ProcessorStats::default()),
        gpu: GpuUse::Off,
    }
}

pub(crate) fn talker_host(
    node: &str,
    talk: Talk,
    sinks: &HostSinks,
) -> (Box<ProcessorHost>, Consumer<Heard>) {
    let (talker, log) = Talker::new(talk);
    let host = ProcessorHost::with_processor(
        plan(node, ProcessorParams::Df(DfParams::default()), sinks, LANES),
        &frame(LANES),
        &TALKER,
        talker,
    )
    .expect("host");
    (host, log)
}

pub(crate) fn open() -> GateInputs {
    GateInputs {
        sample_rate: RATE,
        window: None,
        tier: Coherence::TimeSync,
        tuning: ArrayTuningMode::Together,
        synced: true,
        phase_ready: true,
        gain_ready: true,
    }
}

fn run(hosts: &mut HostList, first_index: u64, samples: &[Complex<f32>]) {
    let lanes = [samples, samples];
    let block = ArrayBlock {
        lanes: &lanes,
        corrected: false,
        correction: CorrectionView::identity(),
        first_index,
        unix_ns: 1_700_000_000_000_000_000,
        generation: 0,
        gap_before: false,
        centers_hz: &[100e6, 100e6],
        cal: CalView::default(),
        pose: Pose::default(),
    };
    hosts.process(&block, &open(), &CorrectionSet::identity(LANES));
}

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_steer_survives_the_mailbox() {
    let mailbox = SteerMailbox::default();
    let mut seen = 0;
    assert_eq!(mailbox.read(&mut seen), None);
    let steer = Steer {
        same_array: true,
        relative_deg: 12.5,
        true_deg: Some(102.5),
        elevation_deg: 3.0,
        sigma_deg: 1.5,
        others_relative_deg: [1.0, 2.0, 3.0],
        others_true_deg: [Some(4.0), None, Some(6.0)],
        others: 2,
        wall_ms: 99,
    };
    mailbox.post(&steer);
    let read = mailbox.read(&mut seen).expect("posted");
    assert_eq!(
        read,
        Steer {
            same_array: false,
            ..steer
        }
    );
    assert_eq!(mailbox.read(&mut seen), None);
}

#[test]
fn processor_events_carry_their_origin_node() {
    let mut taps = taps(7);
    let (host, _log) = talker_host(
        "df-1",
        Talk {
            event: true,
            ..Talk::default()
        },
        &taps.sinks,
    );
    let mut hosts = HostList::new();
    assert!(hosts.add(host).is_ok());
    run(&mut hosts, 0, &[Complex::new(0.1, 0.0); 256]);
    let record = wait_for(|| taps.decoded.try_recv().ok());
    assert_eq!(
        record.origin,
        Some(sdrmm_wire::EventOrigin {
            node: "df-1".to_owned(),
            transmission: 0
        })
    );
    assert_eq!(record.device_set, 7);
    assert_eq!(record.channel, NO_CHANNEL);
    assert_eq!(record.freq_hz, 433.92e6);
    assert!(matches!(record.event, DecoderEvent::Df(bearing) if bearing.bearing_deg == 42.0));
}

#[test]
fn a_full_report_pool_counts_dropped_reports() {
    let taps = taps(0);
    let (host, _log) = talker_host(
        "df-full",
        Talk {
            report: true,
            ..Talk::default()
        },
        &taps.sinks,
    );
    let stats = host.stats().clone();
    let mut hosts = HostList::new();
    assert!(hosts.add(host).is_ok());
    {
        let _paused = PUBLISH_GATE.write().unwrap_or_else(PoisonError::into_inner);
        for block in 0..20 {
            run(&mut hosts, block * 256, &[Complex::new(0.1, 0.0); 256]);
        }
        assert_eq!(stats.dropped_reports.load(Ordering::Relaxed), 4);
    }
    let status = stats.status("df-full", "df", None);
    assert_eq!(status.dropped_reports, 4);
}

#[test]
fn a_reading_past_the_wire_limit_is_truncated_and_counted() {
    let mut taps = taps(0);
    let extra = 3;
    let (host, _log) = talker_host(
        "df-long",
        Talk {
            report: true,
            peaks: usize::from(MAX_DF_PEAKS) + extra,
            ..Talk::default()
        },
        &taps.sinks,
    );
    let stats = host.stats().clone();
    let mut hosts = HostList::new();
    assert!(hosts.add(host).is_ok());
    run(&mut hosts, 0, &[Complex::new(0.1, 0.0); 256]);
    let event = wait_for(|| taps.events.try_recv().ok());
    let ArrayEvent::Report { processor, reading } = event else {
        panic!("a report");
    };
    assert_eq!(processor, "df-long");
    let ProcessorReading::Df(reading) = reading.as_ref() else {
        panic!("a df reading");
    };
    assert_eq!(reading.peaks.len(), usize::from(MAX_DF_PEAKS));
    assert_eq!(stats.truncated.load(Ordering::Relaxed), extra as u64);
    assert_eq!(stats.status("df-long", "df", None).truncated, extra as u64);
}

#[test]
fn a_steer_reaches_the_beamformer_before_the_next_block() {
    let taps = taps(0);
    let (mut beam, mut beam_log) = talker_host("beam", Talk::default(), &taps.sinks);
    beam.steer_in = SteerInput::Local("df".to_owned());
    let (df, _df_log) = talker_host(
        "df",
        Talk {
            steer: Some(30.0),
            ..Talk::default()
        },
        &taps.sinks,
    );
    let mut hosts = HostList::new();
    assert!(hosts.add(beam).is_ok());
    assert!(hosts.add(df).is_ok());
    let samples = [Complex::new(0.1, 0.0); 128];
    run(&mut hosts, 0, &samples);
    run(&mut hosts, 128, &samples);
    let first = beam_log.pop().expect("first block");
    let second = beam_log.pop().expect("second block");
    assert_eq!(first.steered, None);
    let steer = second.steered.expect("steered before the second block");
    assert!(steer.same_array);
    assert_eq!(steer.relative_deg, 30.0);
}

#[test]
fn a_steer_from_a_batched_direction_finder_reaches_the_beamformer() {
    let taps = taps(0);
    let (mut beam, mut beam_log) = talker_host("beam", Talk::default(), &taps.sinks);
    beam.steer_in = SteerInput::Local("df".to_owned());
    let (talker, _df_log) = Talker::new(Talk {
        steer: Some(40.0),
        ..Talk::default()
    });
    let df = ProcessorHost::batched(
        plan(
            "df",
            ProcessorParams::Df(DfParams::default()),
            &taps.sinks,
            LANES,
        ),
        &frame(LANES),
        &TALKER,
        talker,
        64,
    )
    .expect("host");
    let mut hosts = HostList::new();
    assert!(hosts.add(df).is_ok());
    assert!(hosts.add(beam).is_ok());
    let samples = [Complex::new(0.1, 0.0); 64];
    let mut first_index = 0;
    let steer = wait_for(|| {
        run(&mut hosts, first_index, &samples);
        first_index += 64;
        std::iter::from_fn(|| beam_log.pop().ok()).find_map(|heard| heard.steered)
    });
    assert!(steer.same_array);
    assert_eq!(steer.relative_deg, 40.0);
}

#[test]
fn a_steer_from_another_array_arrives_through_the_mailbox() {
    let taps = taps(0);
    let mailbox = Arc::new(SteerMailbox::default());
    let (mut beam, mut log) = talker_host("beam", Talk::default(), &taps.sinks);
    beam.steer_in = SteerInput::Remote(mailbox.clone());
    let mut hosts = HostList::new();
    assert!(hosts.add(beam).is_ok());
    mailbox.post(&Steer {
        relative_deg: 50.0,
        true_deg: None,
        ..Steer::default()
    });
    run(&mut hosts, 0, &[Complex::new(0.1, 0.0); 64]);
    assert_eq!(log.pop().expect("a block").steered, None);
    mailbox.post(&Steer {
        same_array: true,
        relative_deg: 10.0,
        true_deg: Some(100.0),
        ..Steer::default()
    });
    run(&mut hosts, 0, &[Complex::new(0.1, 0.0); 64]);
    let heard = log.pop().expect("a block");
    let steer = heard.steered.expect("steered");
    assert!(!steer.same_array);
    assert_eq!(steer.true_deg, Some(100.0));
}

#[test]
fn gates_follow_the_contract_order() {
    let taps = taps(0);
    let (talker, _log) = Talker::new(Talk::default());
    let host = ProcessorHost::with_processor(
        plan(
            "wide",
            ProcessorParams::Df(DfParams::default()),
            &taps.sinks,
            LANES,
        ),
        &frame(LANES),
        &WIDE,
        talker,
    )
    .expect("host");
    let inputs = GateInputs {
        synced: false,
        ..open()
    };
    assert_eq!(host.gate_for(&inputs), Some(ProcessorGate::Sync));
    assert_eq!(
        host.gate_for(&GateInputs {
            window: Some(ProcessorGate::Calibrating),
            ..inputs
        }),
        Some(ProcessorGate::Calibrating)
    );
    assert_eq!(
        host.gate_for(&GateInputs {
            tier: Coherence::None,
            ..inputs
        }),
        Some(ProcessorGate::Tier)
    );
    assert_eq!(host.gate_for(&open()), None);
    assert!(host.needs_corrector());
}

#[test]
fn a_batched_host_hands_full_batches_to_its_worker() {
    let taps = taps(0);
    let (talker, mut log) = Talker::new(Talk::default());
    let mut host = ProcessorHost::batched(
        plan(
            "batch",
            ProcessorParams::Df(DfParams::default()),
            &taps.sinks,
            LANES,
        ),
        &frame(LANES),
        &TALKER,
        talker,
        1_000,
    )
    .expect("host");
    let samples = [Complex::new(0.2, 0.0); 700];
    let identity = CorrectionSet::identity(LANES);
    for block in 0..4u64 {
        let lanes = [&samples[..], &samples[..]];
        let block = ArrayBlock {
            lanes: &lanes,
            corrected: false,
            correction: CorrectionView::identity(),
            first_index: block * 700,
            unix_ns: 0,
            generation: 0,
            gap_before: false,
            centers_hz: &[100e6, 100e6],
            cal: CalView::default(),
            pose: Pose::default(),
        };
        host.process(&block, &open(), &identity);
    }
    let first = wait_for(|| log.pop().ok());
    let second = wait_for(|| log.pop().ok());
    assert_eq!((first.first_index, first.len), (0, 1_000));
    assert_eq!((second.first_index, second.len), (1_000, 1_000));
}

#[test]
fn a_busy_worker_drops_the_batch_and_counts_it() {
    let taps = taps(0);
    let pause = Arc::new(Mutex::new(()));
    let (talker, _log) = Talker::paused(Talk::default(), pause.clone());
    let held = pause.lock().unwrap_or_else(PoisonError::into_inner);
    let mut host = ProcessorHost::batched(
        plan(
            "busy",
            ProcessorParams::Df(DfParams::default()),
            &taps.sinks,
            LANES,
        ),
        &frame(LANES),
        &TALKER,
        talker,
        100,
    )
    .expect("host");
    let samples = [Complex::new(0.2, 0.0); 100];
    let identity = CorrectionSet::identity(LANES);
    for block in 0..10u64 {
        let lanes = [&samples[..], &samples[..]];
        let block = ArrayBlock {
            lanes: &lanes,
            corrected: false,
            correction: CorrectionView::identity(),
            first_index: block * 100,
            unix_ns: 0,
            generation: 0,
            gap_before: false,
            centers_hz: &[100e6, 100e6],
            cal: CalView::default(),
            pose: Pose::default(),
        };
        host.process(&block, &open(), &identity);
    }
    let dropped = host.stats().dropped_samples.load(Ordering::Relaxed);
    drop(held);
    assert!(dropped >= 500, "dropped {dropped}");
}

#[test]
fn a_passive_radar_host_refuses_a_plan_its_capture_cannot_hold() {
    let taps = taps(0);
    let built = ProcessorHost::build(
        plan(
            "radar",
            ProcessorParams::PassiveRadar(sdrmm_wire::PassiveRadarParams::default()),
            &taps.sinks,
            LANES,
        ),
        &frame(LANES),
    );
    match built {
        Err(EngineError::Channel(ChannelError::Refused(message))) => {
            assert_eq!(message, "Band outside the capture");
        }
        Err(other) => panic!("unexpected refusal {other}"),
        Ok(_) => panic!("FM radar started on a 48 kHz capture"),
    }
}
