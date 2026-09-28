use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::Consumer;
use sdrmm_channels::array_processor::{MAX_LANES, Pose};
use sdrmm_device::{GapScope, LaneEvent, LaneMark};
use sdrmm_test_support::assert_no_alloc;
use sdrmm_wire::{
    ArrayCalSource, ArrayFailure, ArrayOrientation, ArrayStatus, CalPhase, DfParams, ProcessorGate,
    ProcessorParams, SyncState,
};

use super::*;
use crate::array::{
    CommandQueue, LiveFrame,
    capture::{CaptureKind, CaptureRequest, CaptureStart, SolveSummary},
    host::{
        ProcessorHost,
        tests::{BANDED, Heard, TALKER, Talk, Talker, Taps, WIDE, frame, plan, taps},
    },
    tap::{TapPort, TapWriter},
};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 16_384;

struct Rig {
    _ports: Vec<Arc<TapPort>>,
    writers: Vec<TapWriter>,
    aggregator: Aggregator,
    queue: CommandQueue,
    worker: WorkerIo,
    events: Consumer<AggregatorEvent>,
    board: Arc<StatusBoard>,
    frame: LiveFrame,
    index: u64,
    taps: Taps,
}

fn rig(lanes: usize, frame: LiveFrame) -> Rig {
    let (queue, commands) = CommandQueue::new();
    let board = Arc::new(StatusBoard::new(lanes));
    let wiring = wire(lanes, frame.sample_rate, Arc::new(AtomicBool::new(false))).expect("wire");
    let mut ports = Vec::new();
    let mut writers = Vec::new();
    let mut feeds = Vec::new();
    for stream in 0..lanes {
        let (port, writer) = TapPort::new(stream as u32);
        feeds.push(Some(port.lease(frame.sample_rate, 1).expect("lease")));
        ports.push(port);
        writers.push(writer);
    }
    let aggregator = Aggregator::new(
        feeds,
        Box::new(frame.clone()),
        board.clone(),
        commands,
        wiring.aggregator,
        None,
    );
    Rig {
        _ports: ports,
        writers,
        aggregator,
        queue,
        worker: wiring.worker,
        events: wiring.events,
        board,
        frame,
        index: 0,
        taps: taps(3),
    }
}

impl Rig {
    fn write(&mut self, len: usize, sample: impl Fn(usize, u64) -> Complex<f32>) {
        for (lane, writer) in self.writers.iter_mut().enumerate() {
            let block: Vec<Complex<f32>> = (0..len as u64)
                .map(|at| sample(lane, self.index + at))
                .collect();
            writer.samples(&block, self.index);
        }
        self.index += len as u64;
    }

    fn noise(&mut self, len: usize) {
        self.write(len, |lane, at| {
            let phase = (at as f32 * 0.37 + lane as f32).sin();
            Complex::new(0.1 * phase, 0.05 * (at as f32 * 0.11).cos())
        });
    }

    fn settle(&mut self) -> usize {
        let mut worked = 0;
        while self.aggregator.step() == Step::Worked {
            worked += 1;
        }
        worked
    }

    fn send(&mut self, command: Command) {
        self.queue.send(command).expect("queued");
        self.aggregator.step();
    }

    fn talker(
        &mut self,
        node: &str,
        talk: Talk,
        descriptor: &'static ProcessorDescriptorRef,
    ) -> Consumer<Heard> {
        let (talker, log) = Talker::new(talk);
        let host = ProcessorHost::with_processor(
            plan(
                node,
                ProcessorParams::Df(DfParams::default()),
                &self.taps.sinks,
                self.frame.lanes(),
            ),
            &self.frame,
            descriptor,
            talker,
        )
        .expect("host");
        self.send(Command::AddHost { host });
        log
    }

    fn events(&mut self) -> Vec<AggregatorEvent> {
        std::iter::from_fn(|| self.events.pop().ok()).collect()
    }

    fn status(&self) -> ArrayStatus {
        let mut status = ArrayStatus::default();
        self.board.fill(&mut status);
        status
    }
}

type ProcessorDescriptorRef = sdrmm_channels::array_processor::ProcessorDescriptor;

