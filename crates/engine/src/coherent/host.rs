use std::sync::Arc;

use num_complex::Complex;
use sdrmm_channels::{
    ChannelError,
    coherent::{CoherentCtx, CoherentOutputs, CoherentRx, RangeDopplerSurface, create_coherent},
};
use sdrmm_wire::{CalState, CoherentParams, DfReading, RadarDetection};
use tokio::sync::broadcast;

use super::AlignedContext;
use crate::{publishing::coherent::CoherentPublisher, runtime::DecodedSink};

/// How often the calibration state goes out on its own, so an operator watching a solve settle
/// sees it move even while nothing is being reported.
const STATE_INTERVAL_S: f64 = 0.5;

const MAX_LANES: usize = sdrmm_wire::MAX_STREAMS as usize;

/// One coherent node's report: what it read, and what the calibration was doing when it read it.
#[derive(Clone, Debug)]
pub struct CoherentUpdate {
    pub node: u32,
    pub reading: Option<DfReading>,
    pub detections: Vec<RadarDetection>,
    pub cal: CalState,
}

/// A range–Doppler surface on its way to a subscriber, kept off the JSON path for the same reason
/// spectrum frames are.
#[derive(Clone, Debug)]
pub struct SurfaceUpdate {
    pub node: u32,
    pub seq: u32,
    pub surface: Arc<RangeDopplerSurface>,
}

#[derive(Clone)]
pub(crate) struct CoherentSinks {
    pub(crate) updates: broadcast::Sender<CoherentUpdate>,
    pub(crate) surfaces: broadcast::Sender<SurfaceUpdate>,
    pub(crate) decoded: DecodedSink,
}

/// Everything one coherent node needs on the aggregator thread: the processor itself, somewhere
/// to put what it produces, and the rule that keeps a phase-dependent processor from answering
/// when the phase is not known.
pub(crate) struct CoherentHost {
    node: u32,
    /// Which of the radio's lanes feeds each element, in element order. An array's elements are
    /// numbered by where they stand, not by which coaxial run happened to reach which port.
    lanes: Vec<u32>,
    rx: Box<dyn CoherentRx>,
    outputs: CoherentOutputs,
    publisher: CoherentPublisher,
    needs_phase: bool,
    center_hz: f64,
    freq_hz: f64,
    since_state: f64,
    state_samples: f64,
    /// What the calibration last said, so that a change in it goes out when it happens rather
    /// than when the next interval comes round. Whoever is running a calibration is waiting on
    /// exactly this.
    told: (bool, bool, bool),
    weights: Option<Vec<Complex<f32>>>,
    lanes_hz: Vec<f64>,
}

impl CoherentHost {
    pub(crate) fn build(
        node: u32,
        ctx: CoherentCtx,
        params: &CoherentParams,
        sinks: CoherentSinks,
        lanes: Vec<u32>,
    ) -> Result<Box<Self>, ChannelError> {
        let descriptor = sdrmm_channels::coherent::coherent_descriptor(params.type_id())
            .ok_or_else(|| ChannelError::UnknownType(params.type_id().to_owned()))?;
        if lanes.len() != ctx.lanes {
            return Err(ChannelError::InvalidSettings(format!(
                "{} elements were wired but the processor takes {}",
                lanes.len(),
                ctx.lanes
            )));
        }
        let rx: Box<dyn CoherentRx> = if matches!(params, CoherentParams::PassiveRadar(_)) {
            Box::new(super::radar::RadarWorker::new(ctx, params)?)
        } else {
            create_coherent(ctx, params)?
        };
        let publisher = CoherentPublisher::new(node, sinks).map_err(|error| {
            ChannelError::InvalidSettings(format!("start coherent publisher: {error}"))
        })?;
        let mut outputs = CoherentOutputs::default();
        outputs
            .wide
            .reserve(super::align::ALIGN_BLOCK * 2 * lanes.len());
        let lanes_hz = vec![ctx.center_hz; lanes.len()];
        Ok(Box::new(Self {
            node,
            lanes,
            rx,
            outputs,
            publisher,
            needs_phase: descriptor.needs_phase,
            center_hz: ctx.center_hz,
            freq_hz: ctx.center_hz,
            since_state: 0.0,
            state_samples: ctx.sample_rate * STATE_INTERVAL_S,
            told: (false, false, false),
            weights: None,
            lanes_hz,
        }))
    }

