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
    weights: Option<Vec<Complex<f32>>>,
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
        let rx = create_coherent(ctx, params)?;
        let publisher = CoherentPublisher::new(node, sinks).map_err(|error| {
            ChannelError::InvalidSettings(format!("start coherent publisher: {error}"))
        })?;
        Ok(Box::new(Self {
            node,
            lanes,
            rx,
            outputs: CoherentOutputs::default(),
            publisher,
            needs_phase: descriptor.needs_phase,
            center_hz: ctx.center_hz,
            freq_hz: ctx.center_hz,
            since_state: 0.0,
            state_samples: ctx.sample_rate * STATE_INTERVAL_S,
            weights: None,
        }))
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
        if ctx.center_hz != self.center_hz {
            self.center_hz = ctx.center_hz;
            self.freq_hz = ctx.center_hz;
            self.rx.retuned(ctx.center_hz);
        }
        let count = lanes.first().map_or(0, |lane| lane.len()) as f64;
        self.since_state += count;
        let due = self.since_state >= self.state_samples;
        if due {
            self.since_state = 0.0;
        }
        if self.needs_phase && ctx.cal.phase_unknown {
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
            .publish(&mut self.outputs, ctx.cal, self.freq_hz, has_report);
    }
}