fn drain(log: &mut Consumer<Heard>) -> Vec<Heard> {
    std::iter::from_fn(|| log.pop().ok()).collect()
}

#[test]
fn a_full_command_queue_is_busy() {
    let (queue, _commands) = CommandQueue::new();
    let pose = PoseSample {
        host_ns: 0,
        heading_deg: None,
        heading_sigma_deg: 0.0,
        yaw_rate_dps: None,
        fix: None,
        moving: false,
    };
    for _ in 0..crate::array::COMMAND_SLOTS {
        queue.send(Command::Pose { sample: pose }).expect("room");
    }
    assert!(matches!(
        queue.send(Command::Pose { sample: pose }),
        Err(EngineError::Array(ArrayFailure::Busy))
    ));
}

#[test]
fn pose_ring_interpolates_the_heading_at_capture_time() {
    let mut ring = PoseRing::new();
    let sample = |host_ns: i64, heading: f64, yaw: f32| PoseSample {
        host_ns,
        heading_deg: Some(heading),
        heading_sigma_deg: 2.0,
        yaw_rate_dps: Some(yaw),
        fix: None,
        moving: true,
    };
    ring.push(sample(1_000_000_000, 350.0, 5.0));
    ring.push(sample(2_000_000_000, 10.0, 7.0));
    let middle = ring.at(1_500_000_000);
    assert!(middle.heading_deg.expect("heading").abs() < 1e-9);
    assert_eq!(middle.yaw_rate_dps, Some(7.0));
    let quarter = ring.at(1_250_000_000);
    assert!((quarter.heading_deg.expect("heading") - 355.0).abs() < 1e-9);
    assert_eq!(ring.at(2_500_000_000).heading_deg, Some(10.0));
    assert_eq!(ring.at(4_500_000_000).heading_deg, None);
    assert_eq!(ring.at(0).heading_deg, Some(350.0));
    let mounted = oriented(
        middle,
        ArrayOrientation::Heading {
            mount_offset_deg: 90.0,
        },
    );
    assert!((mounted.heading_deg.expect("heading") - 90.0).abs() < 1e-9);
    assert!(mounted.follows);
    let fixed = oriented(
        Pose::default(),
        ArrayOrientation::Fixed { azimuth_deg: 45.0 },
    );
    assert_eq!(fixed.heading_deg, Some(45.0));
    assert_eq!(fixed.yaw_rate_dps, Some(0.0));
}

#[test]
fn the_block_pose_is_read_at_the_capture_instant() {
    let mut frame = frame(2);
    frame.orientation = ArrayOrientation::Heading {
        mount_offset_deg: 0.0,
    };
    frame.in_flight = RATE as u64;
    let mut rig = rig(2, frame);
    let mut log = rig.talker("df", Talk::default(), &TALKER);
    let now = i64::try_from(sdrmm_device::now_ns()).expect("fits");
    let pose = |host_ns: i64, heading: f64| PoseSample {
        host_ns,
        heading_deg: Some(heading),
        heading_sigma_deg: 1.0,
        yaw_rate_dps: None,
        fix: None,
        moving: false,
    };
    rig.send(Command::Pose {
        sample: pose(now - 1_000_000_000, 10.0),
    });
    rig.send(Command::Pose {
        sample: pose(now + 5_000_000_000, 70.0),
    });
    rig.noise(2 * BLOCK);
    rig.settle();
    let heard = drain(&mut log);
    let heading = heard
        .first()
        .and_then(|block| block.heading)
        .expect("a heading");
    assert!(
        heading < 12.0,
        "a block captured a second ago reads {heading}"
    );
}

