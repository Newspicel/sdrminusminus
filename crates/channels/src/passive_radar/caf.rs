use std::ops::Range;
use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::radar::batch::{BatchKernel, BatchShape, MAX_SURVEILLANCE, WeightsAt};
use sdrmm_dsp::radar::wiener::{GroupSums, SolveStats, WeightTable, WienerSolver};

use super::assemble::CpiJob;
use super::plan::RadarPlan;
use crate::ChannelError;

type C32 = Complex<f32>;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CafError {
    #[error("CAF helper stopped")]
    Crew,
    #[error("CPI does not match the plan")]
    Shape,
}

pub struct CubeOut {
    pub cube: Vec<C32>,
    pub suppression_db: [f32; MAX_SURVEILLANCE],
    pub unsuppressed_groups: u32,
}

impl CubeOut {
    #[must_use]
    pub fn new(shape: &BatchShape) -> Self {
        Self {
            cube: vec![C32::default(); shape.lanes * shape.gates * shape.batches],
            suppression_db: [0.0; MAX_SURVEILLANCE],
            unsuppressed_groups: 0,
        }
    }

    #[must_use]
    pub fn cell(&self, shape: &BatchShape, lane: usize, gate: usize, row: usize) -> C32 {
        self.cube
            .get((lane * shape.gates + gate) * shape.batches + row)
            .copied()
            .unwrap_or_default()
    }
}

pub trait CafBackend: Send {
    fn run(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), CafError>;
    fn run_shared(&mut self, job: &Arc<CpiJob>, out: &mut CubeOut) -> Result<(), CafError> {
        self.run(job, out)
    }
    fn gpu(&self) -> bool;
    fn threads(&self) -> u32;
}

pub struct SpectraShard {
    pub first_batch: usize,
    pub batches: usize,
    pub reference: Vec<C32>,
    pub model: Vec<C32>,
    pub lanes: Vec<C32>,
    pub energy: Vec<f64>,
    pub gates: Vec<C32>,
}

impl SpectraShard {
    #[must_use]
    pub fn new(shape: &BatchShape, batches: Range<usize>) -> Self {
        let count = batches.len();
        let m = shape.fft_len;
        let model = if shape.eca() { count * m } else { 0 };
        Self {
            first_batch: batches.start,
            batches: count,
            reference: vec![C32::default(); count * m],
            model: vec![C32::default(); model],
            lanes: vec![C32::default(); count * shape.lanes * m],
            energy: vec![0.0; count * shape.lanes],
            gates: vec![C32::default(); count * shape.lanes * shape.gates],
        }
    }

    fn model(&self, local: usize, m: usize) -> &[C32] {
        self.model.get(local * m..(local + 1) * m).unwrap_or(&[])
    }
}

pub struct GateChunk {
    pub gates: Range<usize>,
    pub series: Box<[C32]>,
}

impl GateChunk {
    #[must_use]
    pub fn new(shape: &BatchShape, gates: Range<usize>) -> Self {
        let len = shape.lanes * gates.len() * shape.batches;
        Self {
            gates,
            series: vec![C32::default(); len].into_boxed_slice(),
        }
    }
}

pub fn spectra_stage(
    kernel: &mut BatchKernel,
    job: &CpiJob,
    shard: &mut SpectraShard,
) -> Result<(), CafError> {
    let shape = kernel.shape();
    let (m, lanes) = (shape.fft_len, shape.lanes);
    if job.lanes != lanes + 1 || shard.first_batch + shard.batches > shape.batches {
        return Err(CafError::Shape);
    }
    let reference = job.lane(0);
    for local in 0..shard.batches {
        let batch = shard.first_batch + local;
        let spectrum = &mut shard.reference[local * m..(local + 1) * m];
        let model = shard
            .model
            .get_mut(local * m..(local + 1) * m)
            .unwrap_or(&mut []);
        kernel
            .reference(reference, batch, spectrum, model)
            .map_err(|_| CafError::Shape)?;
        for lane in 0..lanes {
            let window = job.lane(lane + 1);
            let at = (local * lanes + lane) * m;
            kernel
                .surveillance(window, batch, spectrum, &mut shard.lanes[at..at + m])
                .map_err(|_| CafError::Shape)?;
            shard.energy[local * lanes + lane] = shape.energy(window, batch);
        }
    }
    Ok(())
}

pub fn solve_stage(
    solver: &mut WienerSolver,
    shards: &[&SpectraShard],
    sums: &mut GroupSums,
    table: &mut WeightTable,
) -> Result<SolveStats, CafError> {
    sums.clear();
    for shard in shards {
        let m = shard.reference.len() / shard.batches.max(1);
        let lanes = shard.energy.len() / shard.batches.max(1);
        for local in 0..shard.batches {
            solver
                .accumulate(
                    sums,
                    shard.first_batch + local,
                    shard.model(local, m),
                    &shard.lanes[local * lanes * m..(local + 1) * lanes * m],
                    &shard.energy[local * lanes..(local + 1) * lanes],
                )
                .map_err(|_| CafError::Shape)?;
        }
    }
    solver.solve(sums, table).map_err(|_| CafError::Shape)
}

