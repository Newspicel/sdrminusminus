use std::sync::Mutex;

use rtrb::{Producer, RingBuffer};
use sdrmm_device::lock;
use sdrmm_wire::{ArrayStatus, CalSourceKind};

use super::*;
use crate::array::{CONTROL_EVENT_SLOTS, worker};

const RATE: f64 = 2_400_000.0;

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Noise(bool),
    Tune(ArrayTune),
    Drift(Option<f64>),
}

#[derive(Default)]
struct Engine {
    calls: Mutex<Vec<Call>>,
    context: Mutex<Option<SyncContext>>,
}

impl Engine {
    fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    fn noise_calls(&self) -> Vec<bool> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Noise(on) => Some(on),
                _ => None,
            })
            .collect()
    }
}

impl ArrayControl for Engine {
    fn switch_array_noise(&self, _node: &str, on: bool) -> Result<(), EngineError> {
        lock(&self.calls).push(Call::Noise(on));
        Ok(())
    }

    fn tune_array_internal(&self, _node: &str, tune: ArrayTune) -> Result<(), EngineError> {
        lock(&self.calls).push(Call::Tune(tune));
        Ok(())
    }

    fn sync_context(&self, _node: &str) -> Result<SyncContext, EngineError> {
        lock(&self.context)
            .clone()
            .ok_or_else(|| EngineError::Processor("no context".to_owned()))
    }

    fn clock_drift(&self, _node: &str, ppm: Option<f64>) -> Result<(), EngineError> {
        lock(&self.calls).push(Call::Drift(ppm));
        Ok(())
    }
}

struct Rig {
    controller: Controller,
    engine: Arc<Engine>,
    commands: Consumer<Command>,
    events: Producer<AggregatorEvent>,
    reports: mpsc::Sender<WorkerReport>,
    orders: mpsc::Receiver<WorkerOrder>,
    board: Arc<StatusBoard>,
    solved: broadcast::Receiver<ArrayEvent>,
    t0: Instant,
    lanes: usize,
}

fn config(source: ArrayCalSource, check_s: u32, devices: usize) -> ControlConfig {
    ControlConfig {
        cal: ArrayCal {
            source,
            check_s,
            equaliser: false,
            warm_start: true,
        },
        gain: ArrayGain::Manual { db: 30.0 },
        needs_time: true,
        needs_phase: true,
        tier: TierDecision {
            tier: Coherence::TimeSync,
            devices,
            keeps_phase: false,
            structural_zero_delay: false,
        },
        sample_rate: RATE,
    }
}

fn context(lanes: usize) -> SyncContext {
    SyncContext {
        lanes: (0..lanes as u32)
            .map(|stream| LaneKey {
                device: "kraken:1000".to_owned(),
                stream,
            })
            .collect(),
        center_hz: 433.92e6,
        gain_db: Some(30.0),
        gain_steps_db: vec![0.0, 10.0, 20.0, 30.0, 40.0],
        positions: (0..lanes)
            .map(|lane| {
                let angle = std::f64::consts::TAU * lane as f64 / lanes as f64;
                [0.3 * angle.sin(), 0.3 * angle.cos(), 0.0]
            })
            .collect(),
        azimuth_deg: Some(0.0),
        warm: None,
    }
}

fn rig(lanes: usize, config: ControlConfig) -> Rig {
    let engine = Arc::new(Engine::default());
    *lock(&engine.context) = Some(context(lanes));
    let control: Weak<dyn ArrayControl> =
        Arc::downgrade(&(engine.clone() as Arc<dyn ArrayControl>));
    let (queue, commands) = CommandQueue::new();
    let (events, events_rx) = RingBuffer::new(CONTROL_EVENT_SLOTS);
    let (link, orders, reports) = worker::link();
    let board = Arc::new(StatusBoard::new(lanes));
    let (array_events, solved) = broadcast::channel(16);
    let (_tx, commands_rx) = mpsc::channel();
    let t0 = Instant::now();
    let (controller, _) = Controller::new(
        ControllerIo {
            node: "array-1".to_owned(),
            control,
            commands: commands_rx,
            queue: Arc::new(queue),
            events: events_rx,
            board: board.clone(),
            array_events,
            config,
            link,
        },
        t0,
    );
    Rig {
        controller,
        engine,
        commands,
        events,
        reports,
        orders,
        board,
        solved,
        t0,
        lanes,
    }
}

