use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering, fence},
    },
};

use sdrmm_channels::{
    ChannelError,
    array_processor::{
        ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneBuffer, MAX_EVENTS_PER_BLOCK,
        MAX_LANE_PORTS, OutputSlots, OutputTally, ProcessorAction, ProcessorDescriptor,
        ProcessorFaults, ProcessorNeeds, ProcessorOutput, ResetCause, Steer, TuningNeed,
        create_processor, processor_descriptor,
    },
};
use sdrmm_dsp::manifold::ManifoldTable;
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, BearingSource, Coherence, DecodedRecord, DecoderEvent,
    DfBearing, EventOrigin, GpuUse, NO_CHANNEL, ProcessorGate, ProcessorParams, ProcessorReading,
    ProcessorStatus, RangeDopplerOwned, SpatialSpectrumOwned, StreamKind, SurfaceFrame,
    VisibilityOwned, patch::MAX_NODE_ID_LEN, processor::df::DF_POINTS,
};
use tokio::sync::broadcast;

use super::{
    ArrayEvent, CorrectionSet, LiveFrame,
    align::ALIGN_BLOCK,
    batch::BatchRunner,
    board::{gate_code, gate_of},
    correct::CORR_HOP,
    radar::{DedicatedRunner, Prepared, RadarPlan, build_dedicated},
};
use crate::{EngineError, publishing::Publisher, runtime::VirtualLaneSink};

pub(crate) const MAX_HOSTS: usize = 32;
pub(crate) const HOST_BLOCK: usize = ALIGN_BLOCK + CORR_HOP;
const PUBLISH_SLOTS: usize = 16;
const SURFACE_CELLS: usize = 1 << 19;
const STEER_WORDS: usize = 12;
const STEER_READS: usize = 4;
const MAX_OTHERS: usize = 3;

pub(crate) struct ProcessorStats {
    pub(crate) gated_samples: AtomicU64,
    pub(crate) dropped_samples: AtomicU64,
    pub(crate) dropped_reports: AtomicU64,
    pub(crate) lane_overflows: AtomicU64,
    pub(crate) gate: AtomicU8,
    pub(crate) alive: AtomicBool,
    pub(crate) lane_mismatch: AtomicU64,
    pub(crate) solver_failures: AtomicU64,
    pub(crate) resets: AtomicU64,
    pub(crate) truncated: AtomicU64,
    pub(crate) refused: AtomicU64,
    pub(crate) rebuild: AtomicBool,
}

impl Default for ProcessorStats {
    fn default() -> Self {
        Self {
            gated_samples: AtomicU64::new(0),
            dropped_samples: AtomicU64::new(0),
            dropped_reports: AtomicU64::new(0),
            lane_overflows: AtomicU64::new(0),
            gate: AtomicU8::new(0),
            alive: AtomicBool::new(true),
            lane_mismatch: AtomicU64::new(0),
            solver_failures: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            truncated: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            rebuild: AtomicBool::new(false),
        }
    }
}

impl ProcessorStats {
    pub(crate) fn gate(&self) -> Option<ProcessorGate> {
        gate_of(self.gate.load(Ordering::Relaxed))
    }

    pub(crate) fn set_gate(&self, gate: Option<ProcessorGate>) {
        self.gate.store(gate_code(gate), Ordering::Relaxed);
    }

    pub(crate) fn wants_rebuild(&self) -> bool {
        self.rebuild.load(Ordering::Relaxed)
    }

