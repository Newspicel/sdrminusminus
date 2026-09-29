#![expect(dead_code)]

mod aggregator;
mod align;
mod batch;
mod board;
mod capture;
mod controller;
mod correct;
mod host;
mod radar;
mod record;
mod tap;
pub(crate) mod tier;
mod track;
pub(crate) mod tuner;
mod warm;
mod window;
mod worker;

use std::{
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{JoinHandle, Thread},
};

use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::array_processor::{GeoFix, INLINE_INPUT_LIMIT, MAX_LANES};
use sdrmm_device::{GapScope, lock};
use sdrmm_wire::{
    ArrayCalRecord, ArrayFailure, ArrayNode, ArrayOrientation, ArrayTune, ArrayTuningMode,
    Coherence, ProcessorParams, ProcessorReading, SurfaceFrame,
};
use tokio::sync::broadcast;

pub use sdrmm_channels::array_processor::ProcessorAction;

use crate::EngineError;

pub(crate) use aggregator::AggregatorExit;
#[cfg(test)]
pub(crate) use aggregator::{PoseRing, oriented};
pub(crate) use board::StatusBoard;
pub(crate) use capture::{
    CaptureBuffers, CaptureJob, CaptureRequest, Solution, SolveFailure, SolveSummary,
};
pub(crate) use controller::{
    ControlCommand, ControlConfig, NoiseSwitch, SyncContext, spawn_controller,
};
pub(crate) use correct::CorrectionSet;
use correct::{correction_stage, spawn_stage};
pub(crate) use host::{
    ArrayShape, HostPlan, HostSinks, ProcessorHost, ProcessorStats, SteerInput, SteerMailbox,
};
pub(crate) use radar::{DedicatedRunner, Prepared, RadarPlan, prepare_dedicated};
pub(crate) use record::{ArrayRecording, RecordTap, record_tap};
pub(crate) use tap::{LaneFeed, TapPort, TapWriter};
pub(crate) use tier::TierDecision;
pub(crate) use window::BlankCause;
pub(crate) use worker::spawn_worker;

pub(crate) const COMMAND_SLOTS: usize = 64;
pub(crate) const CONTROL_EVENT_SLOTS: usize = 256;

static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) const fn align_notes() -> align::AlignNotes {
    align::AlignNotes::new()
}

