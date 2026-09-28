use std::f64::consts::TAU;
use std::ops::Range;
use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::RadarDspError;
use super::batch::{BatchShape, MAX_SURVEILLANCE, WeightsAt};
use crate::linalg::MAX_SOLVE_ORDER;

type C32 = Complex<f32>;
type C64 = Complex<f64>;

pub const MAX_DOPPLER_TAPS: usize = 2;

const RETRY_LOADING: f64 = 100.0;
const RESIDUAL_FLOOR: f64 = 1e-12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupPlan {
    pub bounds: Vec<usize>,
    pub extension: usize,
    pub doppler_taps: usize,
    pub taper: bool,
    pub sliding: bool,
}

impl GroupPlan {
    pub fn split(
        batches: usize,
        group_batches: usize,
        extension: usize,
        doppler_taps: usize,
        taper: bool,
        sliding: bool,
    ) -> Result<Self, RadarDspError> {
        if batches == 0 || doppler_taps > MAX_DOPPLER_TAPS {
            return Err(RadarDspError::Setting);
        }
        let per_group = group_batches.max(1);
        let groups = ((batches as f64 / per_group as f64).round() as usize).clamp(1, batches);
        let bounds = (0..=groups).map(|g| g * batches / groups).collect();
        Ok(Self {
            bounds,
            extension,
            doppler_taps,
            taper,
            sliding,
        })
    }

    #[must_use]
    pub fn groups(&self) -> usize {
        self.bounds.len().saturating_sub(1)
    }

    #[must_use]
    pub const fn taps(&self) -> usize {
        2 * self.doppler_taps + 1
    }

    const fn shifts(&self) -> usize {
        4 * self.doppler_taps + 1
    }

    fn batches(&self) -> usize {
        self.bounds.last().copied().unwrap_or(0)
    }

    fn members(&self, group: usize) -> Range<usize> {
        self.bounds[group]..self.bounds[group + 1]
    }

    fn estimation(&self, group: usize) -> Range<usize> {
        let members = self.members(group);
        if self.sliding {
            members.start.saturating_sub(self.extension)
                ..(members.end + self.extension).min(self.batches())
        } else {
            members
        }
    }

    fn cycles(&self, group: usize, batch: usize) -> f64 {
        (batch as f64 + 0.5) / self.members(group).len() as f64
    }

    fn centre(&self, group: usize) -> f64 {
        let members = self.members(group);
        0.5 * (members.start + members.end) as f64
    }

    fn owner(&self, batch: usize) -> usize {
        self.bounds[1..]
            .iter()
            .position(|&end| batch < end)
            .unwrap_or(self.groups() - 1)
    }

    fn blend(&self, batch: usize) -> (usize, usize, f32) {
        let own = self.owner(batch);
        if self.sliding || !self.taper {
            return (own, own, 0.0);
        }
        let last = self.groups() - 1;
        let x = batch as f64 + 0.5;
        if x <= self.centre(0) {
            return (0, 0, 0.0);
        }
        if x >= self.centre(last) {
            return (last, last, 0.0);
        }
        let group = (0..last)
            .find(|&g| x < self.centre(g + 1))
            .unwrap_or(last - 1);
        let (low, high) = (self.centre(group), self.centre(group + 1));
        (group, group + 1, ((x - low) / (high - low)) as f32)
    }

    fn valid_for(&self, shape: &BatchShape) -> bool {
        self.groups() >= 1
            && self.bounds.first() == Some(&0)
            && self.batches() == shape.batches
            && self.bounds.windows(2).all(|pair| pair[0] < pair[1])
            && self.doppler_taps <= MAX_DOPPLER_TAPS
    }
}

pub struct GroupSums {
    groups: usize,
    lanes: usize,
    taps: usize,
    shifts: usize,
    fft_len: usize,
    shared: Vec<C64>,
    cross: Vec<C64>,
    energy: Vec<f64>,
}

