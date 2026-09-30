use super::{
    grid::{CELLS, LogGrid, SHIFT_STEP, Wedge, look},
    observation::{BANKS, FLOOR, Observation, RING, Ring, Rings, blur},
};

pub(crate) const VOTE_CELLS: usize = CELLS / SHIFT_STEP;
pub(crate) const MAX_KEYS: usize = 64;
pub(crate) const KEY_BIN_M: f64 = 500.0;
pub(crate) const VOTE_MIN: f32 = 1.6;
pub(crate) const VOTE_SUPPORT: f32 = 0.2;
pub(crate) const MIN_KEY_WEIGHT: f32 = 0.05;
pub(crate) const BIAS_SIGMA_DEG: f32 = 2.5;
pub(crate) const MAX_POWER: f32 = 2.0;

const VOTE_SEPARATION_CELLS: f64 = 4.0;
const TIE_BREAK: f32 = 1e-3;
const PAINT_TOLERANCE: f32 = 5e-3;
const POWER_TOLERANCE: f32 = 0.02;
const BIAS_TOLERANCE_DEG: f32 = 0.05;

pub(crate) struct Painted {
    pub(crate) wedge: Wedge,
    mean: Box<Rings>,
    pub(crate) power: f32,
    bias_sigma_deg: f32,
}

pub(crate) struct VoteKey {
    pub(crate) station: String,
    pub(crate) bin: (i32, i32),
    evidence: Box<Rings>,
    linear: Box<Rings>,
    weight: f32,
    clock: f64,
    pub(crate) east_m: f64,
    pub(crate) north_m: f64,
    pub(crate) accuracy_m: f32,
    heading_sigma_deg: f32,
    pub(crate) painted: Option<Painted>,
}

fn decay_factor(from: f64, to: f64) -> f32 {
    0.5f64.powf((to - from).max(0.0)) as f32
}

fn scale(rings: &mut Rings, factor: f32) {
    for value in rings.iter_mut().flatten() {
        *value *= factor;
    }
}

impl VoteKey {
    fn new(station: &str, bin: (i32, i32), clock: f64) -> Self {
        Self {
            station: station.to_owned(),
            bin,
            evidence: Box::new([[0.0; RING]; BANKS]),
            linear: Box::new([[0.0; RING]; BANKS]),
            weight: 0.0,
            clock,
            east_m: 0.0,
            north_m: 0.0,
            accuracy_m: 0.0,
            heading_sigma_deg: 0.0,
            painted: None,
        }
    }

    pub(crate) fn weight_at(&self, clock: f64) -> f32 {
        self.weight * decay_factor(self.clock, clock)
    }

    pub(crate) fn power_at(&self, clock: f64) -> f32 {
        self.weight_at(clock).min(MAX_POWER)
    }

    fn absorb(&mut self, observation: &Observation, at: (f64, f64), weight: f32, clock: f64) {
        let factor = decay_factor(self.clock, clock);
        scale(&mut self.evidence, factor);
        scale(&mut self.linear, factor);
        let before = self.weight * factor;
        let after = before + weight;
        let blend = |old: f64, new: f64| {
            if after > 0.0 {
                (old * f64::from(before) + new * f64::from(weight)) / f64::from(after)
            } else {
                new
            }
        };
        self.east_m = blend(self.east_m, at.0);
        self.north_m = blend(self.north_m, at.1);
        self.accuracy_m = blend(
            f64::from(self.accuracy_m),
            f64::from(observation.accuracy_m),
        ) as f32;
        self.heading_sigma_deg = blend(
            f64::from(self.heading_sigma_deg),
            f64::from(observation.heading_sigma_deg),
        ) as f32;
        for ((evidence, linear), bank) in self
            .evidence
            .iter_mut()
            .zip(self.linear.iter_mut())
            .zip(observation.bank.iter())
        {
            for ((sum, linear), value) in evidence.iter_mut().zip(linear.iter_mut()).zip(bank) {
                *sum += weight * value;
                *linear += weight * value.exp();
            }
        }
        self.weight = after;
        self.clock = clock;
    }

    fn mean(&self, sums: &Rings) -> Box<Rings> {
        let mut mean = Box::new(*sums);
        scale(&mut mean, self.weight.max(f32::MIN_POSITIVE).recip());
        mean
    }

    fn bias_sigma_deg(&self) -> f32 {
        BIAS_SIGMA_DEG.hypot(self.heading_sigma_deg)
    }

