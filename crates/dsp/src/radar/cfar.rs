use std::ops::Range;

use num_complex::Complex;

use super::RadarDspError;
use super::threshold::{CfarStatistic, MAX_ALPHA, ThresholdError, alpha, os_order};

pub const MAX_GUARD: usize = 16;
pub const MAX_TRAIN_RANGE: usize = 64;
pub const MAX_TRAIN_DOPPLER: usize = 16;
pub const MAX_LANES: usize = 15;
pub const RHO_TABLE: [f64; 14] = [
    1.0, 1.33, 1.6, 2.0, 2.5, 3.2, 4.0, 5.0, 6.3, 7.0, 8.0, 10.0, 12.6, 14.0,
];
pub const CORRELATION_LAGS: usize = 3;

const MAX_PAD: usize = MAX_GUARD + MAX_TRAIN_DOPPLER;
const CLUTTER_FILL: f32 = 1.0;
const LOOKS_TOLERANCE: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CfarSpec {
    pub stat: CfarStatistic,
    pub plane: bool,
    pub guard_range: usize,
    pub train_range: usize,
    pub guard_doppler: usize,
    pub train_doppler: usize,
    pub alpha: f32,
    pub alpha_edge: f32,
    pub min_snr: f32,
    pub min_gate: usize,
    pub clutter_half_rows: usize,
}

impl CfarSpec {
    #[must_use]
    pub fn valid(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        let rank_ok = match self.stat {
            CfarStatistic::Os { rank } => rank > 0.0 && rank <= 1.0,
            CfarStatistic::Ca | CfarStatistic::Go => true,
        };
        rank_ok
            && positive(self.alpha)
            && positive(self.alpha_edge)
            && self.min_snr.is_finite()
            && self.min_snr >= 0.0
            && (1..=MAX_TRAIN_RANGE).contains(&self.train_range)
            && self.guard_range <= MAX_GUARD
            && self.guard_doppler <= MAX_GUARD
            && self.train_doppler <= MAX_TRAIN_DOPPLER
            && (!self.plane || self.train_doppler >= 1)
    }

    #[must_use]
    pub const fn cells(&self) -> usize {
        if self.plane {
            let outer = (2 * (self.guard_range + self.train_range) + 1)
                * (2 * (self.guard_doppler + self.train_doppler) + 1);
            outer - (2 * self.guard_range + 1) * (2 * self.guard_doppler + 1)
        } else {
            2 * self.train_range
        }
    }

    #[must_use]
    pub const fn statistic_cells(&self) -> usize {
        match self.stat {
            CfarStatistic::Go if self.plane => 2 * self.train_range * (2 * self.pad() + 1),
            _ => self.cells(),
        }
    }

    #[must_use]
    pub const fn edge_cells(&self) -> usize {
        if self.plane {
            self.cells()
        } else {
            2 * self.train_range
        }
    }

    const fn pad(&self) -> usize {
        if self.plane {
            self.guard_doppler + self.train_doppler
        } else {
            0
        }
    }

    const fn is_clutter(&self, row: usize, rows: usize) -> bool {
        row.abs_diff(rows / 2) <= self.clutter_half_rows
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hit {
    pub row: u32,
    pub gate: u32,
    pub power: f32,
    pub noise: f32,
}

pub(crate) struct Strongest {
    capacity: usize,
    heaped: bool,
    dropped: usize,
}

impl Strongest {
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            heaped: false,
            dropped: 0,
        }
    }

    pub(crate) fn offer<T: Copy>(&mut self, out: &mut Vec<T>, item: T, key: impl Fn(&T) -> f32) {
        if out.len() < self.capacity {
            out.push(item);
            return;
        }
        self.dropped += 1;
        if out.is_empty() {
            return;
        }
        if !self.heaped {
            for index in (0..out.len() / 2).rev() {
                sift_down(out, index, &key);
            }
            self.heaped = true;
        }
        if key(&item) > key(&out[0]) {
            out[0] = item;
            sift_down(out, 0, &key);
        }
    }

    pub(crate) const fn dropped(&self) -> usize {
        self.dropped
    }
}