#[test]
fn the_corrector_is_skipped_without_a_wideband_host() {
    let mut rig = rig(2, frame(2));
    rig.board.set_sync(SyncState::Locked);
    let mut banded = rig.talker("band", Talk::default(), &BANDED);
    rig.noise(3 * BLOCK);
    rig.settle();
    let heard = drain(&mut banded);
    assert!(!heard.is_empty());
    assert!(heard.iter().all(|block| !block.corrected));
    let mut wide = rig.talker("wide", Talk::default(), &WIDE);
    rig.noise(3 * BLOCK);
    rig.settle();
    let wide_heard = drain(&mut wide);
    let banded_heard = drain(&mut banded);
    assert!(!wide_heard.is_empty());
    assert!(wide_heard.iter().all(|block| block.corrected));
    assert!(banded_heard.iter().all(|block| block.corrected));
    let gated = rig
        .aggregator
        .hosts()
        .find_mut("wide")
        .expect("wide host")
        .stats()
        .gated_samples
        .load(Ordering::Relaxed);
    assert_eq!(gated, crate::array::correct::CORR_TAPS as u64);
}

#[test]
fn dc_blockers_are_identical_on_every_lane() {
    let mut bank = DcBank::new(RATE);
    let signal: Vec<Complex<f32>> = (0..40_000)
        .map(|at| Complex::from_polar(0.1, at as f32 * 0.3))
        .collect();
    let with_dc = |dc: Complex<f32>| signal.iter().map(|x| x + dc).collect::<Vec<_>>();
    let mut lanes = vec![
        with_dc(Complex::new(0.2, -0.1)),
        with_dc(Complex::new(0.2, -0.1)),
        with_dc(Complex::new(-0.05, 0.3)),
    ];
    bank.process(&mut lanes, 40_000);
    assert_eq!(lanes[0], lanes[1]);
    let tail = 30_000..40_000;
    for at in tail {
        assert!((lanes[0][at] - lanes[2][at]).norm() < 1e-3, "sample {at}");
        assert!((lanes[0][at] - signal[at]).norm() < 1e-2, "sample {at}");
    }
}

#[test]
fn the_array_dc_blocker_runs_only_when_a_member_manages_dc() {
    let mut frame = frame(2);
    frame.dc_block = true;
    let mut rig = rig(2, frame);
    let mut log = rig.talker("df", Talk::default(), &TALKER);
    for _ in 0..6 {
        rig.write(BLOCK, |_, _| Complex::new(0.3, 0.2));
        rig.settle();
    }
    let heard = drain(&mut log);
    let last = heard.last().expect("blocks");
    assert!(last.first[0].norm() < 1e-3, "{:?}", last.first[0]);
    assert_eq!(last.first[0], last.first[1]);
}

#[test]
fn step_does_not_allocate_after_warmup() {
    let mut rig = rig(2, frame(2));
    rig.board.set_sync(SyncState::Locked);
    let mut log = rig.talker(
        "df",
        Talk {
            report: true,
            peaks: 8,
            event: true,
            steer: Some(20.0),
        },
        &WIDE,
    );
    let mut beam = rig.talker("beam", Talk::default(), &TALKER);
    #[cfg(feature = "probe")]
    let _lane = probe::two_probes(&mut rig);
    rig.send(Command::Capture {
        request: CaptureRequest {
            id: 1,
            kind: CaptureKind::Coarse,
            start: CaptureStart::Now,
            len: 65_536,
            decimation: 64,
            source: ArrayCalSource::Noise,
            equaliser: false,
        },
    });
    let block: Vec<Complex<f32>> = (0..BLOCK)
        .map(|at| Complex::new((at as f32 * 0.1).sin() * 0.1, 0.02))
        .collect();
    let mut index = 0u64;
    let mut feed = |rig: &mut Rig| {
        for writer in &mut rig.writers {
            writer.samples(&block, index);
        }
        index += BLOCK as u64;
        rig.aggregator.step();
    };
    for _ in 0..20 {
        feed(&mut rig);
    }
    drain(&mut log);
    drain(&mut beam);
    assert_no_alloc("aggregator step", || {
        for _ in 0..200 {
            feed(&mut rig);
        }
    });
    assert!(
        !rig.events()
            .contains(&AggregatorEvent::CaptureRefused { id: 1 })
    );
}

