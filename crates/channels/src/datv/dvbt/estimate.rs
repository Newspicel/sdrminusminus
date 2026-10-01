use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::fft::Transform;

use super::{
    mapping::Mapping,
    wiener::{Design, FrequencyFilter, measure_span},
};

const PILOT_GAIN: f32 = 0.75;
const PILOT_NOISE: f32 = PILOT_GAIN * PILOT_GAIN;
const SCATTER: usize = 12;
const GRID: usize = 3;
const ALL_PHASES: u8 = 0b1111;
const SPAN_MARGIN: f32 = 4.0;
const ALIAS_SHARE: f32 = 0.9;
const UNKNOWN_NOISE: f32 = 0.05;
const NOISE_FLOOR: f32 = 1e-4;

#[derive(Clone, Copy, Debug)]
pub struct Symbol {
    pub phase: usize,
    pub offset: isize,
    pub shift: isize,
    pub guard: usize,
    pub noise: f32,
}

pub struct ChannelEstimator {
    held: Vec<Complex<f32>>,
    continual: Vec<Complex<f32>>,
    sparse: Vec<Complex<f32>>,
    estimates: Vec<Complex<f32>>,
    seen: u8,
    last_phase: Option<usize>,
    span: Option<(f32, f32)>,
    held_filter: FrequencyFilter,
    sparse_filter: FrequencyFilter,
    profile: Vec<Complex<f32>>,
    inverse: Transform,
}

impl ChannelEstimator {
    pub fn new(carriers: usize) -> Self {
        let grid = carriers.div_ceil(GRID);
        let profile = grid.next_power_of_two();
        Self {
            held: vec![Complex::new(0.0, 0.0); carriers],
            continual: Vec::with_capacity(carriers),
            sparse: vec![Complex::new(0.0, 0.0); carriers],
            estimates: vec![Complex::new(0.0, 0.0); carriers],
            seen: 0,
            last_phase: None,
            span: None,
            held_filter: FrequencyFilter::new(),
            sparse_filter: FrequencyFilter::new(),
            profile: vec![Complex::new(0.0, 0.0); profile],
            inverse: Transform::inverse(profile),
        }
    }

    pub fn reset(&mut self) {
        self.seen = 0;
        self.last_phase = None;
        self.span = None;
    }

    pub fn estimates(&self) -> &[Complex<f32>] {
        &self.estimates
    }

    pub fn first_path(&self) -> Option<f32> {
        self.span.map(|(first, _)| first)
    }

    #[cfg(test)]
    pub fn time_interpolated(&self) -> bool {
        self.seen == ALL_PHASES
    }

    pub fn update(&mut self, map: &Mapping, spectrum: &[Complex<f32>], symbol: Symbol) -> bool {
        let phase = symbol.phase;
        let measure =
            |k: usize| spectrum[map.bin(k, symbol.offset)] * (PILOT_GAIN * map.reference[k]);
        if self.last_phase.is_some_and(|last| (last + 1) % 4 != phase) {
            self.seen = 0;
            self.span = None;
        }
        self.last_phase = Some(phase);
        if self.seen != 0 {
            if symbol.shift != 0 {
                self.follow_window(map, symbol.offset, symbol.shift);
            }
            self.follow_common_phase(map, &measure);
        }
        let first = GRID * phase;
        let sparse_count = (map.carriers - 1 - first) / SCATTER + 1;
        for k in (first..map.carriers).step_by(SCATTER) {
            self.sparse[k] = measure(k);
        }
        if self.seen != ALL_PHASES {
            let sparse = &self.sparse;
            self.span = measure_span(
                &mut self.profile,
                &mut self.inverse,
                |m| sparse[first + m * SCATTER],
                sparse_count,
                map.fft as f32 / SCATTER as f32,
                symbol.guard,
            );
        }
        let noise = self.relative_noise(symbol.noise, first);
        self.prepare(map, symbol.guard, noise);
        let held_wins = self.seen == ALL_PHASES && self.held_predicts_better(map, phase, &measure);
        for k in (first..map.carriers).step_by(SCATTER) {
            self.held[k] = self.sparse[k];
        }
        self.continual.clear();
        self.continual
            .extend(map.continual.iter().map(|&k| measure(k)));
        self.seen |= 1 << phase;
        if held_wins {
            let held = &self.held;
            self.held_filter.apply(
                |m| held[m * GRID],
                0,
                held_count(map),
                &mut self.estimates[..map.carriers],
            );
        } else {
            let sparse = &self.sparse;
            self.sparse_filter.apply(
                |m| sparse[first + m * SCATTER],
                first,
                sparse_count,
                &mut self.estimates[..map.carriers],
            );
        }
        if self.seen == ALL_PHASES {
            let held = &self.held;
            self.span = measure_span(
                &mut self.profile,
                &mut self.inverse,
                |m| held[m * GRID],
                held_count(map),
                map.fft as f32 / GRID as f32,
                symbol.guard,
            );
        }
        held_wins
    }

    fn relative_noise(&self, noise: f32, first: usize) -> f32 {
        let anchors = self.sparse[first..].iter().step_by(SCATTER);
        let count = anchors.len().max(1) as f32;
        let power = anchors.map(Complex::norm_sqr).sum::<f32>() / count;
        if noise > 0.0 && power > 0.0 {
            (PILOT_NOISE * noise / power).max(NOISE_FLOOR)
        } else {
            UNKNOWN_NOISE
        }
    }

