use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_wire::ProcessorParams;
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::processor::ProbeParams;

use super::{
    ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneFormat, MAX_LANE_PORTS, MAX_LANES,
    ProcessorAction, ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput,
    Registration, ResetCause, Steer, TuningNeed, boxed,
};
use crate::ChannelError;

pub const PROBE_PORTS: &[&str] = &["out", "out2"];
pub const PROBE_LOG_BLOCKS: usize = 1_024;

static BUILDS: AtomicU64 = AtomicU64::new(0);
static LOGS: Mutex<Vec<(String, ProbeLog)>> = Mutex::new(Vec::new());

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "probe",
    name: "Probe",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: PROBE_PORTS,
    steer_port: None,
    surface: None,
    needs,
    band: |_| None,
    tuning,
    lane_format,
    execution: |_, _| Execution::Inline,
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: Some(boxed::<ProbeProcessor>),
};

const fn settings(params: &ProcessorParams) -> Option<&ProbeParams> {
    match params {
        ProcessorParams::Probe(probe) => Some(probe),
        _ => None,
    }
}

fn needs(params: &ProcessorParams) -> ProcessorNeeds {
    settings(params).map_or_else(ProcessorNeeds::default, |probe| ProcessorNeeds {
        time: probe.time,
        phase: probe.phase,
        gain: probe.gain,
        geometry: false,
    })
}

fn tuning(params: &ProcessorParams) -> TuningNeed {
    if settings(params).is_some_and(|probe| probe.spread) {
        TuningNeed::Spread
    } else {
        TuningNeed::Together
    }
}

fn written_ports(probe: &ProbeParams) -> usize {
    usize::from(probe.lane_ports).min(MAX_LANE_PORTS)
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    let written = settings(params).is_some_and(|probe| port < written_ports(probe));
    LaneFormat {
        center_hz: ctx.center_hz,
        sample_rate: ctx.sample_rate,
        capacity: if written { ctx.max_block } else { 0 },
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    match (settings(old), settings(new)) {
        (Some(old), Some(new)) => old.rebuild == new.rebuild && old.lane_ports == new.lane_ports,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeBlock {
    pub build: u64,
    pub seq: u64,
    pub lost: u64,
    pub first_index: u64,
    pub unix_ns: u64,
    pub len: usize,
    pub lanes: usize,
    pub corrected: bool,
    pub generation: u32,
    pub gap_before: bool,
    pub phase_ready: bool,
    pub gain_ready: bool,
    pub heading_deg: Option<f64>,
    pub first: [Complex<f32>; MAX_LANES],
    pub power: [f32; MAX_LANES],
    pub applies: u32,
    pub actions: u32,
    pub resets: u32,
    pub last_reset: Option<ResetCause>,
    pub steer: Option<Steer>,
    pub rebuild: u32,
}

pub struct ProbeLog {
    blocks: Consumer<ProbeBlock>,
}

impl ProbeLog {
    pub fn pop(&mut self) -> Option<ProbeBlock> {
        self.blocks.pop().ok()
    }

    pub fn drain(&mut self) -> Vec<ProbeBlock> {
        std::iter::from_fn(|| self.pop()).collect()
    }
}

#[must_use]
pub fn take_probe_log(node: &str) -> Option<ProbeLog> {
    let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    let index = logs.iter().position(|(owner, _)| owner == node)?;
    Some(logs.swap_remove(index).1)
}

fn register(node: &str, log: ProbeLog) {
    let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    logs.retain(|(owner, _)| owner != node);
    logs.push((node.to_owned(), log));
}

pub struct ProbeProcessor {
    params: ProbeParams,
    lanes: usize,
    blocks: Producer<ProbeBlock>,
    record: ProbeBlock,
    faults: ProcessorFaults,
}

impl ProbeProcessor {
    fn write_ports(&self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        for port in 0..written_ports(&self.params) {
            if let Some(mut lane) = out.lane(port) {
                lane.extend(block.lanes[port % self.lanes]);
            }
        }
    }

    fn skip_ports(&self, samples: usize, out: &mut ProcessorOutput<'_>) {
        for port in 0..written_ports(&self.params) {
            out.skip_lane(port, samples as u64);
        }
    }

    fn observe(&mut self, block: &ArrayBlock<'_>) {
        let record = &mut self.record;
        record.first_index = block.first_index;
        record.unix_ns = block.unix_ns;
        record.len = block.len();
        record.lanes = block.lanes.len();
        record.corrected = block.corrected;
        record.generation = block.generation;
        record.gap_before = block.gap_before;
        record.phase_ready = block.cal.phase_ready;
        record.gain_ready = block.cal.gain_ready;
        record.heading_deg = block.pose.heading_deg;
        for (lane, samples) in block.lanes.iter().enumerate() {
            record.first[lane] = samples.first().copied().unwrap_or_default();
            record.power[lane] = mean_power(samples);
        }
        if self.blocks.push(self.record).is_err() {
            self.record.lost += 1;
        }
        self.record.seq += 1;
    }
}

fn mean_power(samples: &[Complex<f32>]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(Complex::norm_sqr).sum::<f32>() / samples.len() as f32
}

impl ArrayProcessor for ProbeProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        let probe = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        if !(1..=MAX_LANES).contains(&ctx.lanes) {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        let (blocks, log) = RingBuffer::new(PROBE_LOG_BLOCKS);
        register(ctx.node, ProbeLog { blocks: log });
        Ok(Self {
            params: *probe,
            lanes: ctx.lanes,
            blocks,
            record: ProbeBlock {
                build: BUILDS.fetch_add(1, Ordering::Relaxed),
                rebuild: probe.rebuild,
                ..ProbeBlock::default()
            },
            faults: ProcessorFaults::default(),
        })
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let probe = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        self.params = *probe;
        self.record.applies += 1;
        Ok(())
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        if ctx.lanes == self.lanes {
            Ok(())
        } else {
            Err(ChannelError::Refused("Lane out of range"))
        }
    }

    fn reset(&mut self, cause: ResetCause) {
        self.record.resets += 1;
        self.record.last_reset = Some(cause);
        self.faults.resets += 1;
    }

    fn steer(&mut self, steer: &Steer) {
        self.record.steer = Some(*steer);
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        self.record.actions += 1;
        Ok(())
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if !self.faults.lanes_match(block, self.lanes) {
            self.skip_ports(block.len(), out);
            return;
        }
        self.observe(block);
        self.write_ports(block, out);
    }

    fn faults(&self) -> ProcessorFaults {
        self.faults
    }
}

impl Drop for ProbeProcessor {
    fn drop(&mut self) {
        if self.params.block_drop_ms > 0 {
            thread::sleep(Duration::from_millis(u64::from(self.params.block_drop_ms)));
        }
    }
}