impl Rig {
    fn at(&self, seconds: f64) -> Instant {
        self.t0 + Duration::from_secs_f64(seconds)
    }

    fn start(&mut self, seconds: f64) {
        let now = self.at(seconds);
        self.controller.start(now);
    }

    fn poll(&mut self, seconds: f64) {
        let now = self.at(seconds);
        self.controller.poll(now);
    }

    fn command(&mut self, command: ControlCommand, seconds: f64) {
        let now = self.at(seconds);
        assert!(self.controller.command(command, now));
    }

    fn capture(&mut self) -> Option<CaptureRequest> {
        std::iter::from_fn(|| self.commands.pop().ok()).find_map(|command| match command {
            Command::Capture { request } => Some(request),
            _ => None,
        })
    }

    fn expect_capture(&mut self, kind: CaptureKind) -> CaptureRequest {
        let request = self.capture().expect("a capture request");
        assert_eq!(request.kind, kind, "{request:?}");
        request
    }

    fn event(&mut self, event: AggregatorEvent) {
        assert!(self.events.push(event).is_ok());
    }

    fn summary(&self, phase_ready: bool) -> SolveSummary {
        let mut summary = SolveSummary {
            lanes: self.lanes as u8,
            phase_ready,
            gain_ready: phase_ready,
            ..SolveSummary::default()
        };
        for lane in 1..self.lanes {
            summary.delay[lane] = 10.0 * lane as f32;
            summary.phase_deg[lane] = 20.0 * lane as f32;
            summary.coherence[lane] = 0.99;
        }
        summary
    }

    fn solve(&mut self, request: &CaptureRequest, delays: &[f64], seconds: f64) {
        let mut summary = self.summary(request.kind == CaptureKind::Solve);
        let mut detail = [0.0; MAX_LANES];
        for (lane, delay) in delays.iter().enumerate() {
            summary.delay[lane] = *delay as f32;
            detail[lane] = *delay;
        }
        let _ = self
            .reports
            .send(WorkerReport::Solved(Box::new(SolveDetail {
                id: request.id,
                delays: detail,
                equalisers: Vec::new(),
                drift_ppm: None,
            })));
        self.event(AggregatorEvent::Captured { id: request.id });
        self.poll(seconds);
        self.event(AggregatorEvent::Solved {
            id: request.id,
            summary,
        });
        self.poll(seconds);
    }

    fn fail(&mut self, request: &CaptureRequest, failure: SolveFailure, seconds: f64) {
        self.event(AggregatorEvent::Captured { id: request.id });
        self.poll(seconds);
        self.event(AggregatorEvent::SolveFailed {
            id: request.id,
            failure,
        });
        self.poll(seconds);
    }

    fn status(&self) -> ArrayStatus {
        let mut status = ArrayStatus::default();
        self.board.fill(&mut status);
        status
    }

    fn lock_up(&mut self, seconds: f64) {
        self.command(
            ControlCommand::NoiseSwitch(Some(NoiseSwitch {
                device_set: 1,
                kind: NoiseSource::Isolated,
                all_lanes_held: true,
            })),
            seconds,
        );
        self.start(seconds);
        let coarse = self.expect_capture(CaptureKind::Coarse);
        assert_eq!(coarse.start, CaptureStart::NoiseWindow);
        assert_eq!(coarse.decimation, 64);
        self.solve(&coarse, &[0.0, 10.0, 20.0], seconds);
        let solve = self.expect_capture(CaptureKind::Solve);
        assert_eq!(solve.start, CaptureStart::NoiseWindow);
        self.solve(&solve, &[0.0, 10.25, 20.5], seconds);
    }
}

#[test]
fn controller_runs_noise_bursts_at_check_s() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 10, 1));
    rig.lock_up(0.0);
    assert_eq!(rig.engine.noise_calls(), [true, false]);
    let status = rig.status();
    assert_eq!(status.cal, CalPhase::Solved);
    assert_eq!(status.sync, SyncState::Locked);
    assert!(status.last_solve_at.is_some());
    assert!(
        status
            .next_check_in_s
            .is_some_and(|left| left > 5.0 && left <= 10.0)
    );
    rig.poll(5.0);
    assert!(rig.capture().is_none());
    for (round, at) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        rig.poll(at);
        let check = rig.expect_capture(CaptureKind::Solve);
        assert_eq!(check.start, CaptureStart::NoiseWindow);
        assert_eq!(rig.engine.noise_calls().len(), 3 + 2 * round);
        assert_eq!(rig.engine.noise_calls().last(), Some(&true));
        rig.solve(&check, &[0.0, 10.25, 20.5], at);
        assert_eq!(rig.engine.noise_calls().last(), Some(&false));
        rig.poll(at + 5.0);
        assert!(rig.capture().is_none(), "round {round}");
    }
    assert_eq!(rig.status().sync, SyncState::Locked);
    assert!(rig.status().lanes[1].residual_delay.is_some());
}