#[test]
fn overflowing_notes_count_and_force_a_realign() {
    let mut rig = rig(2, frame(2));
    let mut log = rig.talker("df", Talk::default(), &TALKER);
    rig.noise(BLOCK);
    rig.settle();
    for _ in 0..70 {
        rig.writers[0].event(LaneEvent::Mark {
            at: rig.index,
            mark: LaneMark::GainChanged { in_flight: 0 },
        });
    }
    rig.noise(2 * BLOCK);
    rig.settle();
    let status = rig.status();
    assert_eq!(status.events_lost, 7);
    assert_eq!(status.sync, SyncState::Lost);
    assert!(status.lanes.iter().all(|lane| lane.sync == SyncState::Lost));
    let events = rig.events();
    for lane in 0..2 {
        assert!(events.contains(&AggregatorEvent::Uncertain {
            lane,
            error: sdrmm_device::UNKNOWN_ERROR,
            scope: GapScope::Device
        }));
    }
    assert!(drain(&mut log).iter().any(|block| block.gap_before));
}

#[test]
fn a_capture_reaches_the_worker_and_its_solution_applies() {
    let mut rig = rig(2, frame(2));
    rig.send(Command::Capture {
        request: CaptureRequest {
            id: 5,
            kind: CaptureKind::Solve,
            start: CaptureStart::Now,
            len: 4_096,
            decimation: 1,
            source: ArrayCalSource::Noise,
            equaliser: false,
        },
    });
    rig.noise(2 * BLOCK);
    rig.settle();
    let job = rig.worker.jobs.pop().expect("a job");
    assert_eq!(job.request.id, 5);
    assert_eq!(job.buffers.lanes.len(), 2);
    assert_eq!(job.buffers.lanes[1].len(), 4_096);
    assert!(rig.events().contains(&AggregatorEvent::Captured { id: 5 }));
    let set = rig.worker.sets.pop().expect("a free set");
    let mut summary = SolveSummary {
        lanes: 2,
        phase_ready: true,
        gain_ready: true,
        ..SolveSummary::default()
    };
    summary.delay[1] = 12.5;
    summary.phase_deg[1] = -30.0;
    let mut offsets = [0; MAX_LANES];
    offsets[1] = 12;
    let _ = rig.worker.solutions.push(Box::new(Solution {
        id: 5,
        offsets: Some(offsets),
        correction: Some(set),
        outcome: Ok(summary),
        quality: CalQuality::default(),
    }));
    let _ = rig.worker.buffers.push(job.buffers);
    let generation = rig.aggregator.generation();
    rig.aggregator.step();
    assert_eq!(rig.aggregator.generation(), generation + 1);
    assert!(
        rig.events()
            .contains(&AggregatorEvent::Solved { id: 5, summary })
    );
    let status = rig.status();
    assert!(status.phase_ready);
    assert!((status.lanes[1].delay_samples - 12.5).abs() < 1e-9);
    assert!(rig.worker.sets.pop().is_ok());
}

#[test]
fn a_solution_whose_correction_does_not_fit_is_a_failed_solve() {
    let mut rig = rig(2, frame(2));
    let summary = SolveSummary {
        lanes: 2,
        phase_ready: true,
        gain_ready: true,
        ..SolveSummary::default()
    };
    let _ = rig.worker.solutions.push(Box::new(Solution {
        id: 9,
        offsets: None,
        correction: Some(Box::new(crate::array::CorrectionSet::identity(3))),
        outcome: Ok(summary),
        quality: CalQuality::default(),
    }));
    let generation = rig.aggregator.generation();
    rig.aggregator.step();
    assert_eq!(rig.aggregator.generation(), generation);
    assert_eq!(
        rig.events(),
        [AggregatorEvent::SolveFailed {
            id: 9,
            failure: crate::array::SolveFailure::Refused
        }]
    );
    assert!(!rig.status().phase_ready);
}

#[test]
fn a_retune_mark_makes_a_solved_phase_stale() {
    let mut rig = rig(2, frame(2));
    rig.aggregator.set_ready(true, true);
    rig.board.set_cal(CalPhase::Solved);
    rig.noise(BLOCK);
    rig.settle();
    rig.writers[0].event(LaneEvent::Mark {
        at: rig.index,
        mark: LaneMark::GainChanged { in_flight: 100 },
    });
    rig.noise(2 * BLOCK);
    rig.settle();
    let status = rig.status();
    assert_eq!(status.cal, CalPhase::Stale);
    assert!(!status.phase_ready);
    assert!(rig.events().iter().any(|event| matches!(
        event,
        AggregatorEvent::BlankEnded {
            cause: crate::array::BlankCause::Gain,
            ..
        }
    )));
}