impl GroupSums {
    #[must_use]
    pub fn new(shape: &BatchShape, plan: &GroupPlan) -> Self {
        let (groups, m) = (plan.groups(), shape.fft_len);
        Self {
            groups,
            lanes: shape.lanes,
            taps: plan.taps(),
            shifts: plan.shifts(),
            fft_len: m,
            shared: vec![C64::default(); groups * plan.shifts() * m],
            cross: vec![C64::default(); groups * shape.lanes * plan.taps() * m],
            energy: vec![0.0; groups * shape.lanes],
        }
    }

    pub fn clear(&mut self) {
        self.shared.fill(C64::default());
        self.cross.fill(C64::default());
        self.energy.fill(0.0);
    }

    fn shared(&self, group: usize, shift: usize) -> &[C64] {
        let at = (group * self.shifts + shift) * self.fft_len;
        &self.shared[at..at + self.fft_len]
    }

    fn cross(&self, group: usize, lane: usize, tap: usize) -> &[C64] {
        let at = ((group * self.lanes + lane) * self.taps + tap) * self.fft_len;
        &self.cross[at..at + self.fft_len]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Blend {
    first: usize,
    second: usize,
    share: f32,
}

pub struct WeightTable {
    lanes: usize,
    taps: usize,
    order: usize,
    fft_len: usize,
    doppler_taps: usize,
    active: bool,
    spectra: Vec<C32>,
    weights: Vec<C64>,
    blends: Vec<Blend>,
    phases: Vec<C32>,
}

impl WeightTable {
    pub fn new(shape: &BatchShape, plan: &GroupPlan) -> Result<Self, RadarDspError> {
        if !shape.eca() || !plan.valid_for(shape) {
            return Err(RadarDspError::Shape);
        }
        let (groups, lanes, taps, m) = (plan.groups(), shape.lanes, plan.taps(), shape.fft_len);
        let mut blends = Vec::with_capacity(shape.batches);
        let mut phases = Vec::with_capacity(shape.batches * 2 * taps);
        for batch in 0..shape.batches {
            let (first, second, share) = plan.blend(batch);
            blends.push(Blend {
                first,
                second,
                share,
            });
            for group in [first, second] {
                for tap in 0..taps {
                    let nu = tap as f64 - plan.doppler_taps as f64;
                    let turn = rotation(nu * plan.cycles(group, batch));
                    phases.push(C32::new(turn.re as f32, turn.im as f32));
                }
            }
        }
        Ok(Self {
            lanes,
            taps,
            order: shape.order(),
            fft_len: m,
            doppler_taps: plan.doppler_taps,
            active: false,
            spectra: vec![C32::default(); groups * lanes * taps * m],
            weights: vec![C64::default(); groups * lanes * taps * shape.order()],
            blends,
            phases,
        })
    }

    #[must_use]
    pub fn at(&self, lane: usize, batch: usize) -> WeightsAt<'_> {
        WeightsAt::of(self, lane, batch)
    }

    #[must_use]
    pub fn weights(&self, group: usize, lane: usize) -> &[C64] {
        let len = self.taps * self.order;
        let at = (group * self.lanes + lane) * len;
        self.weights.get(at..at + len).unwrap_or(&[])
    }

    #[must_use]
    pub const fn doppler_taps(&self) -> usize {
        self.doppler_taps
    }

    pub(crate) fn combine(
        &self,
        lane: usize,
        batch: usize,
        kernel: &mut [C32],
    ) -> Result<bool, RadarDspError> {
        let Some(blend) = self.blends.get(batch) else {
            return Err(RadarDspError::Shape);
        };
        if lane >= self.lanes || kernel.len() < self.fft_len {
            return Err(RadarDspError::Shape);
        }
        if !self.active {
            return Ok(false);
        }
        let kernel = &mut kernel[..self.fft_len];
        kernel.fill(C32::default());
        let phases = &self.phases[batch * 2 * self.taps..(batch + 1) * 2 * self.taps];
        for (slot, (group, share)) in [
            (blend.first, 1.0 - blend.share),
            (blend.second, blend.share),
        ]
        .into_iter()
        .enumerate()
        {
            if share == 0.0 {
                continue;
            }
            for tap in 0..self.taps {
                let factor = phases[slot * self.taps + tap] * share;
                let spectrum = self.spectrum(group, lane, tap);
                for (out, w) in kernel.iter_mut().zip(spectrum) {
                    *out += factor * w;
                }
            }
        }
        Ok(true)
    }

    fn spectrum(&self, group: usize, lane: usize, tap: usize) -> &[C32] {
        let at = ((group * self.lanes + lane) * self.taps + tap) * self.fft_len;
        &self.spectra[at..at + self.fft_len]
    }

    fn spectrum_mut(&mut self, group: usize, lane: usize, tap: usize) -> &mut [C32] {
        let at = ((group * self.lanes + lane) * self.taps + tap) * self.fft_len;
        &mut self.spectra[at..at + self.fft_len]
    }

    fn weights_mut(&mut self, group: usize, lane: usize) -> &mut [C64] {
        let len = self.taps * self.order;
        let at = (group * self.lanes + lane) * len;
        &mut self.weights[at..at + len]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SolveStats {
    pub suppression_db: [f32; MAX_SURVEILLANCE],
    pub unsuppressed_groups: u32,
}

pub struct WienerSolver {
    shape: BatchShape,
    plan: GroupPlan,
    loading: f64,
    unknowns: usize,
    forward: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    fft_scratch: Vec<C64>,
    buffer: Vec<C64>,
    lags: Vec<C64>,
    gram: Vec<C64>,
    factor: Vec<C64>,
    rhs: Vec<C64>,
    solution: Vec<C64>,
}

impl WienerSolver {
    pub fn new(shape: BatchShape, groups: &GroupPlan, loading: f32) -> Result<Self, RadarDspError> {
        if !shape.eca() || !groups.valid_for(&shape) {
            return Err(RadarDspError::Shape);
        }
        if !(loading.is_finite() && loading >= 0.0) {
            return Err(RadarDspError::Setting);
        }
        let unknowns = shape.order() * groups.taps();
        if unknowns > MAX_SOLVE_ORDER {
            return Err(RadarDspError::Order);
        }
        let mut planner = FftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(shape.fft_len);
        let inverse = planner.plan_fft_inverse(shape.fft_len);
        let scratch = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let span = 2 * shape.order() - 1;
        Ok(Self {
            shape,
            plan: groups.clone(),
            loading: f64::from(loading),
            unknowns,
            forward,
            inverse,
            fft_scratch: vec![C64::default(); scratch],
            buffer: vec![C64::default(); shape.fft_len],
            lags: vec![C64::default(); groups.shifts() * span],
            gram: vec![C64::default(); unknowns * unknowns],
            factor: vec![C64::default(); unknowns * unknowns],
            rhs: vec![C64::default(); unknowns],
            solution: vec![C64::default(); unknowns],
        })
    }

    #[must_use]
    pub const fn unknowns(&self) -> usize {
        self.unknowns
    }

    pub fn accumulate(
        &self,
        sums: &mut GroupSums,
        batch: usize,
        shared: &[C32],
        lanes: &[C32],
        energy: &[f64],
    ) -> Result<(), RadarDspError> {
        let (m, lane_count) = (self.shape.fft_len, self.shape.lanes);
        let fits = batch < self.shape.batches
            && shared.len() >= m
            && lanes.len() >= lane_count * m
            && energy.len() >= lane_count
            && sums.groups == self.plan.groups()
            && sums.fft_len == m
            && sums.lanes == lane_count;
        if !fits {
            return Err(RadarDspError::Shape);
        }
        let d = self.plan.doppler_taps as f64;
        for group in 0..self.plan.groups() {
            if !self.plan.estimation(group).contains(&batch) {
                continue;
            }
            let cycles = self.plan.cycles(group, batch);
            for shift in 0..sums.shifts {
                let turn = rotation((shift as f64 - 2.0 * d) * cycles);
                let at = (group * sums.shifts + shift) * m;
                add_rotated(&mut sums.shared[at..at + m], &shared[..m], turn);
            }
            for lane in 0..lane_count {
                let source = &lanes[lane * m..(lane + 1) * m];
                for tap in 0..sums.taps {
                    let turn = rotation(-(tap as f64 - d) * cycles);
                    let at = ((group * lane_count + lane) * sums.taps + tap) * m;
                    add_rotated(&mut sums.cross[at..at + m], source, turn);
                }
                sums.energy[group * lane_count + lane] += energy[lane];
            }
        }
        Ok(())
    }

    pub fn solve(
        &mut self,
        sums: &GroupSums,
        table: &mut WeightTable,
    ) -> Result<SolveStats, RadarDspError> {
        let mut stats = SolveStats::default();
        let groups = self.plan.groups();
        let lanes = self.shape.lanes;
        let fits = sums.groups == groups
            && sums.lanes == lanes
            && sums.fft_len == self.shape.fft_len
            && table.lanes == lanes
            && table.taps == self.plan.taps()
            && table.order == self.shape.order()
            && table.fft_len == self.shape.fft_len
            && table.blends.len() == self.shape.batches
            && table.spectra.len() == groups * lanes * self.plan.taps() * self.shape.fft_len;
        if !fits {
            table.active = false;
            return Err(RadarDspError::Shape);
        }
        let mut totals = [0.0f64; MAX_SURVEILLANCE];
        for group in 0..groups {
            self.load_lags(sums, group);
            self.build_gram();
            if self.factor_loaded() {
                for (lane, total) in totals.iter_mut().enumerate().take(lanes) {
                    *total += f64::from(self.solve_lane(sums, table, group, lane));
                }
            } else {
                stats.unsuppressed_groups += 1;
                for lane in 0..lanes {
                    table.weights_mut(group, lane).fill(C64::default());
                    for tap in 0..self.plan.taps() {
                        table.spectrum_mut(group, lane, tap).fill(C32::default());
                    }
                }
            }
        }
        table.active = true;
        for (out, total) in stats.suppression_db.iter_mut().zip(totals).take(lanes) {
            *out = (total / groups as f64) as f32;
        }
        Ok(stats)
    }

    fn inverse_into_buffer(&mut self, source: &[C64]) {
        self.buffer.copy_from_slice(source);
        self.inverse
            .process_with_scratch(&mut self.buffer, &mut self.fft_scratch);
        let scale = 1.0 / self.shape.fft_len as f64;
        for value in &mut self.buffer {
            *value *= scale;
        }
    }

    fn load_lags(&mut self, sums: &GroupSums, group: usize) {
        let span = 2 * self.shape.order() - 1;
        for shift in 0..sums.shifts {
            self.inverse_into_buffer(sums.shared(group, shift));
            self.lags[shift * span..(shift + 1) * span].copy_from_slice(&self.buffer[..span]);
        }
    }

    fn build_gram(&mut self) {
        let order = self.shape.order();
        let span = 2 * order - 1;
        let n = self.unknowns;
        for row in 0..n {
            let (i, k) = (row / order, row % order);
            for col in row..n {
                let (j, l) = (col / order, col % order);
                let shift = j + 2 * self.plan.doppler_taps - i;
                let value = self.lags[shift * span + k + order - 1 - l];
                self.gram[row * n + col] = value;
                self.gram[col * n + row] = value.conj();
            }
        }
    }

    fn factor_loaded(&mut self) -> bool {
        let n = self.unknowns;
        let mean = (0..n).map(|i| self.gram[i * n + i].re).sum::<f64>() / n as f64;
        let base = self.loading * mean;
        for loading in [base, RETRY_LOADING * base] {
            if cholesky(&self.gram, n, loading, &mut self.factor) {
                return true;
            }
        }
        false
    }

    fn solve_lane(
        &mut self,
        sums: &GroupSums,
        table: &mut WeightTable,
        group: usize,
        lane: usize,
    ) -> f32 {
        let order = self.shape.order();
        let lead = self.shape.lead;
        let taps = self.plan.taps();
        for tap in 0..taps {
            self.inverse_into_buffer(sums.cross(group, lane, tap));
            for k in 0..order {
                self.rhs[tap * order + k] = self.buffer[k];
            }
        }
        substitute(&self.factor, self.unknowns, &self.rhs, &mut self.solution);
        let explained: f64 = self
            .solution
            .iter()
            .zip(&self.rhs)
            .map(|(w, c)| (w.conj() * c).re)
            .sum();
        table
            .weights_mut(group, lane)
            .copy_from_slice(&self.solution);
        for tap in 0..taps {
            self.buffer.fill(C64::default());
            let m = self.shape.fft_len;
            for k in 0..order {
                let at = (k + m + 1 - self.shape.taps - lead) % m;
                self.buffer[at] = self.solution[tap * order + k];
            }
            self.forward
                .process_with_scratch(&mut self.buffer, &mut self.fft_scratch);
            for (out, value) in table
                .spectrum_mut(group, lane, tap)
                .iter_mut()
                .zip(&self.buffer)
            {
                *out = C32::new(value.re as f32, value.im as f32);
            }
        }
        let energy = sums.energy[group * self.shape.lanes + lane];
        suppression_db(energy, energy - explained)
    }
}

fn rotation(turns: f64) -> C64 {
    C64::from_polar(1.0, TAU * turns.rem_euclid(1.0))
}

fn add_rotated(sum: &mut [C64], source: &[C32], turn: C64) {
    if turn == C64::new(1.0, 0.0) {
        for (out, value) in sum.iter_mut().zip(source) {
            *out += C64::new(f64::from(value.re), f64::from(value.im));
        }
    } else {
        for (out, value) in sum.iter_mut().zip(source) {
            *out += turn * C64::new(f64::from(value.re), f64::from(value.im));
        }
    }
}

fn suppression_db(energy: f64, residual: f64) -> f32 {
    if !(energy > 0.0 && energy.is_finite() && residual.is_finite()) {
        return 0.0;
    }
    (10.0 * (energy / residual.max(RESIDUAL_FLOOR * energy)).log10()) as f32
}

fn cholesky(matrix: &[C64], n: usize, loading: f64, factor: &mut [C64]) -> bool {
    if !(loading.is_finite() && loading >= 0.0) {
        return false;
    }
    factor[..n * n].fill(C64::default());
    for i in 0..n {
        for j in 0..=i {
            let mut sum = matrix[i * n + j];
            if i == j {
                sum += loading;
            }
            for k in 0..j {
                sum -= factor[i * n + k] * factor[j * n + k].conj();
            }
            if i == j {
                if !(sum.re > 0.0 && sum.re.is_finite()) {
                    return false;
                }
                factor[i * n + i] = C64::new(sum.re.sqrt(), 0.0);
            } else {
                factor[i * n + j] = sum / factor[j * n + j].re;
            }
        }
    }
    true
}

fn substitute(factor: &[C64], n: usize, rhs: &[C64], out: &mut [C64]) {
    for i in 0..n {
        let mut sum = rhs[i];
        for k in 0..i {
            sum -= factor[i * n + k] * out[k];
        }
        out[i] = sum / factor[i * n + i].re;
    }
    for i in (0..n).rev() {
        let mut sum = out[i];
        for k in i + 1..n {
            sum -= factor[k * n + i].conj() * out[k];
        }
        out[i] = sum / factor[i * n + i].re;
    }
}

#[cfg(test)]
mod tests;
