use std::f32::consts::PI;

use super::frames::Block;
use sdrmm_wire::frame::SymbolPlane;

pub const BASEBAND_GAIN: f32 = 0.22;
pub const BASEBAND_DECAY: f32 = 0.82;
pub const IQ_HEADROOM: f32 = 2.0;
pub const HISTOGRAM_BINS: usize = 96;
const EYE_STROKE: f32 = 0.35;
const EYE_WINDOWS: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<f32>,
}

impl Grid {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0.0; width * height],
        }
    }

    pub fn clear(&mut self) {
        self.cells.fill(0.0);
    }

    pub fn decay(&mut self, factor: f32) {
        for cell in &mut self.cells {
            let value = *cell * factor;
            *cell = if value < 0.002 { 0.0 } else { value };
        }
    }

    fn paint(&mut self, x: i64, y: i64, gain: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let at = y as usize * self.width + x as usize;
        self.cells[at] = (self.cells[at] + gain).min(1.0);
    }

    fn segment(&mut self, from: (f32, f32), to: (f32, f32), gain: f32) {
        let steps = (to.0 - from.0)
            .abs()
            .max((to.1 - from.1).abs())
            .ceil()
            .max(1.0) as usize;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let x = from.0 + (to.0 - from.0) * t;
            let y = from.1 + (to.1 - from.1) * t;
            self.paint(x.round() as i64, y.round() as i64, gain);
        }
    }
}

fn pairs(samples: &[f32]) -> usize {
    samples.len() / 2
}

#[must_use]
pub fn iq_scale(samples: &[f32]) -> f32 {
    let count = pairs(samples);
    if count == 0 {
        return 0.0;
    }
    let power: f32 = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[re, im]| re * re + im * im)
        .sum();
    (power / count as f32).sqrt() * IQ_HEADROOM
}

pub fn add_constellation(
    grid: &mut Grid,
    samples: &[f32],
    scale: f32,
    decimation: f32,
    offset: i64,
) {
    let span = if scale > 0.0 { scale } else { 1.0 };
    let half_w = (grid.width as f32 - 1.0) / 2.0;
    let half_h = (grid.height as f32 - 1.0) / 2.0;
    let step = decimation.floor().max(1.0) as i64;
    let start = offset.rem_euclid(step) as usize;
    for [re, im] in samples
        .as_chunks::<2>()
        .0
        .iter()
        .skip(start)
        .step_by(step as usize)
    {
        let x = (half_w + (re / span).clamp(-1.0, 1.0) * half_w).round() as i64;
        let y = (half_h - (im / span).clamp(-1.0, 1.0) * half_h).round() as i64;
        grid.paint(x, y, BASEBAND_GAIN);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    I,
    Q,
    Frequency,
}

impl Eye {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::Q => "Q",
            Self::Frequency => "freq",
        }
    }
}

fn rail(samples: &[f32], index: usize, eye: Eye) -> f32 {
    let re = samples.get(index * 2).copied().unwrap_or(0.0);
    let im = samples.get(index * 2 + 1).copied().unwrap_or(0.0);
    match eye {
        Eye::I => re,
        Eye::Q => im,
        Eye::Frequency if index == 0 => 0.0,
        Eye::Frequency => {
            let pr = samples.get(index * 2 - 2).copied().unwrap_or(0.0);
            let pi = samples.get(index * 2 - 1).copied().unwrap_or(0.0);
            (im * pr - re * pi).atan2(re * pr + im * pi) / PI
        }
    }
}

pub fn add_eye(grid: &mut Grid, samples: &[f32], period: f32, eye: Eye, scale: f32) {
    let span = if scale > 0.0 { scale } else { 1.0 };
    let count = pairs(samples);
    let width = (period.round().max(2.0) as usize) * 2;
    if count < width {
        return;
    }
    let half_h = (grid.height as f32 - 1.0) / 2.0;
    let hop = width / 2;
    let windows = (count - width) / hop + 1;
    let stroke = BASEBAND_GAIN * EYE_STROKE * (EYE_WINDOWS / windows as f32).sqrt().min(1.0);
    let mut start = 0;
    while start + width <= count {
        let mut last = (0.0, 0.0);
        for k in 0..width {
            let value = rail(samples, start + k, eye) / span;
            let x = k as f32 / (width - 1) as f32 * (grid.width as f32 - 1.0);
            let y = half_h - value.clamp(-1.0, 1.0) * half_h;
            if k == 0 {
                grid.paint(x.round() as i64, y.round() as i64, stroke);
            } else {
                grid.segment(last, (x, y), stroke);
            }
            last = (x, y);
        }
        start += hop;
    }
}