#[test]
fn a_failed_solve_is_visible_and_retried() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 0, 1));
    rig.command(
        ControlCommand::NoiseSwitch(Some(NoiseSwitch {
            device_set: 1,
            kind: NoiseSource::Isolated,
            all_lanes_held: true,
        })),
        0.0,
    );
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    rig.solve(&coarse, &[0.0, 10.0, 20.0], 0.0);
    let weak = SolveFailure::LowCoherence {
        lane: 2,
        coherence: 0.4,
    };
    let mut at = 0.0;
    for delay in RETRY_DELAYS {
        let solve = rig.expect_capture(CaptureKind::Solve);
        rig.fail(&solve, weak, at);
        assert_eq!(
            rig.status().failure,
            Some(ArrayFailure::LowCoherence {
                lane: 2,
                coherence: 0.4
            })
        );
        assert_eq!(rig.engine.noise_calls().last(), Some(&false));
        assert_ne!(rig.status().cal, CalPhase::Failed);
        rig.poll(at + delay.as_secs_f64() * 0.5);
        assert!(rig.capture().is_none());
        at += delay.as_secs_f64();
        rig.poll(at);
    }
    let last = rig.expect_capture(CaptureKind::Solve);
    rig.fail(&last, weak, at);
    assert_eq!(rig.status().cal, CalPhase::Failed);
    rig.poll(at + 100.0);
    assert!(rig.capture().is_none());
    rig.command(ControlCommand::Recalibrate, at + 100.0);
    let again = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&again, &[0.0, 10.25, 20.5], at + 100.0);
    let status = rig.status();
    assert_eq!(status.failure, None);
    assert_eq!(status.cal, CalPhase::Solved);
}

#[test]
fn a_fine_failure_escalates_to_coarse_within_the_burst() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 0, 1));
    rig.lock_up(0.0);
    rig.command(ControlCommand::Recalibrate, 1.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.fail(&solve, SolveFailure::NoPeak { lane: 1 }, 1.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    assert_eq!(rig.engine.noise_calls().last(), Some(&true));
    rig.solve(&coarse, &[0.0, 12.0, 20.0], 1.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 12.25, 20.5], 1.0);
    assert_eq!(rig.engine.noise_calls().last(), Some(&false));
    assert_eq!(rig.status().failure, None);
}

#[test]
fn a_single_phase_coherent_device_locks_without_coarse() {
    let mut settings = config(ArrayCalSource::Off, 0, 1);
    settings.tier = TierDecision {
        tier: Coherence::PhaseCoherent,
        devices: 1,
        keeps_phase: true,
        structural_zero_delay: true,
    };
    let mut rig = rig(2, settings.clone());
    rig.start(0.0);
    assert!(rig.capture().is_none());
    let status = rig.status();
    assert_eq!(status.sync, SyncState::Locked);
    assert_eq!(status.cal, CalPhase::None);
    rig.command(ControlCommand::Resync { coarse: true }, 1.0);
    rig.event(AggregatorEvent::Uncertain {
        lane: 1,
        error: 50_000,
        scope: sdrmm_device::GapScope::Lane,
    });
    rig.poll(2.0);
    assert!(rig.capture().is_none());
    assert_eq!(rig.status().sync, SyncState::Locked);
    assert_eq!(rig.status().failure, None);
    settings.cal.source = ArrayCalSource::Noise;
    let mut rig = self::rig(2, settings);
    rig.command(
        ControlCommand::NoiseSwitch(Some(NoiseSwitch {
            device_set: 1,
            kind: NoiseSource::Isolated,
            all_lanes_held: true,
        })),
        0.0,
    );
    rig.start(0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 0.0], 0.0);
    rig.command(ControlCommand::Resync { coarse: true }, 1.0);
    rig.expect_capture(CaptureKind::Solve);
}

