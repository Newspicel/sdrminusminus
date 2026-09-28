use num_complex::Complex;
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, Coherence, DecoderEvent, ProcessorReading, RdsUpdate,
};

use super::{
    ArrayBlock, ArrayCtx, CalView, CorrectionView, LaneBuffer, OutputSlots, OutputTally, Pose,
    ProcessorOutput,
};

pub(crate) struct Bench {
    pub(crate) node: String,
    pub(crate) rate: f64,
    pub(crate) center_hz: f64,
    pub(crate) centers: Vec<f64>,
    pub(crate) geometry: ArrayGeometry,
    pub(crate) max_block: usize,
    positions: Vec<[f64; 3]>,
}

impl Bench {
    pub(crate) fn spread(node: &str, rate: f64, offsets: &[f64], max_block: usize) -> Self {
        let center_hz = 100e6;
        let geometry = ArrayGeometry::default();
        let positions = geometry.positions(offsets.len()).unwrap_or_default();
        Self {
            node: node.to_owned(),
            rate,
            center_hz,
            centers: offsets.iter().map(|offset| center_hz + offset).collect(),
            geometry,
            max_block,
            positions,
        }
    }

    pub(crate) fn together(node: &str, lanes: usize, rate: f64, max_block: usize) -> Self {
        Self::spread(node, rate, &vec![0.0; lanes], max_block)
    }

    pub(crate) fn ctx(&self) -> ArrayCtx<'_> {
        ArrayCtx {
            node: &self.node,
            lanes: self.centers.len(),
            sample_rate: self.rate,
            center_hz: self.center_hz,
            lane_centers_hz: &self.centers,
            geometry: &self.geometry,
            positions_m: &self.positions,
            manifold: None,
            tier: Coherence::PhaseCoherent,
            tuning: ArrayTuningMode::Together,
            max_block: self.max_block,
        }
    }
}

pub(crate) fn block<'a>(lanes: &'a [&'a [Complex<f32>]], unix_ns: u64) -> ArrayBlock<'a> {
    ArrayBlock {
        lanes,
        corrected: true,
        correction: CorrectionView::identity(),
        first_index: 0,
        unix_ns,
        generation: 0,
        gap_before: false,
        centers_hz: &[],
        cal: CalView::default(),
        pose: Pose::default(),
    }
}

pub(crate) fn noise(len: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed.max(1);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32 - 0.5
    };
    (0..len).map(|_| Complex::new(next(), next())).collect()
}

pub(crate) struct Sink {
    pub(crate) report: Option<ProcessorReading>,
    pub(crate) events: Vec<DecoderEvent>,
    pub(crate) lanes: Vec<LaneBuffer>,
}

impl Sink {
    pub(crate) fn new(type_id: &str, lane_capacities: &[usize]) -> Self {
        Self {
            report: ProcessorReading::empty(type_id),
            events: vec![DecoderEvent::Rds(RdsUpdate::default()); 2],
            lanes: lane_capacities
                .iter()
                .map(|capacity| LaneBuffer::new(*capacity))
                .collect(),
        }
    }

    pub(crate) fn run(&mut self, work: impl FnOnce(&mut ProcessorOutput<'_>)) -> OutputTally {
        for lane in &mut self.lanes {
            lane.clear();
        }
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: None,
            events: &mut self.events,
            lanes: &mut self.lanes,
        });
        work(&mut out);
        out.tally()
    }
}