#[cfg(test)]
pub(crate) fn noted_marks(notes: &align::AlignNotes) -> Vec<sdrmm_device::LaneMark> {
    notes
        .as_slice()
        .iter()
        .filter_map(|note| match note {
            align::AlignNote::Mark { mark, .. } => Some(*mark),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LaneRef {
    pub device_set: u32,
    pub stream: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArraySpec {
    pub node: String,
    pub lanes: Vec<Option<LaneRef>>,
    pub settings: ArrayNode,
    pub tune: Option<ArrayTune>,
    pub warm: Option<ArrayCalRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessorSpec {
    pub node: String,
    pub array: String,
    pub params: ProcessorParams,
    pub lane_ports: Vec<String>,
    pub steer_from: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ArrayEvent {
    Report {
        processor: String,
        reading: Arc<ProcessorReading>,
    },
    Surface {
        processor: String,
        seq: u32,
        surface: Arc<SurfaceFrame>,
    },
    Solved {
        array: String,
        record: ArrayCalRecord,
    },
}

pub(crate) trait ArrayControl: Send + Sync {
    fn switch_array_noise(&self, node: &str, on: bool) -> Result<(), EngineError>;
    fn tune_array_internal(&self, node: &str, tune: ArrayTune) -> Result<(), EngineError>;

    fn sync_context(&self, node: &str) -> Result<SyncContext, EngineError>;
    fn clock_drift(&self, node: &str, ppm: Option<f64>) -> Result<(), EngineError>;
}

pub(crate) struct CommandQueue {
    producer: Mutex<Producer<Command>>,
    aggregator: OnceLock<Thread>,
}

impl CommandQueue {
    pub(crate) fn new() -> (Self, Consumer<Command>) {
        let (producer, consumer) = RingBuffer::new(COMMAND_SLOTS);
        (
            Self {
                producer: Mutex::new(producer),
                aggregator: OnceLock::new(),
            },
            consumer,
        )
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), EngineError> {
        lock(&self.producer)
            .push(command)
            .map_err(|_| EngineError::Array(ArrayFailure::Busy))?;
        if let Some(thread) = self.aggregator.get() {
            thread.unpark();
        }
        Ok(())
    }

    fn wake(&self, thread: Thread) {
        let _ = self.aggregator.set(thread);
    }
}

pub(crate) enum Command {
    SwapFeeds {
        slots: Vec<(usize, LaneFeed)>,
    },
    LanesLost {
        slots: Vec<usize>,
    },
    AddHost {
        host: Box<ProcessorHost>,
    },
    ReplaceHost {
        host: Box<ProcessorHost>,
    },
    RemoveHost {
        node: String,
    },
    ApplyParams {
        node: String,
        params: Box<ProcessorParams>,
    },
    SwapVirtual {
        node: String,
        port: usize,
        sink: crate::runtime::VirtualLaneSink,
    },
    Action {
        node: String,
        action: ProcessorAction,
    },
    Frame {
        frame: Box<LiveFrame>,
    },
    Capture {
        request: CaptureRequest,
    },
    Recalibrate,
    Record {
        writer: Option<Box<RecordTap>>,
    },
    Pose {
        sample: PoseSample,
    },
    CommitDedicated {
        node: String,
        prepared: Prepared,
    },
    #[cfg(any(test, feature = "probe"))]
    Hold(Box<dyn FnOnce() + Send>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PoseSample {
    pub(crate) host_ns: i64,
    pub(crate) heading_deg: Option<f64>,
    pub(crate) heading_sigma_deg: f32,
    pub(crate) yaw_rate_dps: Option<f32>,
    pub(crate) fix: Option<GeoFix>,
    pub(crate) moving: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LiveFrame {
    pub(crate) sample_rate: f64,
    pub(crate) center_hz: f64,
    pub(crate) lane_centers_hz: Vec<f64>,
    pub(crate) orientation: ArrayOrientation,
    pub(crate) tier: Coherence,
    pub(crate) keeps_phase: bool,
    pub(crate) needs_time: bool,
    pub(crate) tuning: ArrayTuningMode,
    pub(crate) dc_block: bool,
    pub(crate) in_flight: u64,
    pub(crate) devices: [u8; MAX_LANES],
}

impl LiveFrame {
    pub(crate) fn lanes(&self) -> usize {
        self.lane_centers_hz.len().min(MAX_LANES)
    }

    pub(crate) fn one_device(&self) -> bool {
        let lanes = &self.devices[..self.lanes()];
        lanes.iter().all(|device| Some(device) == lanes.first())
    }

    pub(crate) fn same_device(&self, a: usize, b: usize) -> bool {
        self.devices
            .get(a)
            .is_some_and(|device| Some(device) == self.devices.get(b))
    }

    pub(crate) fn latency_ns(&self) -> i64 {
        if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            (self.in_flight as f64 * 1e9 / self.sample_rate).round() as i64
        } else {
            0
        }
    }
}

pub(crate) enum Retired {
    Host(Box<ProcessorHost>),
    Feed(LaneFeed),
    Correction(Box<CorrectionSet>),
    Solution(Box<Solution>),
    Command(Command),
    Frame(Box<LiveFrame>),
    Record(Box<RecordTap>),
    Sink(crate::runtime::VirtualLaneSink),
    Runner(Box<dyn DedicatedRunner>),
    Buffers(Box<CaptureBuffers>),
    Params(Box<ProcessorParams>),
    Prepared(Prepared),
    Node(String),
}

impl Retired {
    pub(crate) fn release(self) {
        match self {
            Self::Runner(runner) | Self::Prepared(Prepared::Rebuild(runner)) => {
                if let Err(error) = runner.retire().join() {
                    tracing::error!(%error, "a retired array runner did not stop cleanly");
                }
            }
            Self::Host(host) => host.retire(),
            Self::Feed(feed) => drop(feed),
            Self::Correction(set) => drop(set),
            Self::Solution(solution) => drop(solution),
            Self::Command(command) => drop(command),
            Self::Frame(frame) => drop(frame),
            Self::Record(record) => drop(record),
            Self::Sink(sink) => drop(sink),
            Self::Buffers(buffers) => drop(buffers),
            Self::Params(params) => drop(params),
            Self::Prepared(prepared) => drop(prepared),
            Self::Node(node) => drop(node),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[expect(clippy::large_enum_variant)]
pub(crate) enum AggregatorEvent {
    Captured {
        id: u32,
    },
    CaptureRefused {
        id: u32,
    },
    Solved {
        id: u32,
        summary: SolveSummary,
    },
    SolveFailed {
        id: u32,
        failure: SolveFailure,
    },
    NoiseOnset {
        at: u64,
    },
    NoiseEnded {
        at: u64,
    },
    NoiseNotSeen,
    BlankEnded {
        cause: BlankCause,
        at: u64,
    },
    Uncertain {
        lane: usize,
        error: u64,
        scope: GapScope,
    },
    Clipped {
        lane: usize,
    },
}

pub(crate) struct RuntimeSetup {
    pub(crate) node: String,
    pub(crate) feeds: Vec<Option<LaneFeed>>,
    pub(crate) frame: LiveFrame,
    pub(crate) board: Arc<StatusBoard>,
    pub(crate) control: Weak<dyn ArrayControl>,
    pub(crate) config: ControlConfig,
    pub(crate) events: broadcast::Sender<ArrayEvent>,
}

pub(crate) struct ArrayRuntime {
    node: String,
    commands: Arc<CommandQueue>,
    board: Arc<StatusBoard>,
    stop: Arc<AtomicBool>,
    aggregator: Option<JoinHandle<AggregatorExit>>,
    worker: Option<JoinHandle<()>>,
    controller: Option<JoinHandle<()>>,
    corrector: Option<JoinHandle<()>>,
    controller_tx: mpsc::Sender<ControlCommand>,
}

fn heavy(frame: &LiveFrame) -> bool {
    frame.lanes() as f64 * frame.sample_rate > INLINE_INPUT_LIMIT
}

impl ArrayRuntime {
    pub(crate) fn start(setup: RuntimeSetup) -> Result<Self, EngineError> {
        let RuntimeSetup {
            node,
            feeds,
            frame,
            board,
            control,
            config,
            events,
        } = setup;
        let serial = NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed);
        let (queue, commands) = CommandQueue::new();
        let queue = Arc::new(queue);
        let stop = Arc::new(AtomicBool::new(false));
        let wiring = aggregator::wire(feeds.len(), frame.sample_rate, stop.clone())?;
        let worker = spawn_worker(format!("sdrmm-array-sync-{serial}"), wiring.worker)?;
        let (controller_tx, controller_rx) = mpsc::channel();
        let controller = spawn_controller(
            format!("sdrmm-array-ctl-{serial}"),
            controller::ControllerIo {
                node: node.clone(),
                control,
                commands: controller_rx,
                queue: queue.clone(),
                events: wiring.events,
                board: board.clone(),
                array_events: events,
                config,
                link: wiring.link,
            },
        );
        let controller = match controller {
            Ok(controller) => controller,
            Err(error) => {
                stop.store(true, Ordering::Release);
                worker.thread().unpark();
                drop(wiring.aggregator);
                join_quietly(worker, "array sync worker");
                return Err(error);
            }
        };
        let aggregator_thread = Arc::new(OnceLock::new());
        let (stage, corrector) = if heavy(&frame) {
            let (stage, stage_worker) =
                correction_stage(feeds.len(), stop.clone(), aggregator_thread.clone());
            match spawn_stage(format!("sdrmm-array-correct-{serial}"), stage_worker) {
                Ok(handle) => (Some(stage), Some(handle)),
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    let _ = controller_tx.send(ControlCommand::Stop);
                    worker.thread().unpark();
                    join_quietly(controller, "array controller");
                    join_quietly(worker, "array sync worker");
                    return Err(error);
                }
            }
        } else {
            (None, None)
        };
        let aggregator = aggregator::Aggregator::new(
            feeds,
            Box::new(frame),
            board.clone(),
            commands,
            wiring.aggregator,
            Some(worker.thread().clone()),
            stage,
        );
        let spawned = aggregator::spawn(format!("sdrmm-array-{serial}"), aggregator, stop.clone());
        let aggregator = match spawned {
            Ok(handle) => handle,
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = controller_tx.send(ControlCommand::Stop);
                worker.thread().unpark();
                join_quietly(controller, "array controller");
                join_quietly(worker, "array sync worker");
                if let Some(corrector) = corrector {
                    corrector.thread().unpark();
                    join_quietly(corrector, "array correction");
                }
                return Err(error);
            }
        };
        let _ = aggregator_thread.set(aggregator.thread().clone());
        queue.wake(aggregator.thread().clone());
        Ok(Self {
            node,
            commands: queue,
            board,
            stop,
            aggregator: Some(aggregator),
            worker: Some(worker),
            controller: Some(controller),
            corrector,
            controller_tx,
        })
    }

    pub(crate) fn node(&self) -> &str {
        &self.node
    }

    pub(crate) fn board(&self) -> &Arc<StatusBoard> {
        &self.board
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), EngineError> {
        self.commands.send(command)
    }

    pub(crate) fn control(&self, command: ControlCommand) -> Result<(), EngineError> {
        self.controller_tx.send(command).map_err(|_| {
            EngineError::Array(ArrayFailure::Stopped {
                message: "array controller stopped".to_owned(),
            })
        })
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.aggregator.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub(crate) fn stop(mut self) -> Option<AggregatorExit> {
        self.halt()
    }

    fn halt(&mut self) -> Option<AggregatorExit> {
        let _ = self.controller_tx.send(ControlCommand::Stop);
        if let Some(controller) = self.controller.take() {
            join_quietly(controller, "array controller");
        }
        self.stop.store(true, Ordering::Release);
        let exit = self.aggregator.take().and_then(|aggregator| {
            aggregator.thread().unpark();
            aggregator.join().ok()
        });
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            join_quietly(worker, "array sync worker");
        }
        if let Some(corrector) = self.corrector.take() {
            corrector.thread().unpark();
            join_quietly(corrector, "array correction");
        }
        exit
    }
}

impl Drop for ArrayRuntime {
    fn drop(&mut self) {
        drop(self.halt());
    }
}

fn join_quietly(handle: JoinHandle<()>, what: &str) {
    if handle.join().is_err() {
        tracing::error!(thread = what, "an array thread panicked");
    }
}

#[cfg(test)]
mod tests;