#[test]
fn a_missing_noise_switch_is_shown_and_time_still_syncs() {
    let mut rig = rig(2, config(ArrayCalSource::Noise, 0, 1));
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    assert_eq!(coarse.start, CaptureStart::Now);
    assert_eq!(coarse.source, ArrayCalSource::Off);
    assert_eq!(rig.status().failure, Some(ArrayFailure::NoNoiseSource));
    rig.solve(&coarse, &[0.0, 7.0], 0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    assert_eq!(solve.source, ArrayCalSource::Off);
    rig.solve(&solve, &[0.0, 7.5], 0.0);
    let status = rig.status();
    assert_eq!(status.sync, SyncState::Locked);
    assert_eq!(status.failure, Some(ArrayFailure::NoNoiseSource));
    assert!(rig.engine.noise_calls().is_empty());
    rig.command(
        ControlCommand::NoiseSwitch(Some(NoiseSwitch {
            device_set: 1,
            kind: NoiseSource::Isolated,
            all_lanes_held: false,
        })),
        1.0,
    );
    rig.command(ControlCommand::Recalibrate, 1.0);
    rig.poll(1.0);
    let shared = rig.expect_capture(CaptureKind::Solve);
    assert_eq!(shared.source, ArrayCalSource::Off);
    assert_eq!(rig.status().failure, Some(ArrayFailure::NoiseShared));
}

#[test]
fn a_coarse_drift_marks_the_clocks_and_stops_syncing() {
    let mut rig = rig(2, config(ArrayCalSource::Off, 10, 2));
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    rig.fail(&coarse, SolveFailure::Drift { ppm: 2.0 }, 0.0);
    assert_eq!(
        rig.status().failure,
        Some(ArrayFailure::ClockDrift { ppm: 2.0 })
    );
    assert_eq!(rig.status().drift_ppm, Some(2.0));
    assert!(rig.engine.calls().contains(&Call::Drift(Some(2.0))));
    assert!(
        std::iter::from_fn(|| rig.commands.pop().ok())
            .any(|command| matches!(command, Command::Recalibrate))
    );
    rig.poll(60.0);
    assert!(rig.capture().is_none());
    rig.command(ControlCommand::Resync { coarse: true }, 61.0);
    assert!(rig.engine.calls().contains(&Call::Drift(None)));
    rig.expect_capture(CaptureKind::Coarse);
    assert_eq!(rig.status().failure, None);
}

#[test]
fn a_live_check_slip_loses_the_lane_and_resolves() {
    let mut rig = rig(2, config(ArrayCalSource::Off, 10, 1));
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    rig.solve(&coarse, &[0.0, 40.0], 0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 40.25], 0.0);
    rig.poll(10.0);
    let check = rig.expect_capture(CaptureKind::Check);
    let mut delays = [None; MAX_LANES];
    delays[0] = Some(0.0);
    delays[1] = Some(43.25);
    let _ = rig
        .reports
        .send(WorkerReport::Checked(Box::new(CheckDetail {
            id: check.id,
            delays,
        })));
    rig.event(AggregatorEvent::Captured { id: check.id });
    rig.poll(10.0);
    assert_eq!(rig.status().lanes[1].sync, SyncState::Lost);
    let resolve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&resolve, &[0.0, 43.25], 10.0);
    assert_eq!(rig.status().sync, SyncState::Locked);
}

#[test]
fn a_skipped_check_keeps_the_lock() {
    let mut rig = rig(2, config(ArrayCalSource::Off, 10, 1));
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    rig.solve(&coarse, &[0.0, 40.0], 0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 40.25], 0.0);
    rig.poll(10.0);
    let check = rig.expect_capture(CaptureKind::Check);
    let _ = rig
        .reports
        .send(WorkerReport::Checked(Box::new(CheckDetail {
            id: check.id,
            delays: [None; MAX_LANES],
        })));
    rig.poll(10.0);
    assert_eq!(rig.status().sync, SyncState::Locked);
    assert_eq!(rig.controller.skipped_checks, 1);
    rig.poll(20.0);
    rig.expect_capture(CaptureKind::Check);
}

#[test]
fn solved_records_are_throttled_and_carry_the_lanes() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 0, 1));
    rig.lock_up(0.0);
    let Ok(ArrayEvent::Solved { array, record }) = rig.solved.try_recv() else {
        panic!("a solved record");
    };
    assert_eq!(array, "array-1");
    assert_eq!(record.lanes.len(), 3);
    assert_eq!(record.source, CalSourceKind::Noise);
    assert!((record.solution[1].delay_samples - 10.25).abs() < 1e-9);
    rig.command(ControlCommand::Recalibrate, 2.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 10.3, 20.5], 2.0);
    assert!(rig.solved.try_recv().is_err());
    rig.poll(10.5);
    let Ok(ArrayEvent::Solved { record, .. }) = rig.solved.try_recv() else {
        panic!("the held record");
    };
    assert!((record.solution[1].delay_samples - 10.3).abs() < 1e-9);
}