#[must_use]
pub fn eye_scale(samples: &[f32], eye: Eye) -> f32 {
    if eye == Eye::Frequency {
        return 1.0;
    }
    (0..pairs(samples))
        .map(|index| rail(samples, index, eye).abs())
        .fold(0.0, f32::max)
}

#[must_use]
pub fn samples_per_symbol(sample_rate: f32, symbol_rate: f32) -> f32 {
    if sample_rate > 0.0 && symbol_rate > 0.0 {
        sample_rate / symbol_rate
    } else {
        1.0
    }
}

#[must_use]
pub fn symbol_phase(samples: &[f32], period: f32) -> i64 {
    let count = pairs(samples);
    if period.is_nan() || period < 2.0 || (count as f32) < period {
        return 0;
    }
    let step = 2.0 * std::f64::consts::PI / f64::from(period);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (index, [a, b]) in samples.as_chunks::<2>().0.iter().enumerate() {
        let power = f64::from(a * a + b * b);
        let angle = step * index as f64;
        re += power * angle.cos();
        im += power * angle.sin();
    }
    if re == 0.0 && im == 0.0 {
        return 0;
    }
    let turns = im.atan2(re) / (2.0 * std::f64::consts::PI);
    let offset = (turns - turns.floor()) * f64::from(period);
    (offset.floor() as i64).min(f64::from(period).ceil() as i64 - 1)
}