    pub(super) fn poll(&mut self, center_hz: f64, cal: &CalState) {
        if center_hz != self.center_hz {
            self.center_hz = center_hz;
            self.freq_hz = center_hz;
            self.rx.retuned(center_hz);
        }
        if cal.reference_on || (self.needs_phase && cal.phase_unknown) {
            self.rx.retuned(center_hz);
            return;
        }
        self.outputs.reset();
        self.rx.poll(&mut self.outputs);
        self.publish_outputs(cal);
    }

    fn publish_outputs(&mut self, cal: &CalState) {
        let has_report = self.outputs.bearing.is_some()
            || !self.outputs.detections.is_empty()
            || self.outputs.surface.is_some();
        if !has_report && self.outputs.events.is_empty() && self.outputs.weights.is_none() {
            return;
        }
        if let Some(weights) = self.outputs.weights.take() {
            self.weights = Some(reorder(&weights, &self.lanes));
        }
        self.publisher
            .publish(&mut self.outputs, cal, self.freq_hz, has_report);
    }

    pub(crate) fn tuned(&mut self, radio_hz: &[f64], center_hz: f64) {
        for (slot, lane) in self.lanes_hz.iter_mut().zip(&self.lanes) {
            if let Some(hz) = radio_hz.get(*lane as usize) {
                *slot = *hz;
            }
        }
        self.rx.tuned(&self.lanes_hz, center_hz);
    }

    pub(crate) fn wide(&self) -> &[Complex<f32>] {
        &self.outputs.wide
    }

    pub(crate) const fn node(&self) -> u32 {
        self.node
    }

    /// The steering this processor last worked out, handed over once so the aggregator can point
    /// the beam lane where the array is looking.
    pub(crate) fn take_weights(&mut self) -> Option<Vec<Complex<f32>>> {
        self.weights.take()
    }
}

/// Puts a processor's per-element weights back in the radio's own lane order, because the
/// aggregator sums lanes and the processor counts elements.
fn reorder(weights: &[Complex<f32>], lanes: &[u32]) -> Vec<Complex<f32>> {
    let mut out = vec![
        Complex::new(0.0, 0.0);
        lanes
            .iter()
            .map(|lane| *lane as usize + 1)
            .max()
            .unwrap_or(0)
    ];
    for (weight, lane) in weights.iter().zip(lanes) {
        if let Some(slot) = out.get_mut(*lane as usize) {
            *slot = *weight;
        }
    }
    out
}

impl super::AlignedSink for CoherentHost {
    fn process(&mut self, lanes: &[&[Complex<f32>]], ctx: AlignedContext<'_>) {
        self.outputs.wide.clear();
        if ctx.center_hz != self.center_hz || ctx.realigned {
            self.center_hz = ctx.center_hz;
            self.freq_hz = ctx.center_hz;
            self.rx.retuned(ctx.center_hz);
        }
        let count = lanes.first().map_or(0, |lane| lane.len()) as f64;
        self.since_state += count;
        let state = (ctx.cal.solved, ctx.cal.phase_unknown, ctx.cal.reference_on);
        let due = self.since_state >= self.state_samples || state != self.told;
        if due {
            self.since_state = 0.0;
            self.told = state;
        }
        if ctx.cal.reference_on || (self.needs_phase && ctx.cal.phase_unknown) {
            self.rx.retuned(ctx.center_hz);
            if due {
                self.outputs.reset();
                self.publisher
                    .publish(&mut self.outputs, ctx.cal, self.freq_hz, true);
            }
            return;
        }
        self.outputs.reset();
        let mut ordered: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let mut count = 0;
        for (slot, source) in ordered.iter_mut().zip(&self.lanes) {
            let Some(lane) = lanes.get(*source as usize) else {
                return;
            };
            *slot = lane;
            count += 1;
        }
        self.rx.process(&ordered[..count], &mut self.outputs);
        self.publish_outputs(ctx.cal);
    }
}