#[test]
fn a_new_rate_holds_every_host_until_it_is_rebuilt() {
    let mut rig = rig(2, frame(2));
    let mut log = rig.talker("df", Talk::default(), &TALKER);
    rig.noise(2 * BLOCK);
    rig.settle();
    assert!(!drain(&mut log).is_empty());
    let mut faster = frame(2);
    faster.sample_rate = 2.0 * RATE;
    rig.send(Command::Frame {
        frame: Box::new(faster),
    });
    rig.noise(2 * BLOCK);
    rig.settle();
    assert!(drain(&mut log).is_empty());
    let host = rig.aggregator.hosts().find_mut("df").expect("host");
    assert!(host.stats().wants_rebuild());
    assert_eq!(host.stats().gate(), Some(ProcessorGate::Retuning));
}

#[test]
fn a_lost_lane_gates_every_host_on_sync() {
    let mut rig = rig(2, frame(2));
    let _log = rig.talker("df", Talk::default(), &TALKER);
    rig.send(Command::LanesLost { slots: vec![1] });
    rig.noise(BLOCK);
    rig.settle();
    let gate = rig
        .aggregator
        .hosts()
        .find_mut("df")
        .expect("host")
        .stats()
        .gate();
    assert_eq!(gate, Some(ProcessorGate::Sync));
    assert_eq!(rig.status().lanes[1].sync, SyncState::Lost);
}

#[test]
fn a_panicking_aggregator_is_reported_as_stopped() {
    let rig = rig(2, frame(2));
    let board = rig.board.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = spawn("sdrmm-array-test".to_owned(), rig.aggregator, stop.clone()).expect("spawn");
    rig.queue
        .send(Command::Hold(Box::new(|| panic!("the aggregator broke"))))
        .expect("queued");
    handle.thread().unpark();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "the aggregator never stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
    let exit = handle.join().expect("caught");
    assert!(exit.hosts.is_empty());
    let mut status = ArrayStatus::default();
    board.fill(&mut status);
    assert_eq!(
        status.failure,
        Some(ArrayFailure::Stopped {
            message: "the aggregator broke".to_owned()
        })
    );
}

#[cfg(feature = "probe")]
mod probe {
    use sdrmm_channels::array_processor::probe::{ProbeBlock, take_probe_log};
    use sdrmm_wire::processor::ProbeParams;

    use super::*;
    use crate::{array::host::HostPlan, runtime::VirtualLaneSink};

    fn probe_host(
        rig: &Rig,
        node: &str,
        probe: ProbeParams,
        sinks: Vec<Option<VirtualLaneSink>>,
    ) -> Box<ProcessorHost> {
        let mut plan: HostPlan = plan(
            node,
            ProcessorParams::Probe(probe),
            &rig.taps.sinks,
            rig.frame.lanes(),
        );
        plan.sinks = sinks;
        let built = ProcessorHost::build(plan, &rig.frame).expect("probe host");
        assert!(built.radar.is_none());
        built.host
    }

    pub(super) fn two_probes(rig: &mut Rig) -> crate::capture_ring::CaptureConsumer {
        let (sink, ring) = VirtualLaneSink::detached(2, 1 << 17);
        let ported = probe_host(
            rig,
            "probe-alloc-ported",
            ProbeParams {
                time: true,
                lane_ports: 1,
                ..ProbeParams::default()
            },
            vec![Some(sink)],
        );
        let plain = probe_host(rig, "probe-alloc", ProbeParams::default(), Vec::new());
        rig.send(Command::AddHost { host: ported });
        rig.send(Command::AddHost { host: plain });
        ring
    }