    pub(crate) fn faults(&self) -> ProcessorFaults {
        ProcessorFaults {
            lane_mismatch: self.lane_mismatch.load(Ordering::Relaxed),
            dropped_blocks: 0,
            solver_failures: self.solver_failures.load(Ordering::Relaxed),
            resets: self.resets.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_faults(&self, base: &ProcessorFaults, faults: ProcessorFaults) {
        self.lane_mismatch
            .store(base.lane_mismatch + faults.lane_mismatch, Ordering::Relaxed);
        self.solver_failures.store(
            base.solver_failures + faults.solver_failures,
            Ordering::Relaxed,
        );
        self.resets
            .store(base.resets + faults.resets, Ordering::Relaxed);
        self.truncated
            .store(base.truncated + faults.truncated, Ordering::Relaxed);
    }

    pub(crate) fn status(&self, node: &str, kind: &str, error: Option<String>) -> ProcessorStatus {
        let alive = self.alive.load(Ordering::Relaxed);
        let error = error.or_else(|| {
            if !alive {
                Some("Stopped".to_owned())
            } else if self.wants_rebuild() {
                Some("Rebuilding".to_owned())
            } else if self.refused.load(Ordering::Relaxed) > 0 {
                Some("Settings refused".to_owned())
            } else {
                None
            }
        });
        ProcessorStatus {
            node: node.to_owned(),
            kind: kind.to_owned(),
            running: alive,
            gated: self.gate(),
            gated_samples: self.gated_samples.load(Ordering::Relaxed),
            dropped_samples: self.dropped_samples.load(Ordering::Relaxed),
            dropped_reports: self.dropped_reports.load(Ordering::Relaxed),
            lane_overflows: self.lane_overflows.load(Ordering::Relaxed),
            lane_mismatch: self.lane_mismatch.load(Ordering::Relaxed),
            solver_failures: self.solver_failures.load(Ordering::Relaxed),
            resets: self.resets.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
            error,
        }
    }
}

pub(crate) struct SteerMailbox {
    seq: AtomicU64,
    words: [AtomicU64; STEER_WORDS],
}

impl Default for SteerMailbox {
    fn default() -> Self {
        Self {
            seq: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl SteerMailbox {
    pub(crate) fn post(&self, steer: &Steer) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (slot, word) in self.words.iter().zip(encode(steer)) {
            slot.store(word, Ordering::Relaxed);
        }
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    pub(crate) fn read(&self, seen: &mut u64) -> Option<Steer> {
        for _ in 0..STEER_READS {
            let before = self.seq.load(Ordering::Acquire);
            if before == *seen {
                return None;
            }
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let mut words = [0u64; STEER_WORDS];
            for (word, slot) in words.iter_mut().zip(&self.words) {
                *word = slot.load(Ordering::Relaxed);
            }
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                *seen = before;
                return Some(decode(&words));
            }
        }
        None
    }
}

fn optional(value: Option<f64>) -> u64 {
    value.unwrap_or(f64::NAN).to_bits()
}

fn from_optional(bits: u64) -> Option<f64> {
    let value = f64::from_bits(bits);
    (!value.is_nan()).then_some(value)
}

fn encode(steer: &Steer) -> [u64; STEER_WORDS] {
    [
        steer.relative_deg.to_bits(),
        optional(steer.true_deg),
        steer.elevation_deg.to_bits(),
        f64::from(steer.sigma_deg).to_bits(),
        steer.others_relative_deg[0].to_bits(),
        steer.others_relative_deg[1].to_bits(),
        steer.others_relative_deg[2].to_bits(),
        optional(steer.others_true_deg[0]),
        optional(steer.others_true_deg[1]),
        optional(steer.others_true_deg[2]),
        u64::from(steer.others),
        steer.wall_ms,
    ]
}

fn decode(words: &[u64; STEER_WORDS]) -> Steer {
    Steer {
        same_array: false,
        relative_deg: f64::from_bits(words[0]),
        true_deg: from_optional(words[1]),
        elevation_deg: f64::from_bits(words[2]),
        sigma_deg: f64::from_bits(words[3]) as f32,
        others_relative_deg: [
            f64::from_bits(words[4]),
            f64::from_bits(words[5]),
            f64::from_bits(words[6]),
        ],
        others_true_deg: [
            from_optional(words[7]),
            from_optional(words[8]),
            from_optional(words[9]),
        ],
        others: u8::try_from(words[10]).unwrap_or(0),
        wall_ms: words[11],
    }
}

pub(crate) enum SteerInput {
    None,
    Local(String),
    Remote(Arc<SteerMailbox>),
}

#[derive(Clone)]
pub(crate) struct HostSinks {
    pub(crate) events: broadcast::Sender<ArrayEvent>,
    pub(crate) decoded: broadcast::Sender<DecodedRecord>,
    pub(crate) decoded_lost: Arc<AtomicU64>,
    pub(crate) anchor: u32,
}

#[derive(Clone)]
pub(crate) struct ArrayShape {
    pub(crate) geometry: ArrayGeometry,
    pub(crate) positions: Vec<[f64; 3]>,
    pub(crate) manifold: Option<Arc<ManifoldTable>>,
    pub(crate) tuning: ArrayTuningMode,
}

impl ArrayShape {
    pub(crate) fn ctx<'a>(&'a self, node: &'a str, frame: &'a LiveFrame) -> ArrayCtx<'a> {
        ArrayCtx {
            node,
            lanes: frame.lanes(),
            sample_rate: frame.sample_rate,
            center_hz: frame.center_hz,
            lane_centers_hz: &frame.lane_centers_hz[..frame.lanes()],
            geometry: &self.geometry,
            positions_m: &self.positions,
            manifold: self.manifold.as_deref(),
            tier: frame.tier,
            tuning: self.tuning,
            max_block: HOST_BLOCK,
        }
    }
}

pub(crate) struct HostPlan {
    pub(crate) node: String,
    pub(crate) params: ProcessorParams,
    pub(crate) shape: ArrayShape,
    pub(crate) sinks: Vec<Option<VirtualLaneSink>>,
    pub(crate) outputs: HostSinks,
    pub(crate) steer_in: SteerInput,
    pub(crate) steer_out: Arc<SteerMailbox>,
    pub(crate) stats: Arc<ProcessorStats>,
    pub(crate) gpu: GpuUse,
}

pub(crate) struct BuiltHost {
    pub(crate) host: Box<ProcessorHost>,
    pub(crate) radar: Option<Arc<RadarPlan>>,
}

enum Out {
    Report,
    Surface,
    Event,
}

pub(crate) struct Packet {
    out: Out,
    reading: Option<ProcessorReading>,
    surface: Option<SurfaceFrame>,
    event: DecoderEvent,
    seq: u32,
    freq_hz: f64,
}

pub(crate) fn event_slot() -> DecoderEvent {
    DecoderEvent::Df(DfBearing {
        bearing_deg: 0.0,
        confidence: 0.0,
        lat: None,
        lon: None,
        station_id: Some(String::with_capacity(MAX_NODE_ID_LEN)),
        node: String::with_capacity(MAX_NODE_ID_LEN),
        sigma_deg: 0.0,
        accuracy_m: None,
        heading_deg: None,
        heading_sigma_deg: None,
        relative_deg: None,
        mirror_deg: None,
        freq_hz: None,
        source: BearingSource::Array,
        moving: false,
        others: Vec::with_capacity(MAX_OTHERS),
        likelihood: Vec::with_capacity(DF_POINTS),
    })
}

fn surface_slot(kind: Option<StreamKind>) -> Option<SurfaceFrame> {
    match kind? {
        StreamKind::RangeDoppler => Some(SurfaceFrame::RangeDoppler(RangeDopplerOwned {
            cells: Vec::with_capacity(SURFACE_CELLS),
            ..RangeDopplerOwned::default()
        })),
        StreamKind::SpatialSpectrum => Some(SurfaceFrame::SpatialSpectrum(SpatialSpectrumOwned {
            cells: Vec::with_capacity(SURFACE_CELLS),
            ..SpatialSpectrumOwned::default()
        })),
        StreamKind::Visibility => Some(SurfaceFrame::Visibility(VisibilityOwned {
            amplitude: Vec::with_capacity(SURFACE_CELLS),
            phase: Vec::with_capacity(SURFACE_CELLS),
            ..VisibilityOwned::default()
        })),
        _ => None,
    }
}

fn event_freq(event: &DecoderEvent) -> Option<f64> {
    match event {
        DecoderEvent::Df(bearing) => bearing.freq_hz,
        _ => None,
    }
}

pub(crate) struct Outputs {
    report: Option<ProcessorReading>,
    surface: Option<SurfaceFrame>,
    events: Vec<DecoderEvent>,
    lanes: Vec<LaneBuffer>,
    sinks: Vec<Option<VirtualLaneSink>>,
    lane_ratio: [f64; MAX_LANE_PORTS],
    skip_carry: [f64; MAX_LANE_PORTS],
    publisher: Publisher<Packet>,
    stats: Arc<ProcessorStats>,
    decoded_lost: Arc<AtomicU64>,
    freq_hz: f64,
    surface_seq: u32,
}

impl Outputs {
    fn new(
        plan: &mut HostPlan,
        descriptor: &'static ProcessorDescriptor,
        ctx: &ArrayCtx<'_>,
    ) -> io::Result<Self> {
        let ports = descriptor.lane_ports.len().min(MAX_LANE_PORTS);
        let mut lanes = Vec::with_capacity(ports);
        let mut lane_ratio = [1.0; MAX_LANE_PORTS];
        for (port, ratio) in lane_ratio.iter_mut().enumerate().take(ports) {
            let format = (descriptor.lane_format)(&plan.params, ctx, port);
            lanes.push(LaneBuffer::new(format.capacity));
            if ctx.sample_rate > 0.0 && format.sample_rate > 0.0 {
                *ratio = format.sample_rate / ctx.sample_rate;
            }
        }
        let mut sinks: Vec<Option<VirtualLaneSink>> = std::mem::take(&mut plan.sinks);
        sinks.resize_with(ports, || None);
        let type_id = descriptor.type_id;
        let kind = descriptor.surface;
        let node = plan.node.clone();
        let sink = plan.outputs.clone();
        let publisher = Publisher::new(
            "sdrmm-array-pub",
            PUBLISH_SLOTS,
            || Packet {
                out: Out::Report,
                reading: ProcessorReading::empty(type_id),
                surface: surface_slot(kind),
                event: event_slot(),
                seq: 0,
                freq_hz: 0.0,
            },
            move |packet: &mut Packet| publish(&node, &sink, packet),
            || {},
        )?;
        Ok(Self {
            report: ProcessorReading::empty(type_id),
            surface: surface_slot(kind),
            events: (0..MAX_EVENTS_PER_BLOCK).map(|_| event_slot()).collect(),
            lanes,
            sinks,
            lane_ratio,
            skip_carry: [0.0; MAX_LANE_PORTS],
            publisher,
            stats: plan.stats.clone(),
            decoded_lost: plan.outputs.decoded_lost.clone(),
            freq_hz: ctx.center_hz,
            surface_seq: 0,
        })
    }

    pub(crate) fn run(
        &mut self,
        work: impl FnOnce(&mut ProcessorOutput<'_>),
        freq_hz: f64,
    ) -> Option<Steer> {
        self.freq_hz = freq_hz;
        for lane in &mut self.lanes {
            lane.clear();
        }
        let tally = {
            let mut out = ProcessorOutput::new(OutputSlots {
                report: self.report.as_mut(),
                surface: self.surface.as_mut(),
                events: &mut self.events,
                lanes: &mut self.lanes,
            });
            work(&mut out);
            out.tally()
        };
        self.deliver(tally);
        tally.steer
    }

    fn deliver(&mut self, tally: OutputTally) {
        let mut dropped = tally.dropped_reports;
        if tally.report
            && !self.publisher.submit(|packet| {
                packet.out = Out::Report;
                std::mem::swap(&mut packet.reading, &mut self.report);
            })
        {
            dropped += 1;
        }
        if tally.surface {
            self.surface_seq = self.surface_seq.wrapping_add(1);
            let seq = self.surface_seq;
            if !self.publisher.submit(|packet| {
                packet.out = Out::Surface;
                packet.seq = seq;
                std::mem::swap(&mut packet.surface, &mut self.surface);
            }) {
                dropped += 1;
            }
        }
        if dropped > 0 {
            self.stats
                .dropped_reports
                .fetch_add(dropped, Ordering::Relaxed);
        }
        let mut lost = tally.dropped_events;
        for event in self.events.iter_mut().take(tally.events) {
            let freq_hz = event_freq(event).unwrap_or(self.freq_hz);
            if !self.publisher.submit(|packet| {
                packet.out = Out::Event;
                packet.freq_hz = freq_hz;
                std::mem::swap(&mut packet.event, event);
            }) {
                lost += 1;
            }
        }
        if lost > 0 {
            self.decoded_lost.fetch_add(lost, Ordering::Relaxed);
        }
        self.lanes_out();
    }

    fn lanes_out(&mut self) {
        let mut overflowed = 0;
        for (buffer, sink) in self.lanes.iter().zip(&mut self.sinks) {
            overflowed += buffer.overflowed();
            let Some(sink) = sink else {
                continue;
            };
            if !buffer.samples().is_empty() {
                sink.push(buffer.samples());
            }
            if buffer.skipped() > 0 {
                sink.skip(buffer.skipped());
            }
        }
        if overflowed > 0 {
            self.stats
                .lane_overflows
                .fetch_add(overflowed, Ordering::Relaxed);
        }
    }

    pub(crate) fn skip(&mut self, samples: u64) {
        for ((sink, ratio), carry) in self
            .sinks
            .iter_mut()
            .zip(self.lane_ratio)
            .zip(&mut self.skip_carry)
        {
            let Some(sink) = sink else {
                continue;
            };
            *carry += samples as f64 * ratio;
            let whole = carry.floor();
            *carry -= whole;
            if whole >= 1.0 {
                sink.skip(whole as u64);
            }
        }
    }

    pub(crate) fn set_sink(
        &mut self,
        port: usize,
        sink: VirtualLaneSink,
    ) -> Result<Option<VirtualLaneSink>, VirtualLaneSink> {
        match self.sinks.get_mut(port) {
            Some(slot) => Ok(slot.replace(sink)),
            None => Err(sink),
        }
    }
}

#[cfg(test)]
pub(crate) static PUBLISH_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn publish(node: &str, sinks: &HostSinks, packet: &mut Packet) {
    #[cfg(test)]
    let _open = PUBLISH_GATE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match packet.out {
        Out::Report => {
            if let Some(reading) = &packet.reading {
                let _ = sinks.events.send(ArrayEvent::Report {
                    processor: node.to_owned(),
                    reading: Arc::new(reading.clone()),
                });
            }
        }
        Out::Surface => {
            if let Some(surface) = &packet.surface {
                let _ = sinks.events.send(ArrayEvent::Surface {
                    processor: node.to_owned(),
                    seq: packet.seq,
                    surface: Arc::new(surface.clone()),
                });
            }
        }
        Out::Event => {
            let record = DecodedRecord {
                origin: Some(EventOrigin {
                    node: node.to_owned(),
                    transmission: 0,
                }),
                device_set: sinks.anchor,
                channel: NO_CHANNEL,
                at: format!("{:.9}", jiff::Timestamp::now()),
                freq_hz: packet.freq_hz,
                event: packet.event.clone(),
                sinks: Vec::new(),
            };
            if sinks.decoded.send(record).is_err() {
                tracing::debug!(node, "array event had no listener");
            }
        }
    }
}

pub(crate) enum Runner {
    Inline(Box<dyn ArrayProcessor>),
    Batched(BatchRunner),
    Dedicated(Box<dyn DedicatedRunner>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GateInputs {
    pub(crate) sample_rate: f64,
    pub(crate) window: Option<ProcessorGate>,
    pub(crate) tier: Coherence,
    pub(crate) tuning: ArrayTuningMode,
    pub(crate) synced: bool,
    pub(crate) phase_ready: bool,
    pub(crate) gain_ready: bool,
}

pub(crate) struct ProcessorHost {
    node: String,
    descriptor: &'static ProcessorDescriptor,
    needs: ProcessorNeeds,
    tuning: TuningNeed,
    banded: bool,
    runner: Runner,
    outputs: Option<Box<Outputs>>,
    stats: Arc<ProcessorStats>,
    base: ProcessorFaults,
    gated: Option<ProcessorGate>,
    gap_pending: bool,
    steer_in: SteerInput,
    steer_seen: u64,
    steer_out: Arc<SteerMailbox>,
    shape: ArrayShape,
}

impl ProcessorHost {
    pub(crate) fn build(plan: HostPlan, frame: &LiveFrame) -> Result<BuiltHost, EngineError> {
        let type_id = plan.params.type_id();
        let descriptor = processor_descriptor(type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        let ctx = plan.shape.ctx(&plan.node, frame);
        let (host, radar) = match (descriptor.execution)(&plan.params, &ctx) {
            Execution::Inline => {
                let processor = create_processor(&ctx, &plan.params)?;
                (
                    Self::with_processor(plan, frame, descriptor, processor)?,
                    None,
                )
            }
            Execution::Worker { batch } => {
                let processor = create_processor(&ctx, &plan.params)?;
                (
                    Self::batched(plan, frame, descriptor, processor, batch)?,
                    None,
                )
            }
            Execution::Dedicated => {
                let (runner, radar) = build_dedicated(&ctx, &plan.params, plan.gpu)?;
                let runner = Runner::Dedicated(runner);
                (
                    Self::assemble(plan, frame, descriptor, runner, true)?,
                    Some(radar),
                )
            }
        };
        Ok(BuiltHost { host, radar })
    }

    pub(crate) fn with_processor(
        plan: HostPlan,
        frame: &LiveFrame,
        descriptor: &'static ProcessorDescriptor,
        processor: Box<dyn ArrayProcessor>,
    ) -> Result<Box<Self>, EngineError> {
        Self::assemble(plan, frame, descriptor, Runner::Inline(processor), true)
    }

    pub(crate) fn batched(
        mut plan: HostPlan,
        frame: &LiveFrame,
        descriptor: &'static ProcessorDescriptor,
        processor: Box<dyn ArrayProcessor>,
        batch: usize,
    ) -> Result<Box<Self>, EngineError> {
        let outputs = {
            let shape = plan.shape.clone();
            let node = plan.node.clone();
            let ctx = shape.ctx(&node, frame);
            Outputs::new(&mut plan, descriptor, &ctx).map_err(publisher_error)?
        };
        let runner = BatchRunner::start(
            plan.node.clone(),
            processor,
            Box::new(outputs),
            plan.shape.clone(),
            frame,
            batch,
            plan.stats.clone(),
        )?;
        Self::assemble(plan, frame, descriptor, Runner::Batched(runner), false)
    }

    fn assemble(
        mut plan: HostPlan,
        frame: &LiveFrame,
        descriptor: &'static ProcessorDescriptor,
        runner: Runner,
        own_outputs: bool,
    ) -> Result<Box<Self>, EngineError> {
        let outputs = if own_outputs {
            let shape = plan.shape.clone();
            let node = plan.node.clone();
            let ctx = shape.ctx(&node, frame);
            Some(Box::new(
                Outputs::new(&mut plan, descriptor, &ctx).map_err(publisher_error)?,
            ))
        } else {
            None
        };
        let base = plan.stats.faults();
        Ok(Box::new(Self {
            needs: (descriptor.needs)(&plan.params),
            tuning: (descriptor.tuning)(&plan.params),
            banded: (descriptor.band)(&plan.params).is_some(),
            node: plan.node,
            descriptor,
            runner,
            outputs,
            stats: plan.stats,
            base,
            gated: None,
            gap_pending: false,
            steer_in: plan.steer_in,
            steer_seen: 0,
            steer_out: plan.steer_out,
            shape: plan.shape,
        }))
    }

    pub(crate) fn node(&self) -> &str {
        &self.node
    }

    fn installed(&self) {
        self.stats.alive.store(true, Ordering::Relaxed);
        self.stats.rebuild.store(false, Ordering::Relaxed);
    }

    pub(crate) const fn descriptor(&self) -> &'static ProcessorDescriptor {
        self.descriptor
    }

    pub(crate) fn stats(&self) -> &Arc<ProcessorStats> {
        &self.stats
    }

    pub(crate) const fn needs_corrector(&self) -> bool {
        (self.needs.time || self.needs.phase) && !self.banded
    }

    pub(crate) const fn gated(&self) -> Option<ProcessorGate> {
        self.gated
    }

    fn steered_by(&self, source: &str) -> bool {
        matches!(&self.steer_in, SteerInput::Local(from) if from == source)
    }

    pub(crate) fn gate_for(&self, inputs: &GateInputs) -> Option<ProcessorGate> {
        let tuning_mismatch = match self.tuning {
            TuningNeed::Together => inputs.tuning == ArrayTuningMode::Spread,
            TuningNeed::Spread => inputs.tuning == ArrayTuningMode::Together,
            TuningNeed::Any => false,
        };
        if let Some(window) = inputs.window {
            Some(window)
        } else if inputs.tier == Coherence::None {
            Some(ProcessorGate::Tier)
        } else if tuning_mismatch {
            Some(ProcessorGate::TuningMode)
        } else if self.needs.time && !inputs.synced {
            Some(ProcessorGate::Sync)
        } else if self.needs.phase && !inputs.phase_ready {
            Some(ProcessorGate::Phase)
        } else if self.needs.gain && !inputs.gain_ready {
            Some(ProcessorGate::Gain)
        } else if self.stats.wants_rebuild() {
            Some(ProcessorGate::Retuning)
        } else {
            None
        }
    }

    pub(crate) fn process(
        &mut self,
        block: &ArrayBlock<'_>,
        inputs: &GateInputs,
        correction: &CorrectionSet,
    ) -> Option<Steer> {
        self.pull_remote_steer();
        let count = block.len();
        if let Some(gate) = self.gate_for(inputs) {
            self.gap_pending |= block.gap_before;
            self.hold(gate, count as u64);
            return None;
        }
        if self.gated.take().is_some() {
            self.stats.set_gate(None);
            self.reset(ResetCause::Resumed);
            self.gap_pending = true;
        }
        let resumed;
        let block = if std::mem::take(&mut self.gap_pending) && !block.gap_before {
            resumed = ArrayBlock {
                lanes: block.lanes,
                corrected: block.corrected,
                correction: block.correction,
                first_index: block.first_index,
                unix_ns: block.unix_ns,
                generation: block.generation,
                gap_before: true,
                centers_hz: block.centers_hz,
                cal: block.cal,
                pose: block.pose,
            };
            &resumed
        } else {
            block
        };
        let freq_hz = block.centers_hz.first().copied().unwrap_or(0.0);
        let steer = match (&mut self.runner, self.outputs.as_deref_mut()) {
            (Runner::Inline(processor), Some(outputs)) => {
                let steer = outputs.run(|out| processor.process(block, out), freq_hz);
                self.stats.record_faults(&self.base, processor.faults());
                steer
            }
            (Runner::Dedicated(runner), _) => {
                runner.push(block);
                None
            }
            (Runner::Batched(batch), _) => {
                batch.push(block, correction, inputs.sample_rate);
                None
            }
            (Runner::Inline(_), None) => None,
        };
        if let Some(steer) = steer {
            self.steer_out.post(&steer);
        }
        steer
    }

    pub(crate) fn hold(&mut self, gate: ProcessorGate, samples: u64) {
        if self.gated != Some(gate) {
            self.gated = Some(gate);
            self.stats.set_gate(Some(gate));
        }
        self.stats
            .gated_samples
            .fetch_add(samples, Ordering::Relaxed);
        match (&mut self.runner, self.outputs.as_deref_mut()) {
            (Runner::Batched(batch), _) => batch.skip(samples),
            (_, Some(outputs)) => outputs.skip(samples),
            _ => {}
        }
    }

    pub(crate) fn poll(&mut self, freq_hz: f64) {
        match (&mut self.runner, self.outputs.as_deref_mut()) {
            (Runner::Inline(processor), Some(outputs)) => {
                let steer = outputs.run(|out| processor.poll(out), freq_hz);
                if let Some(steer) = steer {
                    self.steer_out.post(&steer);
                }
            }
            (Runner::Dedicated(runner), Some(outputs)) => {
                outputs.run(|out| runner.poll(out), freq_hz);
                self.stats.record_faults(&self.base, runner.faults());
            }
            (Runner::Batched(batch), _) => batch.watch(),
            _ => {}
        }
    }

    fn pull_remote_steer(&mut self) {
        if let SteerInput::Remote(mailbox) = &self.steer_in
            && let Some(steer) = mailbox.read(&mut self.steer_seen)
            && steer.true_deg.is_some()
        {
            self.steer(&steer);
        }
    }

    pub(crate) fn steer(&mut self, steer: &Steer) {
        match &mut self.runner {
            Runner::Inline(processor) => processor.steer(steer),
            Runner::Batched(batch) => batch.steer(*steer),
            Runner::Dedicated(_) => {}
        }
    }

    pub(crate) fn reset(&mut self, cause: ResetCause) {
        match &mut self.runner {
            Runner::Inline(processor) => processor.reset(cause),
            Runner::Batched(batch) => batch.reset(cause),
            Runner::Dedicated(_) => {}
        }
    }

    pub(crate) fn retune(&mut self, frame: &LiveFrame) {
        let result = match &mut self.runner {
            Runner::Inline(processor) => {
                let ctx = self.shape.ctx(&self.node, frame);
                processor.retune(&ctx)
            }
            Runner::Batched(batch) => {
                batch.retune(frame);
                Ok(())
            }
            Runner::Dedicated(_) => Ok(()),
        };
        match result {
            Ok(()) => self.reset(ResetCause::Retuned),
            Err(_) => self.stats.rebuild.store(true, Ordering::Relaxed),
        }
    }

    pub(crate) fn apply(&mut self, params: Box<ProcessorParams>) -> Option<Box<ProcessorParams>> {
        let needs = (self.descriptor.needs)(&params);
        let tuning = (self.descriptor.tuning)(&params);
        let banded = (self.descriptor.band)(&params).is_some();
        let (applied, left) = match &mut self.runner {
            Runner::Inline(processor) => {
                let applied = processor.apply(&params).is_ok();
                if !applied {
                    self.stats.refused.fetch_add(1, Ordering::Relaxed);
                }
                (applied, Some(params))
            }
            Runner::Batched(batch) => {
                let left = batch.apply(params);
                (left.is_none(), left)
            }
            Runner::Dedicated(_) => {
                self.stats.refused.fetch_add(1, Ordering::Relaxed);
                (false, Some(params))
            }
        };
        if applied {
            self.needs = needs;
            self.tuning = tuning;
            self.banded = banded;
        }
        left
    }

    pub(crate) fn action(&mut self, action: ProcessorAction) {
        let result = match &mut self.runner {
            Runner::Inline(processor) => processor.action(action),
            Runner::Batched(batch) => {
                batch.action(action);
                Ok(())
            }
            Runner::Dedicated(runner) => runner.action(action),
        };
        if result.is_err() {
            self.stats.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn set_sink(
        &mut self,
        port: usize,
        sink: VirtualLaneSink,
    ) -> Option<VirtualLaneSink> {
        match (&mut self.runner, self.outputs.as_deref_mut()) {
            (Runner::Batched(batch), _) => batch.set_sink(port, sink),
            (_, Some(outputs)) => match outputs.set_sink(port, sink) {
                Ok(previous) => previous,
                Err(unused) => Some(unused),
            },
            (_, None) => Some(sink),
        }
    }

    pub(crate) fn commit(&mut self, prepared: Prepared) -> Option<Box<dyn DedicatedRunner>> {
        match &mut self.runner {
            Runner::Dedicated(runner) => runner.commit(prepared),
            _ => match prepared {
                Prepared::Rebuild(runner) => Some(runner),
                Prepared::Same | Prepared::Live(_) => None,
            },
        }
    }

    pub(crate) fn retire(self: Box<Self>) {
        let host = *self;
        if let Runner::Dedicated(runner) = host.runner
            && let Err(error) = runner.retire().join()
        {
            tracing::error!(%error, "a retired radar runner did not stop cleanly");
        }
    }

    #[cfg(test)]
    pub(crate) fn lane_index(&self, port: usize) -> Option<u64> {
        self.outputs
            .as_ref()?
            .sinks
            .get(port)?
            .as_ref()
            .map(VirtualLaneSink::next_index)
    }
}

fn publisher_error(error: io::Error) -> EngineError {
    EngineError::Processor(format!("start array publisher: {error}"))
}

pub(crate) struct HostList {
    #[expect(clippy::vec_box)]
    hosts: Vec<Box<ProcessorHost>>,
    steers: [Option<Steer>; MAX_HOSTS],
}

impl HostList {
    pub(crate) fn new() -> Self {
        Self {
            hosts: Vec::with_capacity(MAX_HOSTS),
            steers: [None; MAX_HOSTS],
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.hosts.len()
    }

    pub(crate) fn needs_corrector(&self) -> bool {
        self.hosts.iter().any(|host| host.needs_corrector())
    }

    pub(crate) fn add(&mut self, host: Box<ProcessorHost>) -> Result<(), Box<ProcessorHost>> {
        if self.hosts.len() >= MAX_HOSTS {
            host.stats.alive.store(false, Ordering::Relaxed);
            return Err(host);
        }
        host.installed();
        self.hosts.push(host);
        Ok(())
    }

    pub(crate) fn replace(
        &mut self,
        host: Box<ProcessorHost>,
    ) -> Result<Option<Box<ProcessorHost>>, Box<ProcessorHost>> {
        match self.hosts.iter_mut().find(|held| held.node == host.node) {
            Some(held) => {
                host.installed();
                Ok(Some(std::mem::replace(held, host)))
            }
            None => self.add(host).map(|()| None),
        }
    }

    pub(crate) fn remove(&mut self, node: &str) -> Option<Box<ProcessorHost>> {
        let index = self.hosts.iter().position(|host| host.node == node)?;
        Some(self.hosts.remove(index))
    }

    pub(crate) fn find_mut(&mut self, node: &str) -> Option<&mut ProcessorHost> {
        self.hosts
            .iter_mut()
            .find(|host| host.node == node)
            .map(|host| &mut **host)
    }

    pub(crate) fn process(
        &mut self,
        block: &ArrayBlock<'_>,
        inputs: &GateInputs,
        correction: &CorrectionSet,
    ) {
        for (host, steer) in self.hosts.iter_mut().zip(&mut self.steers) {
            *steer = host.process(block, inputs, correction);
        }
        self.deliver_steers();
    }

    fn deliver_steers(&mut self) {
        for source in 0..self.hosts.len() {
            let Some(steer) = self.steers[source].take() else {
                continue;
            };
            for target in 0..self.hosts.len() {
                if target != source && self.hosts[target].steered_by(&self.hosts[source].node) {
                    self.hosts[target].steer(&Steer {
                        same_array: true,
                        ..steer
                    });
                }
            }
        }
    }

    pub(crate) fn hold(&mut self, gate: ProcessorGate, samples: u64) {
        for host in &mut self.hosts {
            host.hold(gate, samples);
        }
    }

    pub(crate) fn poll(&mut self, freq_hz: f64) {
        for host in &mut self.hosts {
            host.poll(freq_hz);
        }
    }

    pub(crate) fn reset(&mut self, cause: ResetCause) {
        for host in &mut self.hosts {
            host.reset(cause);
        }
    }

    pub(crate) fn retune(&mut self, frame: &LiveFrame) {
        for host in &mut self.hosts {
            host.retune(frame);
        }
    }

    pub(crate) fn rebuild(&self) {
        for host in &self.hosts {
            host.stats.rebuild.store(true, Ordering::Relaxed);
        }
    }

    #[expect(clippy::vec_box)]
    pub(crate) fn take_all(&mut self) -> Vec<Box<ProcessorHost>> {
        std::mem::take(&mut self.hosts)
    }
}

#[cfg(test)]
pub(crate) mod tests;