#[must_use]
pub fn symbol_histogram(values: &[f32], stride: usize, scale: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; HISTOGRAM_BINS];
    let span = if scale > 0.0 { scale } else { 1.0 };
    for value in values.iter().step_by(stride.max(1)) {
        let unit = (value / span + 1.0) / 2.0;
        if !(0.0..=1.0).contains(&unit) {
            continue;
        }
        let at = ((unit * HISTOGRAM_BINS as f32).floor() as usize).min(HISTOGRAM_BINS - 1);
        out[at] += 1.0;
    }
    let peak = out.iter().copied().fold(0.0, f32::max);
    if peak > 0.0 {
        for bin in &mut out {
            *bin /= peak;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    values: Vec<f32>,
    at: usize,
    filled: usize,
}

impl Trend {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            values: vec![0.0; capacity.max(1)],
            at: 0,
            filled: 0,
        }
    }

    pub fn push(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        let capacity = self.values.len();
        self.values[self.at] = value;
        self.at = (self.at + 1) % capacity;
        self.filled = (self.filled + 1).min(capacity);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.filled
    }

    #[must_use]
    pub fn sample(&self, index: usize) -> f32 {
        if index >= self.filled {
            return 0.0;
        }
        let capacity = self.values.len();
        let from = (self.at + capacity - self.filled) % capacity;
        self.values[(from + index) % capacity]
    }

    #[must_use]
    pub fn range(&self) -> (f32, f32) {
        if self.filled == 0 {
            return (0.0, 1.0);
        }
        let (min, max) = (0..self.filled)
            .map(|index| self.sample(index))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), value| {
                (min.min(value), max.max(value))
            });
        if min == max {
            (min - 1.0, max + 1.0)
        } else {
            (min, max)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolState {
    pub bits: String,
    pub i: f32,
    pub q: f32,
    pub count: usize,
    pub share: f32,
    pub mean: f32,
    pub sigma: f32,
    pub peak: f32,
}

fn plane_width(block: &Block) -> usize {
    if block.plane == SymbolPlane::Complex {
        2
    } else {
        1
    }
}

fn axis(values: &[f32], index: usize, width: usize, part: usize) -> f32 {
    if part == 1 && width == 1 {
        return 0.0;
    }
    values.get(index * width + part).copied().unwrap_or(0.0)
}

fn nearest_point(reference: &[f32], width: usize, points: usize, x: f32, y: f32) -> usize {
    (0..points)
        .map(|k| {
            let dx = x - axis(reference, k, width, 0);
            let dy = y - axis(reference, k, width, 1);
            (k, dx * dx + dy * dy)
        })
        .fold((0, f32::INFINITY), |best, next| {
            if next.1 < best.1 { next } else { best }
        })
        .0
}

fn mean_power(values: &[f32], width: usize, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let sum: f32 = (0..count)
        .map(|k| {
            let a = axis(values, k, width, 0);
            let b = axis(values, k, width, 1);
            a * a + b * b
        })
        .sum();
    sum / count as f32
}

fn fit_scale(target: f32, measured: f32) -> f32 {
    if measured > 0.0 && target > 0.0 {
        (target / measured).sqrt()
    } else {
        1.0
    }
}

#[must_use]
pub fn symbol_gain(block: &Block) -> f32 {
    let width = plane_width(block);
    let points = block.reference.len() / width;
    let count = block.symbols.len() / width;
    if points == 0 || count == 0 {
        return 1.0;
    }
    let measured = mean_power(&block.symbols, width, count);
    let blind = fit_scale(mean_power(&block.reference, width, points), measured);
    let decided: f32 = (0..count)
        .map(|n| {
            let x = axis(&block.symbols, n, width, 0) * blind;
            let y = axis(&block.symbols, n, width, 1) * blind;
            let k = nearest_point(&block.reference, width, points, x, y);
            let a = axis(&block.reference, k, width, 0);
            let b = axis(&block.reference, k, width, 1);
            a * a + b * b
        })
        .sum();
    blind * fit_scale(decided / count as f32, measured * blind * blind)
}

#[must_use]
pub fn decision_distance(block: &Block) -> f32 {
    let width = plane_width(block);
    let points = block.reference.len() / width;
    let mut closest = f32::INFINITY;
    for a in 0..points {
        for b in a + 1..points {
            let dx = axis(&block.reference, a, width, 0) - axis(&block.reference, b, width, 0);
            let dy = axis(&block.reference, a, width, 1) - axis(&block.reference, b, width, 1);
            let distance = dx.hypot(dy);
            if distance > 0.0 {
                closest = closest.min(distance);
            }
        }
    }
    if closest.is_finite() {
        closest / 2.0
    } else {
        1.0
    }
}

#[must_use]
pub fn state_bits(index: usize, points: usize) -> String {
    if points.is_power_of_two() {
        let bits = points.trailing_zeros() as usize;
        format!("{index:0bits$b}")
    } else {
        index.to_string()
    }
}

#[derive(Clone, Copy, Default)]
struct Tally {
    count: usize,
    sum: f64,
    squares: f64,
    peak: f32,
}

#[must_use]
pub fn symbol_states(block: &Block) -> Vec<SymbolState> {
    let width = plane_width(block);
    let points = block.reference.len() / width;
    if points == 0 {
        return Vec::new();
    }
    let count = block.symbols.len() / width;
    let gain = symbol_gain(block);
    let half = decision_distance(block);
    let mut tallies = vec![Tally::default(); points];
    for n in 0..count {
        let x = axis(&block.symbols, n, width, 0) * gain;
        let y = axis(&block.symbols, n, width, 1) * gain;
        let k = nearest_point(&block.reference, width, points, x, y);
        let dx = x - axis(&block.reference, k, width, 0);
        let dy = y - axis(&block.reference, k, width, 1);
        let error = if width == 2 { dx.hypot(dy) } else { dx } / half;
        let tally = &mut tallies[k];
        tally.count += 1;
        tally.sum += f64::from(error);
        tally.squares += f64::from(error * error);
        if error.abs() > tally.peak.abs() {
            tally.peak = error;
        }
    }
    let mut states: Vec<SymbolState> = tallies
        .iter()
        .enumerate()
        .map(|(k, tally)| state_of(block, width, points, count, k, *tally))
        .collect();
    if width == 1 {
        states.sort_by(|a, b| b.i.total_cmp(&a.i));
    }
    states
}

fn state_of(
    block: &Block,
    width: usize,
    points: usize,
    count: usize,
    k: usize,
    tally: Tally,
) -> SymbolState {
    let hits = tally.count as f64;
    let mean = if tally.count > 0 {
        tally.sum / hits
    } else {
        f64::NAN
    };
    let sigma = if tally.count > 1 {
        (tally.squares / hits - mean * mean).max(0.0).sqrt()
    } else {
        f64::NAN
    };
    SymbolState {
        bits: state_bits(k, points),
        i: axis(&block.reference, k, width, 0),
        q: axis(&block.reference, k, width, 1),
        count: tally.count,
        share: if count > 0 {
            tally.count as f32 / count as f32
        } else {
            0.0
        },
        mean: mean as f32,
        sigma: sigma as f32,
        peak: if tally.count > 0 {
            tally.peak
        } else {
            f32::NAN
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(grid: &Grid) -> Vec<(usize, usize)> {
        let mut hits = Vec::new();
        for y in 0..grid.height {
            for x in 0..grid.width {
                if grid.cells[y * grid.width + x] > 0.0 {
                    hits.push((x, y));
                }
            }
        }
        hits
    }

    fn iq(pairs: &[(f32, f32)]) -> Vec<f32> {
        pairs.iter().flat_map(|(re, im)| [*re, *im]).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn scales_by_rms_so_one_spike_does_not_shrink_the_plot() {
        let quiet = iq_scale(&iq(&[(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)]));
        let spiked = iq_scale(&iq(&[
            (1.0, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (0.0, -1.0),
            (0.0, 0.0),
            (0.0, 0.0),
            (0.0, 0.0),
            (3.0, 0.0),
        ]));
        assert!(close(quiet, IQ_HEADROOM));
        assert!(close(spiked, IQ_HEADROOM * (13.0f32 / 8.0).sqrt()));
        assert!(spiked < 3.0);
        assert_eq!(iq_scale(&[]), 0.0);
        assert!(close(iq_scale(&[0.0, 1.0, 9.0]), IQ_HEADROOM));
    }

    #[test]
    fn puts_the_origin_in_the_middle_and_plus_i_to_the_right() {
        let mut grid = Grid::new(11, 11);
        add_constellation(&mut grid, &iq(&[(0.0, 0.0), (1.0, 0.0)]), 1.0, 1.0, 0);
        assert_eq!(lit(&grid), vec![(5, 5), (10, 5)]);
    }

    #[test]
    fn draws_plus_q_upward() {
        let mut grid = Grid::new(11, 11);
        add_constellation(&mut grid, &iq(&[(0.0, 1.0)]), 1.0, 1.0, 0);
        assert_eq!(lit(&grid), vec![(5, 0)]);
    }

    #[test]
    fn clamps_a_sample_past_the_scale_onto_the_edge() {
        let mut grid = Grid::new(11, 11);
        add_constellation(&mut grid, &iq(&[(50.0, 0.0)]), 1.0, 1.0, 0);
        assert_eq!(lit(&grid), vec![(10, 5)]);
    }

    #[test]
    fn plots_only_every_nth_sample_when_decimating() {
        let samples = iq(&[(1.0, 0.0), (0.0, 0.0), (-1.0, 0.0), (0.0, 0.0)]);
        let mut grid = Grid::new(11, 11);
        add_constellation(&mut grid, &samples, 1.0, 2.0, 0);
        assert_eq!(lit(&grid), vec![(0, 5), (10, 5)]);
        let mut shifted = Grid::new(11, 11);
        add_constellation(&mut shifted, &samples, 1.0, 2.0, 1);
        assert_eq!(lit(&shifted), vec![(5, 5)]);
    }

    #[test]
    fn accumulates_one_gain_step_per_visit_and_saturates() {
        let mut grid = Grid::new(3, 3);
        add_constellation(&mut grid, &iq(&[(0.0, 0.0)]), 1.0, 1.0, 0);
        assert!(close(grid.cells[4], BASEBAND_GAIN));
        for _ in 0..40 {
            add_constellation(&mut grid, &iq(&[(0.0, 0.0)]), 1.0, 1.0, 0);
        }
        assert_eq!(grid.cells[4], 1.0);
    }

    #[test]
    fn survives_a_zero_scale() {
        let mut grid = Grid::new(5, 5);
        add_constellation(&mut grid, &iq(&[(0.0, 0.0)]), 0.0, 1.0, 0);
        assert_eq!(lit(&grid), vec![(2, 2)]);
    }

    #[test]
    fn draws_every_eye_window_as_one_trace_over_two_periods() {
        let mut grid = Grid::new(9, 9);
        let samples = iq(&[
            (1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (-1.0, 0.0),
        ]);
        add_eye(&mut grid, &samples, 2.0, Eye::I, 1.0);
        let hits = lit(&grid);
        let mut columns: Vec<usize> = hits.iter().map(|hit| hit.0).collect();
        columns.sort_unstable();
        columns.dedup();
        assert_eq!(columns, (0..9).collect::<Vec<_>>());
        assert!(hits.iter().any(|hit| hit.1 == 0) && hits.iter().any(|hit| hit.1 == 8));
    }

    #[test]
    fn an_eye_needs_a_whole_window() {
        let mut grid = Grid::new(9, 9);
        add_eye(&mut grid, &iq(&[(1.0, 0.0), (1.0, 0.0)]), 8.0, Eye::I, 1.0);
        assert!(lit(&grid).is_empty());
    }

    #[test]
    fn folds_the_quadrature_rail_when_asked() {
        let mut grid = Grid::new(9, 9);
        add_eye(
            &mut grid,
            &iq(&[(0.0, 1.0), (0.0, 1.0), (0.0, -1.0), (0.0, -1.0)]),
            2.0,
            Eye::Q,
            1.0,
        );
        let top: Vec<usize> = lit(&grid)
            .iter()
            .filter(|hit| hit.0 == 0)
            .map(|hit| hit.1)
            .collect();
        assert_eq!(top, vec![0]);
    }

    #[test]
    fn reads_a_rotating_phasor_as_a_steady_frequency() {
        let pairs: Vec<(f32, f32)> = (0..16)
            .map(|i| {
                let phase = i as f32 * PI / 2.0;
                (phase.cos(), phase.sin())
            })
            .collect();
        let mut grid = Grid::new(9, 9);
        add_eye(&mut grid, &iq(&pairs), 2.0, Eye::Frequency, 1.0);
        let rows: Vec<usize> = lit(&grid).iter().map(|hit| hit.1).collect();
        assert!(rows.contains(&2));
        assert!(rows.iter().all(|row| (2..=4).contains(row)));
    }

    #[test]
    fn scales_the_i_and_q_rails_to_their_own_peak() {
        assert!(close(
            eye_scale(&iq(&[(0.25, 0.0), (-0.5, 0.0)]), Eye::I),
            0.5
        ));
        assert!(close(
            eye_scale(&iq(&[(0.0, 0.25), (0.0, -0.125)]), Eye::Q),
            0.25
        ));
        assert_eq!(eye_scale(&iq(&[(0.001, 0.001)]), Eye::Frequency), 1.0);
    }

    #[test]
    fn fades_towards_zero_and_snaps_there() {
        let mut grid = Grid::new(1, 1);
        add_constellation(&mut grid, &iq(&[(0.0, 0.0)]), 1.0, 1.0, 0);
        grid.decay(0.5);
        assert!(close(grid.cells[0], BASEBAND_GAIN / 2.0));
        for _ in 0..20 {
            grid.decay(0.5);
        }
        assert_eq!(grid.cells[0], 0.0);
        add_constellation(&mut grid, &iq(&[(0.0, 0.0)]), 1.0, 1.0, 0);
        grid.clear();
        assert!(lit(&grid).is_empty());
    }

    #[test]
    fn divides_the_sample_rate_by_the_symbol_rate() {
        assert_eq!(samples_per_symbol(24_000.0, 4800.0), 5.0);
        assert_eq!(samples_per_symbol(24_000.0, 0.0), 1.0);
        assert_eq!(samples_per_symbol(0.0, 4800.0), 1.0);
    }

    fn shaped_bpsk(symbols: &[f32], period: usize, phase: usize) -> Vec<f32> {
        let total = symbols.len() * period;
        let mut out = vec![0.0; total * 2];
        for i in 0..total {
            let since = (i + total - phase) % period;
            let index = ((i + total - phase) / period) % symbols.len();
            let window = (PI * (since as f32 + 0.5) / period as f32).sin();
            out[i * 2] = symbols[index] * window;
        }
        out
    }

    fn sampled_energy(wave: &[f32], period: usize, offset: usize) -> f32 {
        let taken: Vec<f32> = wave
            .as_chunks::<2>()
            .0
            .iter()
            .skip(offset)
            .step_by(period)
            .map(|[re, im]| re * re + im * im)
            .collect();
        if taken.is_empty() {
            0.0
        } else {
            taken.iter().sum::<f32>() / taken.len() as f32
        }
    }

    #[test]
    fn lands_on_the_instant_the_eye_is_widest_open() {
        let period = 8;
        let symbols = [1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
        for phase in [0, 2, 5, 7] {
            let wave = shaped_bpsk(&symbols, period, phase);
            let want = (0..period)
                .max_by(|a, b| {
                    sampled_energy(&wave, period, *a).total_cmp(&sampled_energy(&wave, period, *b))
                })
                .unwrap_or(0) as i64;
            let found = symbol_phase(&wave, period as f32);
            let apart = (found - want).abs();
            assert!(
                apart.min(period as i64 - apart) <= 1,
                "phase {phase}: {found} vs {want}"
            );
        }
    }

    #[test]
    fn stays_inside_the_period_and_declines_without_a_symbol() {
        let wave = shaped_bpsk(&[1.0, -1.0, 1.0, -1.0], 8, 3);
        assert!((0..8).contains(&symbol_phase(&wave, 8.0)));
        assert_eq!(symbol_phase(&[], 8.0), 0);
        assert_eq!(symbol_phase(&iq(&[(1.0, 0.0), (1.0, 0.0)]), 1.0), 0);
    }

    #[test]
    fn puts_four_levels_in_four_separated_humps() {
        let levels: Vec<f32> = (0..400).map(|i| [1.0, 3.0, -1.0, -3.0][i % 4]).collect();
        let bins = symbol_histogram(&levels, 1, 3.0);
        assert_eq!(bins.iter().filter(|value| **value > 0.0).count(), 4);
        assert_eq!(bins.iter().copied().fold(0.0, f32::max), 1.0);
    }

    #[test]
    fn a_histogram_ignores_what_falls_off_the_rail_and_reads_one_rail_of_a_pair() {
        let bins = symbol_histogram(&[0.0, 40.0, -40.0], 1, 1.0);
        assert_eq!(bins.iter().sum::<f32>(), 1.0);
        let pairs = symbol_histogram(&[1.0, -1.0, 1.0, -1.0, 1.0, -1.0], 2, 1.0);
        assert_eq!(pairs.iter().filter(|value| **value > 0.0).count(), 1);
    }

    #[test]
    fn a_trend_keeps_the_newest_values_and_refuses_what_cannot_plot() {
        let mut trend = Trend::new(3);
        for value in [1.0, 2.0, 3.0, 4.0, 5.0] {
            trend.push(value);
        }
        assert_eq!(trend.len(), 3);
        assert_eq!([0, 1, 2].map(|i| trend.sample(i)), [3.0, 4.0, 5.0]);
        let mut empty = Trend::new(4);
        empty.push(f32::NAN);
        empty.push(f32::INFINITY);
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn a_trend_range_spans_its_values_and_widens_when_flat() {
        let mut trend = Trend::new(8);
        for value in [-3.0, 12.0, 4.0] {
            trend.push(value);
        }
        assert_eq!(trend.range(), (-3.0, 12.0));
        let mut flat = Trend::new(4);
        flat.push(7.0);
        flat.push(7.0);
        let (min, max) = flat.range();
        assert!(max > min);
    }

    fn rail_block(symbols: &[f32], reference: &[f32]) -> Block {
        Block {
            plane: SymbolPlane::Level,
            symbol_rate: 4800.0,
            evm: 0.0,
            mer_db: 0.0,
            margin: 0.0,
            freq_error_hz: 0.0,
            reference: reference.to_vec(),
            symbols: symbols.to_vec(),
        }
    }

    fn levels(symbols: &[f32]) -> Block {
        rail_block(symbols, &[-3.0, -1.0, 1.0, 3.0])
    }

    fn cloud(symbols: &[f32]) -> Block {
        Block {
            plane: SymbolPlane::Complex,
            ..rail_block(symbols, &[1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0])
        }
    }

    #[test]
    fn the_decision_distance_is_half_the_shortest_hop() {
        assert_eq!(decision_distance(&levels(&[])), 1.0);
        assert!(close(decision_distance(&cloud(&[])), 1.0));
        assert_eq!(decision_distance(&rail_block(&[], &[2.0])), 1.0);
    }

    #[test]
    fn the_gain_lifts_a_weak_rail_onto_its_levels() {
        assert!(close(symbol_gain(&levels(&[-3.0, -1.0, 1.0, 3.0])), 1.0));
        assert!(close(symbol_gain(&levels(&[-1.5, -0.5, 0.5, 1.5])), 2.0));
        assert_eq!(symbol_gain(&levels(&[])), 1.0);
    }

    #[test]
    fn a_state_is_named_by_the_bits_that_select_it() {
        assert_eq!(state_bits(2, 4), "10");
        assert_eq!(state_bits(5, 8), "101");
        assert_eq!(state_bits(2, 3), "2");
    }

    #[test]
    fn a_clean_rail_reads_every_state_dead_on() {
        let states = symbol_states(&levels(&[-3.0, -1.0, 1.0, 3.0]));
        assert_eq!(
            states.iter().map(|state| state.i).collect::<Vec<_>>(),
            vec![3.0, 1.0, -1.0, -3.0]
        );
        assert_eq!(
            states
                .iter()
                .map(|state| state.bits.as_str())
                .collect::<Vec<_>>(),
            vec!["11", "10", "01", "00"]
        );
        for state in &states {
            assert_eq!(state.count, 1);
            assert!(close(state.share, 0.25));
            assert!(close(state.mean, 0.0));
        }
        for state in symbol_states(&levels(&[-6.0, -2.0, 2.0, 6.0])) {
            assert!(close(state.mean, 0.0));
        }
    }

    #[test]
    fn a_compressed_rail_pulls_the_outer_states_in() {
        let states = symbol_states(&levels(&[2.4, -2.4, 1.0, -1.0, 2.4, -2.4, 1.0, -1.0]));
        for state in &states {
            let sign = if state.i.abs() == 3.0 {
                -state.i.signum()
            } else {
                state.i.signum()
            };
            assert_eq!(state.mean.signum(), sign);
        }
    }

    #[test]
    fn the_offset_is_measured_against_the_slice_point() {
        let states = symbol_states(&levels(&[-3.0, -1.0, 1.0, 3.5, -3.0, -1.0, 1.0, 3.5]));
        assert_eq!(states[0].i, 3.0);
        assert!(states[0].mean > 0.2 && states[0].mean < 0.5);
    }

    #[test]
    fn a_wobbling_state_spreads_and_a_steady_one_does_not() {
        let states = symbol_states(&levels(&[-3.0, -1.0, 1.0, 3.0, -3.0, -1.0, 1.6, 3.0]));
        let wobbly = states
            .iter()
            .find(|state| state.i == 1.0)
            .expect("a state at 1");
        let steady = states
            .iter()
            .find(|state| state.i == 3.0)
            .expect("a state at 3");
        assert!(wobbly.sigma > 0.0);
        assert!(close(steady.sigma, 0.0));
        assert!(wobbly.peak.abs() > wobbly.mean.abs());
    }

    #[test]
    fn a_state_nothing_landed_on_is_kept() {
        let states = symbol_states(&levels(&[-3.0, -1.0, 1.0, -3.0, -1.0, 1.0]));
        let missing = states
            .iter()
            .find(|state| state.i == 3.0)
            .expect("a state at 3");
        assert_eq!(missing.count, 0);
        assert_eq!(missing.share, 0.0);
        assert!(missing.mean.is_nan());
    }

    #[test]
    fn a_cloud_is_measured_by_how_far_each_point_strays() {
        let states = symbol_states(&cloud(&[1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0]));
        assert_eq!(states.len(), 4);
        for state in &states {
            assert_eq!(state.count, 1);
            assert!(close(state.mean, 0.0));
        }
        assert!(symbol_states(&rail_block(&[1.0, 2.0], &[])).is_empty());
    }
}