#[test]
fn a_matching_warm_record_is_applied_before_the_first_solve() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 0, 1));
    let delays = [0.0, 10.25, 20.5];
    let summary = rig.summary(true);
    let record = warm::record(
        &context(3).lanes,
        &Band {
            center_hz: 433.92e6,
            sample_rate: RATE,
            gain_db: Some(30.0),
            keeps_phase: false,
        },
        CalSourceKind::Noise,
        &summary,
        &delays,
        &[],
    );
    lock(&rig.engine.context).as_mut().expect("a context").warm = Some(record);
    rig.start(0.0);
    let Ok(WorkerOrder::Warm(order)) = rig.orders.try_recv() else {
        panic!("a warm order");
    };
    assert!(order.usage.gain && !order.usage.phase && order.offsets);
    assert!(rig.capture().is_none());
    rig.event(AggregatorEvent::Solved {
        id: order.id,
        summary: rig.summary(false),
    });
    rig.poll(0.1);
    assert_eq!(rig.status().cal, CalPhase::Warm);
    let first = rig.expect_capture(CaptureKind::Solve);
    assert_eq!(first.source, ArrayCalSource::Off);
}

#[test]
fn an_emitter_without_a_heading_needs_a_position() {
    let source = ArrayCalSource::Emitter {
        offset_hz: 50e3,
        bandwidth_hz: 5e3,
        bearing_deg: 37.0,
    };
    let mut settings = config(source, 0, 1);
    settings.tier.structural_zero_delay = true;
    settings.tier.tier = Coherence::PhaseCoherent;
    let mut rig = rig(4, settings);
    lock(&rig.engine.context)
        .as_mut()
        .expect("a context")
        .azimuth_deg = None;
    rig.start(0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    assert_eq!(solve.source, ArrayCalSource::Off);
    assert_eq!(rig.status().failure, Some(ArrayFailure::NeedsPosition));
    lock(&rig.engine.context)
        .as_mut()
        .expect("a context")
        .azimuth_deg = Some(10.0);
    rig.solve(&solve, &[0.0; 4], 0.0);
    rig.command(ControlCommand::Recalibrate, 1.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    assert_eq!(solve.source, source);
    assert_eq!(solve.len, PILOT_CAPTURE);
    let Ok(WorkerOrder::Steer { id, steering }) = rig.orders.try_recv() else {
        panic!("a steering order");
    };
    assert_eq!(id, solve.id);
    assert!((steering[0].norm() - 1.0).abs() < 1e-6);
}

#[test]
fn noise_is_switched_off_when_the_controller_goes_away() {
    let mut rig = rig(2, config(ArrayCalSource::Noise, 0, 1));
    rig.command(
        ControlCommand::NoiseSwitch(Some(NoiseSwitch {
            device_set: 1,
            kind: NoiseSource::Isolated,
            all_lanes_held: true,
        })),
        0.0,
    );
    rig.start(0.0);
    assert_eq!(rig.engine.noise_calls(), [true]);
    let engine = rig.engine.clone();
    drop(rig);
    assert_eq!(engine.noise_calls(), [true, false]);
}

#[test]
fn a_noise_burst_without_onset_times_out_as_noise_not_seen() {
    let mut rig = rig(2, config(ArrayCalSource::Noise, 0, 1));
    rig.command(
        ControlCommand::NoiseSwitch(Some(NoiseSwitch {
            device_set: 1,
            kind: NoiseSource::Isolated,
            all_lanes_held: true,
        })),
        0.0,
    );
    rig.start(0.0);
    rig.expect_capture(CaptureKind::Coarse);
    rig.poll(30.0);
    assert_eq!(rig.status().failure, Some(ArrayFailure::NoiseNotSeen));
    assert_eq!(rig.engine.noise_calls(), [true, false]);
}

#[test]
fn an_uncertain_gap_resyncs_by_its_size() {
    let mut rig = rig(3, config(ArrayCalSource::Noise, 0, 1));
    rig.lock_up(0.0);
    rig.event(AggregatorEvent::Uncertain {
        lane: 1,
        error: 100,
        scope: sdrmm_device::GapScope::Lane,
    });
    rig.poll(1.0);
    let resolve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&resolve, &[0.0, 60.25, 20.5], 1.0);
    let status = rig.status();
    assert_eq!(status.sync, SyncState::Locked);
    assert_eq!(status.lanes[1].sync, SyncState::Locked);
    rig.poll(2.0);
    assert!(rig.capture().is_none());
    let mut rig = self::rig(3, config(ArrayCalSource::Noise, 0, 1));
    rig.lock_up(0.0);
    rig.event(AggregatorEvent::Uncertain {
        lane: 1,
        error: 50_000,
        scope: sdrmm_device::GapScope::Lane,
    });
    rig.poll(1.0);
    rig.expect_capture(CaptureKind::Coarse);
}

#[test]
fn a_not_coherent_tier_never_syncs() {
    let mut settings = config(ArrayCalSource::Noise, 10, 2);
    settings.tier.tier = Coherence::None;
    let mut rig = rig(2, settings.clone());
    rig.start(0.0);
    rig.poll(30.0);
    assert!(rig.capture().is_none());
    assert_eq!(rig.status().failure, Some(ArrayFailure::NotCoherent));
    settings.tier.tier = Coherence::TimeSync;
    rig.command(ControlCommand::Configure(Box::new(settings)), 31.0);
    rig.poll(31.0);
    rig.expect_capture(CaptureKind::Coarse);
    assert_eq!(rig.status().failure, Some(ArrayFailure::NoNoiseSource));
}

#[test]
fn auto_gain_steps_down_on_clip_and_up_after_dwell() {
    let steps = [0.0, 10.0, 20.0, 30.0, 40.0];
    let quiet = [-30.0f32, -25.0];
    let gain = |levels: &[f32], clipped: bool, current_db: f64, since_up, since_clip| {
        auto_step(&AutoGain {
            levels_dbfs: levels,
            clipped,
            current_db,
            steps_db: &steps,
            since_up,
            since_clip,
        })
    };
    let long = Some(Duration::from_secs(60));
    let short = Some(Duration::from_secs(5));
    assert_eq!(gain(&[-3.0, -1.0], true, 30.0, long, short), Some(20.0));
    assert_eq!(gain(&[-3.0], true, 0.0, long, short), None);
    assert_eq!(gain(&quiet, false, 20.0, long, None), Some(30.0));
    assert_eq!(gain(&[-30.0, -26.0], false, 20.0, long, long), Some(30.0));
    assert_eq!(gain(&[-30.0, -26.0], false, 20.0, short, long), None);
    assert_eq!(gain(&[-30.0, -26.0], false, 20.0, long, short), None);
    assert_eq!(gain(&[-10.0], false, 20.0, long, long), None);
    assert_eq!(gain(&[-14.0], false, 20.0, long, long), None);
}

#[test]
fn the_controller_steps_the_array_gain_through_the_engine() {
    let mut settings = config(ArrayCalSource::Noise, 0, 1);
    settings.gain = ArrayGain::Auto;
    let mut rig = rig(3, settings);
    rig.lock_up(0.0);
    rig.board.lanes[1].set_level(-2.0, true);
    rig.poll(2.5);
    assert!(rig.engine.calls().contains(&Call::Tune(ArrayTune {
        center_hz: 433.92e6,
        gain: ArrayGain::Manual { db: 20.0 },
    })));
}

#[test]
fn checks_start_when_a_processor_starts_needing_sync() {
    let mut settings = config(ArrayCalSource::Off, 10, 1);
    settings.needs_time = false;
    settings.needs_phase = false;
    let mut rig = rig(2, settings.clone());
    rig.start(0.0);
    let coarse = rig.expect_capture(CaptureKind::Coarse);
    rig.solve(&coarse, &[0.0, 4.0], 0.0);
    let solve = rig.expect_capture(CaptureKind::Solve);
    rig.solve(&solve, &[0.0, 4.25], 0.0);
    rig.poll(30.0);
    assert!(rig.capture().is_none());
    assert_eq!(rig.status().next_check_in_s, None);
    settings.needs_time = true;
    rig.command(ControlCommand::Configure(Box::new(settings)), 31.0);
    rig.poll(35.0);
    assert!(rig.capture().is_none());
    rig.poll(41.0);
    rig.expect_capture(CaptureKind::Check);
}