fn sift_down<T>(heap: &mut [T], mut index: usize, key: &impl Fn(&T) -> f32) {
    loop {
        let left = 2 * index + 1;
        let right = left + 1;
        let mut smallest = index;
        if left < heap.len() && key(&heap[left]) < key(&heap[smallest]) {
            smallest = left;
        }
        if right < heap.len() && key(&heap[right]) < key(&heap[smallest]) {
            smallest = right;
        }
        if smallest == index {
            return;
        }
        heap.swap(index, smallest);
        index = smallest;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    len: usize,
}

impl Span {
    const fn end(self) -> usize {
        self.start + self.len
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    left: Span,
    right: Span,
    edge: bool,
}

fn range_window(gate: usize, gates: usize, guard: usize, train: usize) -> Window {
    let left_avail = gate.saturating_sub(guard);
    let right_first = gate + guard + 1;
    let right_avail = gates.saturating_sub(right_first);
    let mut left = train.min(left_avail);
    let mut right = train.min(right_avail);
    if left < train {
        right = right_avail.min(2 * train - left);
    }
    if right < train {
        left = left_avail.min(2 * train - right);
    }
    Window {
        left: Span {
            start: left_avail - left,
            len: left,
        },
        right: Span {
            start: right_first.min(gates),
            len: right,
        },
        edge: left < train || right < train,
    }
}

pub struct Cfar {
    spec: CfarSpec,
    gates: usize,
    rows: usize,
    prefix: Vec<f64>,
    table: Vec<f64>,
    scratch: Vec<f32>,
}

impl Cfar {
    pub fn new(spec: CfarSpec, gates: usize, rows: usize) -> Result<Self, RadarDspError> {
        if !spec.valid() || gates == 0 || rows == 0 {
            return Err(RadarDspError::Setting);
        }
        let widest = CfarSpec {
            plane: true,
            guard_range: MAX_GUARD,
            train_range: MAX_TRAIN_RANGE,
            guard_doppler: MAX_GUARD,
            train_doppler: MAX_TRAIN_DOPPLER,
            ..spec
        };
        Ok(Self {
            spec,
            gates,
            rows,
            prefix: vec![0.0; gates + 1],
            table: vec![0.0; (rows + 2 * MAX_PAD + 1) * (gates + 1)],
            scratch: Vec::with_capacity(widest.cells()),
        })
    }

    pub fn set_spec(&mut self, spec: CfarSpec) -> Result<(), RadarDspError> {
        if !spec.valid() {
            return Err(RadarDspError::Setting);
        }
        self.spec = spec;
        Ok(())
    }

    #[must_use]
    pub const fn spec(&self) -> CfarSpec {
        self.spec
    }

    pub fn detect(
        &mut self,
        power: &[f32],
        report_rows: Range<usize>,
        out: &mut Vec<Hit>,
        capacity: usize,
    ) -> Result<usize, RadarDspError> {
        out.clear();
        if power.len() < self.rows * self.gates {
            return Err(RadarDspError::Shape);
        }
        out.reserve(capacity);
        let mut keep = Strongest::new(capacity);
        if self.spec.plane {
            self.build_table(power);
        }
        let rows = report_rows.start.min(self.rows)..report_rows.end.min(self.rows);
        for row in rows {
            if self.spec.is_clutter(row, self.rows) {
                continue;
            }
            let line = &power[row * self.gates..(row + 1) * self.gates];
            if !self.spec.plane {
                fill_prefix(&mut self.prefix, line);
            }
            for gate in self.spec.min_gate..self.gates {
                let (noise, factor) = if self.spec.plane {
                    self.plane_noise(power, row, gate)
                } else {
                    self.range_noise(line, gate)
                };
                let cell = line[gate];
                if noise > 0.0 && cell > factor * noise && cell >= self.spec.min_snr * noise {
                    let hit = Hit {
                        row: row as u32,
                        gate: gate as u32,
                        power: cell,
                        noise,
                    };
                    keep.offer(out, hit, |hit| hit.power);
                }
            }
        }
        out.sort_unstable_by_key(|hit| (hit.row, hit.gate));
        Ok(keep.dropped())
    }

    fn range_noise(&mut self, line: &[f32], gate: usize) -> (f32, f32) {
        let spec = self.spec;
        let window = range_window(gate, self.gates, spec.guard_range, spec.train_range);
        let sum = |span: Span| self.prefix[span.end()] - self.prefix[span.start];
        let total = (sum(window.left) + sum(window.right)) as f32;
        let count = window.left.len + window.right.len;
        if count == 0 {
            return (0.0, spec.alpha);
        }
        let mean = total / count as f32;
        match spec.stat {
            CfarStatistic::Ca => (mean, spec.alpha),
            _ if window.edge => (mean, spec.alpha_edge),
            CfarStatistic::Go => {
                let left = sum(window.left) as f32 / window.left.len as f32;
                let right = sum(window.right) as f32 / window.right.len as f32;
                (left.max(right), spec.alpha)
            }
            CfarStatistic::Os { rank } => {
                self.scratch.clear();
                for span in [window.left, window.right] {
                    self.scratch
                        .extend_from_slice(&line[span.start..span.end()]);
                }
                (order_statistic(&mut self.scratch, rank), spec.alpha)
            }
        }
    }

    fn build_table(&mut self, power: &[f32]) {
        let (rows, gates) = (self.rows, self.gates);
        let pad = self.spec.pad();
        let stride = gates + 1;
        let padded = rows + 2 * pad;
        self.table[..stride].fill(0.0);
        for padded_row in 0..padded {
            let row = (padded_row as i64 - pad as i64).rem_euclid(rows as i64) as usize;
            let clutter = self.spec.is_clutter(row, rows);
            let line = &power[row * gates..(row + 1) * gates];
            let (above, here) = self.table.split_at_mut((padded_row + 1) * stride);
            let above = &above[padded_row * stride..];
            let here = &mut here[..stride];
            here[0] = 0.0;
            let mut running = 0.0f64;
            for gate in 0..gates {
                let value = if clutter { CLUTTER_FILL } else { line[gate] };
                running += f64::from(value);
                here[gate + 1] = above[gate + 1] + running;
            }
        }
    }

    fn rect(&self, rows: Range<usize>, gates: Range<usize>) -> f64 {
        let stride = self.gates + 1;
        let at = |row: usize, gate: usize| self.table[row * stride + gate];
        at(rows.end, gates.end) - at(rows.start, gates.end) - at(rows.end, gates.start)
            + at(rows.start, gates.start)
    }

    fn plane_noise(&mut self, power: &[f32], row: usize, gate: usize) -> (f32, f32) {
        let spec = self.spec;
        let pad = spec.pad();
        let outer_rows = row..row + 2 * pad + 1;
        let guard_rows = row + pad - spec.guard_doppler..row + pad + spec.guard_doppler + 1;
        let reach = spec.guard_range + spec.train_range;
        let width = (2 * reach + 1).min(self.gates);
        let start = gate.saturating_sub(reach).min(self.gates - width);
        let outer = start..start + width;
        let guard = gate.saturating_sub(spec.guard_range).max(start)
            ..(gate + spec.guard_range + 1).min(outer.end);
        let count = outer_rows.len() * outer.len() - guard_rows.len() * guard.len();
        if count == 0 {
            return (0.0, spec.alpha);
        }
        let total = self.rect(outer_rows.clone(), outer.clone())
            - self.rect(guard_rows.clone(), guard.clone());
        let mean = (total / count as f64) as f32;
        let left = outer.start..guard.start;
        let right = guard.end..outer.end;
        let edge = left.len() < spec.train_range || right.len() < spec.train_range;
        match spec.stat {
            CfarStatistic::Ca => (mean, spec.alpha),
            _ if edge => (mean, spec.alpha_edge),
            CfarStatistic::Go => {
                let side = |gates: Range<usize>| {
                    (self.rect(outer_rows.clone(), gates.clone())
                        / (outer_rows.len() * gates.len()) as f64) as f32
                };
                (side(left.clone()).max(side(right.clone())), spec.alpha)
            }
            CfarStatistic::Os { rank } => {
                self.gather_plane(power, row, &outer, &guard);
                (order_statistic(&mut self.scratch, rank), spec.alpha)
            }
        }
    }

    fn gather_plane(
        &mut self,
        power: &[f32],
        row: usize,
        outer: &Range<usize>,
        guard: &Range<usize>,
    ) {
        let spec = self.spec;
        let pad = spec.pad() as i64;
        let guard_doppler = spec.guard_doppler as i64;
        self.scratch.clear();
        for offset in -pad..=pad {
            let source = (row as i64 + offset).rem_euclid(self.rows as i64) as usize;
            let clutter = spec.is_clutter(source, self.rows);
            let line = &power[source * self.gates..(source + 1) * self.gates];
            for gate in outer.clone() {
                if offset.abs() <= guard_doppler && guard.contains(&gate) {
                    continue;
                }
                self.scratch
                    .push(if clutter { CLUTTER_FILL } else { line[gate] });
            }
        }
    }
}

fn fill_prefix(prefix: &mut [f64], line: &[f32]) {
    prefix[0] = 0.0;
    let mut running = 0.0f64;
    for (slot, value) in prefix[1..].iter_mut().zip(line) {
        running += f64::from(*value);
        *slot = running;
    }
}

fn order_statistic(cells: &mut [f32], rank: f32) -> f32 {
    if cells.is_empty() {
        return 0.0;
    }
    let order = os_order(rank, cells.len()) - 1;
    let (_, value, _) = cells.select_nth_unstable_by(order, f32::total_cmp);
    *value
}

pub struct AlphaTable {
    looks: usize,
    alpha: Vec<f32>,
    edge: Vec<f32>,
}

impl AlphaTable {
    pub fn new(
        stat: CfarStatistic,
        cells: usize,
        edge_cells: usize,
        pfa: f64,
        max_looks: u32,
    ) -> Result<Self, ThresholdError> {
        let looks = max_looks.max(1);
        let mut table = Self {
            looks: looks as usize,
            alpha: Vec::with_capacity(looks as usize * RHO_TABLE.len()),
            edge: Vec::with_capacity(looks as usize * RHO_TABLE.len()),
        };
        for look in 1..=looks {
            for rho in RHO_TABLE {
                table.alpha.push(design(stat, cells, pfa, look, rho)?);
                table
                    .edge
                    .push(design(CfarStatistic::Ca, edge_cells, pfa, look, rho)?);
            }
        }
        Ok(table)
    }

    #[must_use]
    pub fn pick(&self, looks: u32, correlation: f64) -> (f32, f32) {
        let look = (looks.max(1) as usize).min(self.looks) - 1;
        let row = look * RHO_TABLE.len();
        let column = RHO_TABLE
            .iter()
            .position(|&rho| rho >= correlation)
            .unwrap_or(RHO_TABLE.len() - 1);
        (self.alpha[row + column], self.edge[row + column])
    }
}

fn design(
    stat: CfarStatistic,
    cells: usize,
    pfa: f64,
    looks: u32,
    correlation: f64,
) -> Result<f32, ThresholdError> {
    match alpha(stat, cells, pfa, looks, correlation) {
        Ok(value) => Ok(value as f32),
        Err(ThresholdError::Unreachable(_)) if correlation > 1.0 => Ok(MAX_ALPHA as f32),
        Err(error) => Err(error),
    }
}

#[must_use]
pub fn range_correlation(autocorrelation: &[Complex<f32>], run: usize) -> f64 {
    let Some(zero) = autocorrelation.first().map(|value| f64::from(value.norm())) else {
        return 1.0;
    };
    if !(zero > 0.0 && zero.is_finite() && run > 0) {
        return 1.0;
    }
    let tail: f64 = autocorrelation
        .iter()
        .enumerate()
        .skip(1)
        .take(CORRELATION_LAGS)
        .map(|(lag, value)| {
            let ratio = f64::from(value.norm()) / zero;
            let weight = (1.0 - lag as f64 / run as f64).max(0.0);
            weight * ratio * ratio
        })
        .sum();
    if tail.is_finite() {
        1.0 + 2.0 * tail
    } else {
        1.0
    }
}

pub struct LaneCoherence {
    lanes: usize,
    samples: usize,
    cross: [[Complex<f64>; MAX_LANES]; MAX_LANES],
}

impl LaneCoherence {
    pub fn new(lanes: usize) -> Result<Self, RadarDspError> {
        if !(1..=MAX_LANES).contains(&lanes) {
            return Err(RadarDspError::Setting);
        }
        Ok(Self {
            lanes,
            samples: 0,
            cross: [[Complex::new(0.0, 0.0); MAX_LANES]; MAX_LANES],
        })
    }

    pub fn clear(&mut self) {
        self.samples = 0;
        self.cross = [[Complex::new(0.0, 0.0); MAX_LANES]; MAX_LANES];
    }

    pub fn add(&mut self, snapshot: &[Complex<f32>]) {
        self.samples += 1;
        let lanes = self.lanes.min(snapshot.len());
        let widen = |value: &Complex<f32>| Complex::new(f64::from(value.re), f64::from(value.im));
        for (k, first) in snapshot[..lanes].iter().enumerate() {
            let a = widen(first);
            for (cell, second) in self.cross[k][k..lanes].iter_mut().zip(&snapshot[k..lanes]) {
                *cell += a * widen(second).conj();
            }
        }
    }

    #[must_use]
    pub fn looks(&self) -> u32 {
        let lanes = self.lanes;
        let bias = 1.0 / self.samples.max(1) as f64;
        let mut frobenius = lanes as f64;
        for k in 0..lanes {
            for j in k + 1..lanes {
                let norm = self.cross[k][k].re * self.cross[j][j].re;
                if norm > 0.0 {
                    let squared = self.cross[k][j].norm_sqr() / norm;
                    frobenius += 2.0 * (squared - bias).max(0.0);
                }
            }
        }
        if !frobenius.is_finite() {
            return 1;
        }
        let ratio = (lanes * lanes) as f64 / frobenius;
        ((ratio + LOOKS_TOLERANCE).floor() as u32).clamp(1, lanes as u32)
    }
}

#[cfg(test)]
mod tests;