    fn wants_repaint(&self, mean: &Rings, power: f32, bias: f32) -> bool {
        let Some(painted) = &self.painted else {
            return true;
        };
        let moved =
            (painted.wedge.east_m - self.east_m).hypot(painted.wedge.north_m - self.north_m) >= 1.0
                || (painted.wedge.accuracy_m - self.accuracy_m).abs() >= 1.0;
        let reweighed =
            (painted.power - power).abs() > POWER_TOLERANCE * painted.power.max(POWER_TOLERANCE);
        let rebiased = (painted.bias_sigma_deg - bias).abs() > BIAS_TOLERANCE_DEG;
        let reshaped = painted
            .mean
            .iter()
            .flatten()
            .zip(mean.iter().flatten())
            .any(|(was, now)| (was - now).abs() * power > PAINT_TOLERANCE);
        moved || reweighed || rebiased || reshaped
    }

    pub(crate) fn refresh(&mut self, clock: f64, force: bool) -> Option<Option<Painted>> {
        let power = self.power_at(clock);
        let bias = self.bias_sigma_deg();
        let mean = self.mean(&self.evidence);
        if !force && !self.wants_repaint(&mean, power, bias) {
            return None;
        }
        let wedge = self.wedge(&mean, power, bias);
        Some(self.painted.replace(Painted {
            wedge,
            mean,
            power,
            bias_sigma_deg: bias,
        }))
    }

    fn wedge(&self, mean: &Rings, power: f32, bias: f32) -> Wedge {
        let mut ring = Box::new([[0.0; RING]; BANKS]);
        let mut raised = [0.0f32; RING];
        let mut spread = [0.0f32; RING];
        for (out, bank) in ring.iter_mut().zip(mean.iter()) {
            for (raised, value) in raised.iter_mut().zip(bank) {
                *raised = (power * value).exp();
            }
            blur(&raised, bias, &mut spread);
            for (out, value) in out.iter_mut().zip(&spread) {
                *out = value.max(f32::MIN_POSITIVE).ln();
            }
        }
        let widest = &ring[BANKS - 1];
        let centre_value = widest.iter().sum::<f32>() / RING as f32;
        Wedge {
            ring,
            centre_value,
            east_m: self.east_m,
            north_m: self.north_m,
            accuracy_m: self.accuracy_m,
        }
    }

    fn support(&self) -> Box<Rings> {
        let mut support = self.mean(&self.linear);
        for value in support.iter_mut().flatten() {
            *value = (((*value - FLOOR) / (1.0 - FLOOR)).max(0.0) / VOTE_SUPPORT).min(1.0);
        }
        support
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) east_m: f64,
    pub(crate) north_m: f64,
    pub(crate) votes: f32,
}

#[derive(Default)]
pub(crate) struct VoteMaps {
    pub(crate) keys: Vec<VoteKey>,
}

pub(crate) struct Added {
    pub(crate) index: usize,
    pub(crate) evicted: Option<VoteKey>,
}

impl VoteMaps {
    pub(crate) fn bin_of(east_m: f64, north_m: f64) -> (i32, i32) {
        let bin = |metres: f64| {
            (metres / KEY_BIN_M)
                .floor()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
        };
        (bin(east_m), bin(north_m))
    }

    pub(crate) fn add(
        &mut self,
        observation: &Observation,
        at: (f64, f64),
        weight: f32,
        clock: f64,
    ) -> Added {
        let bin = Self::bin_of(at.0, at.1);
        let found = self
            .keys
            .iter()
            .position(|key| key.bin == bin && key.station == observation.station);
        let (index, evicted) = match found {
            Some(index) => (index, None),
            None => {
                let evicted = (self.keys.len() >= MAX_KEYS)
                    .then(|| self.weakest(clock))
                    .flatten()
                    .map(|weakest| self.keys.swap_remove(weakest));
                self.keys
                    .push(VoteKey::new(&observation.station, bin, clock));
                (self.keys.len() - 1, evicted)
            }
        };
        self.keys[index].absorb(observation, at, weight, clock);
        Added { index, evicted }
    }