    #[test]
    fn two_lane_ports_write_two_virtual_lanes() {
        let mut rig = rig(2, frame(2));
        let (first, mut first_ring) = VirtualLaneSink::detached(2, 1 << 17);
        let (second, mut second_ring) = VirtualLaneSink::detached(3, 1 << 17);
        let host = probe_host(
            &rig,
            "probe-ports",
            ProbeParams {
                lane_ports: 2,
                ..ProbeParams::default()
            },
            vec![Some(first), Some(second)],
        );
        rig.send(Command::AddHost { host });
        rig.write(2 * BLOCK, |lane, at| Complex::new(lane as f32, at as f32));
        rig.settle();
        let mut seen = [Vec::new(), Vec::new()];
        for (ring, seen) in [&mut first_ring, &mut second_ring]
            .into_iter()
            .zip(&mut seen)
        {
            ring.consume(usize::MAX, |samples, index| seen.push((index, samples[0])));
        }
        assert_eq!(seen[0].first(), Some(&(0, Complex::new(0.0, 0.0))));
        assert_eq!(seen[1].first(), Some(&(0, Complex::new(1.0, 0.0))));
    }

    #[test]
    fn a_gated_host_counts_gated_samples_and_skips_its_lanes() {
        let mut rig = rig(2, frame(2));
        let (sink, mut ring) = VirtualLaneSink::detached(2, 1 << 17);
        let host = probe_host(
            &rig,
            "probe-gated",
            ProbeParams {
                gain: true,
                lane_ports: 1,
                ..ProbeParams::default()
            },
            vec![Some(sink)],
        );
        let mut log = take_probe_log("probe-gated").expect("log");
        let stats = host.stats().clone();
        rig.send(Command::AddHost { host });
        rig.noise(3 * BLOCK);
        assert!(rig.settle() > 0);
        let gated = stats.gated_samples.load(Ordering::Relaxed);
        assert_eq!(gated, 3 * BLOCK as u64 - crate::array::window::PRE_GUARD);
        assert_eq!(stats.gate(), Some(ProcessorGate::Gain));
        assert_eq!(ring.consume(usize::MAX, |_, _| {}), 0);
        let skipped = rig
            .aggregator
            .hosts()
            .find_mut("probe-gated")
            .and_then(|host| host.lane_index(0));
        assert_eq!(skipped, Some(gated));
        assert!(log.drain().is_empty());
        rig.aggregator.set_ready(false, true);
        rig.noise(BLOCK);
        rig.settle();
        let resumed = log.drain();
        let first: &ProbeBlock = resumed.first().expect("processed after the gate");
        assert!(first.gap_before);
        assert_eq!(first.last_reset, Some(ResetCause::Resumed));
    }

    #[test]
    fn an_in_place_apply_keeps_processor_state() {
        let mut rig = rig(2, frame(2));
        let params = ProbeParams::default();
        let host = probe_host(&rig, "probe-apply", params, Vec::new());
        let mut log = take_probe_log("probe-apply").expect("log");
        rig.send(Command::AddHost { host });
        rig.noise(2 * BLOCK);
        rig.settle();
        let before = log.drain();
        let first = before.last().expect("blocks");
        assert_eq!(first.applies, 0);
        rig.send(Command::ApplyParams {
            node: "probe-apply".to_owned(),
            params: Box::new(ProcessorParams::Probe(ProbeParams {
                time: false,
                ..params
            })),
        });
        rig.noise(2 * BLOCK);
        rig.settle();
        let after = log.drain();
        let last = after.last().expect("blocks after the apply");
        assert_eq!(last.build, first.build);
        assert_eq!(last.applies, 1);
        assert!(last.seq > first.seq);
    }

    #[test]
    fn a_replaced_host_is_dropped_on_the_reclaimer() {
        let mut rig = rig(2, frame(2));
        let slow = probe_host(
            &rig,
            "probe-slow",
            ProbeParams {
                block_drop_ms: 500,
                ..ProbeParams::default()
            },
            Vec::new(),
        );
        rig.send(Command::AddHost { host: slow });
        let fresh = probe_host(
            &rig,
            "probe-slow",
            ProbeParams {
                rebuild: 1,
                ..ProbeParams::default()
            },
            Vec::new(),
        );
        let started = Instant::now();
        rig.send(Command::ReplaceHost { host: fresh });
        rig.noise(BLOCK);
        rig.settle();
        assert!(started.elapsed() < Duration::from_millis(250));
    }
}