    fn prepare(&mut self, map: &Mapping, guard: usize, noise: f32) {
        let (first, last) = self.span.unwrap_or((0.0, guard as f32));
        for (filter, spacing) in [
            (&mut self.held_filter, GRID),
            (&mut self.sparse_filter, SCATTER),
        ] {
            let alias = ALIAS_SHARE * map.fft as f32 / spacing as f32;
            let start = first - SPAN_MARGIN;
            let width = (last - first + 2.0 * SPAN_MARGIN).min(alias);
            filter.prepare(Design {
                spacing,
                fft: map.fft,
                first: start,
                width,
                noise,
            });
        }
    }

    fn follow_window(&mut self, map: &Mapping, offset: isize, shift: isize) {
        let cycles = shift as f64 / map.fft as f64;
        let turn = |k: usize| {
            let bin = k as isize + offset - (map.carriers / 2) as isize;
            let (sin, cos) = (TAU * cycles * bin as f64).sin_cos();
            Complex::new(cos as f32, sin as f32)
        };
        let (sin, cos) = (TAU * cycles).sin_cos();
        let step = Complex::new(cos, sin);
        let start = turn(0);
        let mut rotor = Complex::new(f64::from(start.re), f64::from(start.im));
        for value in &mut self.held {
            *value *= Complex::new(rotor.re as f32, rotor.im as f32);
            rotor *= step;
        }
        for (&k, value) in map.continual.iter().zip(&mut self.continual) {
            *value *= turn(k);
        }
    }

    fn follow_common_phase(&mut self, map: &Mapping, measure: &impl Fn(usize) -> Complex<f32>) {
        let turn: Complex<f32> = map
            .continual
            .iter()
            .zip(&self.continual)
            .map(|(&k, previous)| measure(k) * previous.conj())
            .sum();
        let size = turn.norm();
        if size > 0.0 && size.is_finite() {
            let unit = turn / size;
            for value in &mut self.held {
                *value *= unit;
            }
        }
    }

    fn held_predicts_better(
        &self,
        map: &Mapping,
        phase: usize,
        measure: &impl Fn(usize) -> Complex<f32>,
    ) -> bool {
        let first = GRID * phase;
        let sparse_count = (map.carriers - 1 - first) / SCATTER + 1;
        let held = |m: usize| self.held[m * GRID];
        let sparse = |m: usize| self.sparse[first + m * SCATTER];
        let (held_error, sparse_error) = map
            .continual
            .iter()
            .filter(|&&k| k % SCATTER != first)
            .fold((0.0, 0.0), |(held_error, sparse_error), &k| {
                let truth = measure(k);
                let from_held = self.held_filter.at(&held, 0, held_count(map), k);
                let from_sparse = self.sparse_filter.at(&sparse, first, sparse_count, k);
                (
                    held_error + (from_held - truth).norm_sqr(),
                    sparse_error + (from_sparse - truth).norm_sqr(),
                )
            });
        held_error <= sparse_error
    }
}

fn held_count(map: &Mapping) -> usize {
    (map.carriers - 1) / GRID + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol_at(phase: usize) -> Symbol {
        Symbol {
            phase,
            offset: 0,
            shift: 0,
            guard: 512,
            noise: 1e-9,
        }
    }

    fn spectrum_for(map: &Mapping, channel: impl Fn(usize) -> Complex<f32>) -> Vec<Complex<f32>> {
        let mut spectrum = vec![Complex::new(0.0, 0.0); map.fft];
        for k in 0..map.carriers {
            spectrum[map.bin(k, 0)] = channel(k) * (map.reference[k] / PILOT_GAIN);
        }
        spectrum
    }

    #[test]
    fn four_symbols_resolve_an_echo_the_scattered_grid_alone_cannot() {
        let map = Mapping::new(2048);
        let echo = |k: usize| {
            Complex::new(1.0, 0.0)
                + Complex::from_polar(0.8, -std::f32::consts::TAU * k as f32 * 300.0 / 2048.0)
        };
        let spectrum = spectrum_for(&map, echo);
        let mut estimator = ChannelEstimator::new(map.carriers);
        let mut used_held = false;
        for symbol in 0..8 {
            used_held = estimator.update(&map, &spectrum, symbol_at(symbol % 4));
        }
        assert!(used_held && estimator.time_interpolated());
        let worst = (0..map.carriers)
            .filter(|k| k % GRID == 0)
            .map(|k| (estimator.estimates()[k] - echo(k)).norm())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "worst grid error {worst}");
    }

    #[test]
    fn a_common_phase_step_does_not_stale_the_held_pilots() {
        let map = Mapping::new(2048);
        let mut estimator = ChannelEstimator::new(map.carriers);
        let echo = |k: usize| {
            Complex::new(1.0, 0.0)
                + Complex::from_polar(0.8, -std::f32::consts::TAU * k as f32 * 300.0 / 2048.0)
        };
        for symbol in 0..12 {
            let rotation = Complex::from_polar(1.0, 0.4 * symbol as f32);
            let spectrum = spectrum_for(&map, |k| echo(k) * rotation);
            let held = estimator.update(&map, &spectrum, symbol_at(symbol % 4));
            if symbol < 4 {
                continue;
            }
            assert!(held, "symbol {symbol} fell back to the scattered grid");
            let worst = (40..map.carriers - 40)
                .map(|k| (estimator.estimates()[k] - echo(k) * rotation).norm())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.05, "symbol {symbol}: worst {worst}");
        }
    }

    #[test]
    fn a_phase_slip_restarts_the_time_interpolation() {
        let map = Mapping::new(2048);
        let spectrum = spectrum_for(&map, |_| Complex::new(1.0, 0.0));
        let mut estimator = ChannelEstimator::new(map.carriers);
        for symbol in 0..4 {
            estimator.update(&map, &spectrum, symbol_at(symbol));
        }
        assert!(estimator.time_interpolated());
        estimator.update(&map, &spectrum, symbol_at(2));
        assert!(!estimator.time_interpolated());
    }
}