    fn weakest(&self, clock: f64) -> Option<usize> {
        self.keys
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.weight_at(clock).total_cmp(&b.1.weight_at(clock)))
            .map(|(index, _)| index)
    }

    pub(crate) fn retire(&mut self, clock: f64) -> Vec<VoteKey> {
        let mut retired = Vec::new();
        let mut index = 0;
        while index < self.keys.len() {
            if self.keys[index].weight_at(clock) < MIN_KEY_WEIGHT {
                retired.push(self.keys.swap_remove(index));
            } else {
                index += 1;
            }
        }
        retired
    }

    pub(crate) fn candidates(&self, grid: &LogGrid, max: usize, out: &mut Vec<Candidate>) {
        out.clear();
        if self.keys.len() < 2 || max == 0 {
            return;
        }
        let supports: Vec<(Box<Rings>, f32, &VoteKey)> = self
            .keys
            .iter()
            .map(|key| {
                let support = key.support();
                let widest = widest_mean(&support);
                (support, widest, key)
            })
            .collect();
        let vote_m = grid.cell_m() * SHIFT_STEP as f64;
        let (_, top) = grid.max();
        let mut totals = vec![0.0f32; VOTE_CELLS * VOTE_CELLS];
        let mut scores = vec![f32::NEG_INFINITY; VOTE_CELLS * VOTE_CELLS];
        for vote_row in 0..VOTE_CELLS {
            for vote_col in 0..VOTE_CELLS {
                let (east, north) = vote_centre(grid, vote_row, vote_col);
                let total: f32 = supports
                    .iter()
                    .map(|(support, widest, key)| {
                        look(
                            east - key.east_m,
                            north - key.north_m,
                            key.accuracy_m,
                            vote_m,
                        )
                        .sample(support, *widest)
                    })
                    .sum();
                let index = vote_row * VOTE_CELLS + vote_col;
                totals[index] = total;
                scores[index] = total
                    + TIE_BREAK / (1.0 + (top - block_max(grid, vote_row, vote_col)).max(0.0));
            }
        }
        let mut peaks: Vec<(usize, f32)> = (0..VOTE_CELLS * VOTE_CELLS)
            .filter(|&index| totals[index] >= VOTE_MIN && is_local_max(&scores, index))
            .map(|index| (index, scores[index]))
            .collect();
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut kept: Vec<usize> = Vec::new();
        for (index, _) in peaks {
            if kept.len() == max {
                break;
            }
            if kept
                .iter()
                .all(|&other| cells_apart(index, other) >= VOTE_SEPARATION_CELLS)
            {
                kept.push(index);
            }
        }
        out.extend(kept.into_iter().map(|index| {
            let (east_m, north_m) = vote_centre(grid, index / VOTE_CELLS, index % VOTE_CELLS);
            Candidate {
                east_m,
                north_m,
                votes: totals[index],
            }
        }));
    }
}

fn widest_mean(support: &Rings) -> f32 {
    let widest: &Ring = &support[BANKS - 1];
    widest.iter().sum::<f32>() / RING as f32
}

fn vote_centre(grid: &LogGrid, vote_row: usize, vote_col: usize) -> (f64, f64) {
    let (west, north) = grid.cell_centre(vote_row * SHIFT_STEP, vote_col * SHIFT_STEP);
    let half = (SHIFT_STEP as f64 - 1.0) / 2.0 * grid.cell_m();
    (west + half, north - half)
}

fn block_max(grid: &LogGrid, vote_row: usize, vote_col: usize) -> f32 {
    let values = grid.values();
    let mut best = f32::NEG_INFINITY;
    for row in vote_row * SHIFT_STEP..(vote_row + 1) * SHIFT_STEP {
        for col in vote_col * SHIFT_STEP..(vote_col + 1) * SHIFT_STEP {
            best = best.max(values[row * CELLS + col]);
        }
    }
    best
}

fn is_local_max(scores: &[f32], index: usize) -> bool {
    let (row, col) = (index / VOTE_CELLS, index % VOTE_CELLS);
    let value = scores[index];
    for d_row in -1isize..=1 {
        for d_col in -1isize..=1 {
            if d_row == 0 && d_col == 0 {
                continue;
            }
            let (Some(r), Some(c)) = (row.checked_add_signed(d_row), col.checked_add_signed(d_col))
            else {
                continue;
            };
            if r < VOTE_CELLS && c < VOTE_CELLS && scores[r * VOTE_CELLS + c] > value {
                return false;
            }
        }
    }
    true
}

fn cells_apart(a: usize, b: usize) -> f64 {
    let rows = (a / VOTE_CELLS) as f64 - (b / VOTE_CELLS) as f64;
    let cols = (a % VOTE_CELLS) as f64 - (b % VOTE_CELLS) as f64;
    rows.hypot(cols)
}