pub fn residual_stage(
    kernel: &mut BatchKernel,
    table: Option<&WeightTable>,
    shard: &mut SpectraShard,
) -> Result<(), CafError> {
    let shape = kernel.shape();
    let (m, lanes, gates) = (shape.fft_len, shape.lanes, shape.gates);
    for local in 0..shard.batches {
        let batch = shard.first_batch + local;
        let model = shard.model.get(local * m..(local + 1) * m).unwrap_or(&[]);
        for lane in 0..lanes {
            let weights = table.map_or(WeightsAt::none(), |table| table.at(lane, batch));
            let product = &shard.lanes[(local * lanes + lane) * m..(local * lanes + lane + 1) * m];
            let at = (lane * shard.batches + local) * gates;
            kernel
                .residual(product, model, weights, &mut shard.gates[at..at + gates])
                .map_err(|_| CafError::Shape)?;
        }
    }
    Ok(())
}

pub fn gather_stage(
    shards: &[&SpectraShard],
    shape: &BatchShape,
    gates: Range<usize>,
    series: &mut [C32],
) -> Result<(), CafError> {
    let (batches, lanes, width) = (shape.batches, shape.lanes, gates.len());
    if gates.end > shape.gates || series.len() < lanes * width * batches {
        return Err(CafError::Shape);
    }
    for shard in shards {
        if shard.first_batch + shard.batches > batches {
            return Err(CafError::Shape);
        }
        for lane in 0..lanes {
            for local in 0..shard.batches {
                let batch = shard.first_batch + local;
                let row = (lane * shard.batches + local) * shape.gates;
                let source = &shard.gates[row + gates.start..row + gates.end];
                for (offset, value) in source.iter().enumerate() {
                    series[(lane * width + offset) * batches + batch] = *value;
                }
            }
        }
    }
    Ok(())
}

pub fn doppler_stage(
    kernel: &mut BatchKernel,
    window: &[f32],
    series: &mut [C32],
) -> Result<(), CafError> {
    let batches = kernel.shape().batches;
    for line in series.chunks_exact_mut(batches) {
        kernel.doppler(line, window).map_err(|_| CafError::Shape)?;
    }
    Ok(())
}

struct Eca {
    solver: WienerSolver,
    sums: GroupSums,
    table: WeightTable,
}

pub struct CpuCaf {
    shape: BatchShape,
    kernel: BatchKernel,
    eca: Option<Eca>,
    shard: SpectraShard,
    window: Vec<f32>,
}

impl CpuCaf {
    pub fn new(plan: &RadarPlan) -> Result<Self, ChannelError> {
        let shape = plan.shape;
        let eca = match &plan.groups {
            Some(groups) if shape.eca() => {
                let refused = |_| ChannelError::Refused("Clutter order over 512");
                Some(Eca {
                    solver: WienerSolver::new(shape, groups, plan.params.clutter.loading)
                        .map_err(refused)?,
                    sums: GroupSums::new(&shape, groups),
                    table: WeightTable::new(&shape, groups).map_err(refused)?,
                })
            }
            _ => None,
        };
        Ok(Self {
            shape,
            kernel: BatchKernel::new(shape),
            eca,
            shard: SpectraShard::new(&shape, 0..shape.batches),
            window: plan.window.clone(),
        })
    }
}

impl CafBackend for CpuCaf {
    fn run(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), CafError> {
        if job.window < self.shape.window()
            || out.cube.len() != self.shape.lanes * self.shape.gates * self.shape.batches
        {
            return Err(CafError::Shape);
        }
        spectra_stage(&mut self.kernel, job, &mut self.shard)?;
        match &mut self.eca {
            Some(eca) => {
                let stats = solve_stage(
                    &mut eca.solver,
                    &[&self.shard],
                    &mut eca.sums,
                    &mut eca.table,
                )?;
                out.suppression_db = stats.suppression_db;
                out.unsuppressed_groups = stats.unsuppressed_groups;
            }
            None => {
                out.suppression_db = job.front_suppression_db;
                out.unsuppressed_groups = 0;
            }
        }
        let table = self.eca.as_ref().map(|eca| &eca.table);
        residual_stage(&mut self.kernel, table, &mut self.shard)?;
        gather_stage(
            &[&self.shard],
            &self.shape,
            0..self.shape.gates,
            &mut out.cube,
        )?;
        doppler_stage(&mut self.kernel, &self.window, &mut out.cube)
    }

    fn gpu(&self) -> bool {
        false
    }

    fn threads(&self) -> u32 {
        0
    }
}
