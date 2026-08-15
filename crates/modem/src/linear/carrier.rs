use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::LoopFilter;

use crate::constellation::Constellation;

pub const DAMPING: f64 = std::f64::consts::FRAC_1_SQRT_2;

pub const FREQ_LIMIT_CYCLES_PER_SYMBOL: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseDetector {
    DecisionDirected,
    MthPower { m: u32 },
}

impl PhaseDetector {
    #[must_use]
    pub fn error(self, y: Complex<f32>, table: &Constellation) -> f64 {
        if y.re == 0.0 && y.im == 0.0 {
            return 0.0;
        }
        match self {
            Self::DecisionDirected => {
                let label = table.hard_slice(y);
                let index = table.labels().iter().position(|&l| l == label);
                let Some(x) = index.map(|i| table.points()[i]) else {
                    return 0.0;
                };
                if x.re == 0.0 && x.im == 0.0 {
                    return 0.0;
                }
                f64::from((y * x.conj()).arg())
            }
            Self::MthPower { m } => {
                let reference = unit_power(table.points()[0], m);
                let stripped = unit_power(y, m);
                if reference.norm() <= 0.0 || stripped.norm() <= 0.0 {
                    return 0.0;
                }
                (stripped * reference.conj()).arg() / f64::from(m)
            }
        }
    }
}

fn unit_power(y: Complex<f32>, m: u32) -> Complex<f64> {
    let y = Complex::new(f64::from(y.re), f64::from(y.im));
    let norm = y.norm();
    if norm <= 0.0 {
        return Complex::new(0.0, 0.0);
    }
    let unit = y / norm;
    let mut acc = Complex::new(1.0f64, 0.0);
    for _ in 0..m {
        acc *= unit;
    }
    acc
}

#[derive(Clone, Debug)]
pub struct CarrierLoop {
    detector: PhaseDetector,
    filter: LoopFilter,
    phase: f64,
    fll_gain: f64,
    fll_freq: f64,
    last_stripped: Option<Complex<f64>>,
}

impl CarrierLoop {
    #[must_use]
    pub fn new(detector: PhaseDetector, loop_bw: f64) -> Self {
        if let PhaseDetector::MthPower { m } = detector {
            assert!(m >= 2, "M-th power stripping needs an order of at least 2");
        }
        Self {
            detector,
            filter: LoopFilter::new(loop_bw, DAMPING, FREQ_LIMIT_CYCLES_PER_SYMBOL),
            phase: 0.0,
            fll_gain: 0.0,
            fll_freq: 0.0,
            last_stripped: None,
        }
    }

    #[must_use]
    pub fn with_frequency_aid(mut self, gain: f64) -> Self {
        assert!(
            matches!(self.detector, PhaseDetector::MthPower { .. }),
            "the frequency aid needs a modulation-stripping detector"
        );
        assert!(
            gain >= 0.0 && gain.is_finite(),
            "FLL gain {gain} is not one"
        );
        self.fll_gain = gain;
        self
    }

    #[must_use]
    pub fn advance(&mut self, y: Complex<f32>, table: &Constellation) -> Complex<f32> {
        let rot = Complex::new((-self.phase).cos() as f32, (-self.phase).sin() as f32);
        let out = y * rot;
        let error = self.detector.error(out, table);
        let mut inc = self.filter.advance(error);
        if self.fll_gain > 0.0 {
            inc += self.advance_frequency_aid(out);
        }
        self.phase = wrap(self.phase + inc);
        out
    }

    fn advance_frequency_aid(&mut self, y: Complex<f32>) -> f64 {
        let PhaseDetector::MthPower { m } = self.detector else {
            return 0.0;
        };
        let stripped = unit_power(y, m);
        if stripped.norm() <= 0.0 {
            return self.fll_freq;
        }
        if let Some(previous) = self.last_stripped {
            let rotation = (stripped * previous.conj()).arg() / f64::from(m);
            self.fll_freq = (self.fll_freq + self.fll_gain * rotation).clamp(
                -TAU * FREQ_LIMIT_CYCLES_PER_SYMBOL,
                TAU * FREQ_LIMIT_CYCLES_PER_SYMBOL,
            );
        }
        self.last_stripped = Some(stripped);
        self.fll_freq
    }

    #[must_use]
    pub fn freq_cycles_per_symbol(&self) -> f64 {
        self.filter.freq_norm() + self.fll_freq / TAU
    }

    pub fn reset(&mut self) {
        self.filter.reset(0.0);
        self.phase = 0.0;
        self.fll_freq = 0.0;
        self.last_stripped = None;
    }
}

fn wrap(theta: f64) -> f64 {
    let t = (theta + PI).rem_euclid(TAU);
    t - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{perf::assert_no_alloc, rng::Rng},
        constellation::tables,
    };

    fn stream(m: u32, n: usize, seed: u64, table: &Constellation) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| table.points()[(rng.next_u64() % u64::from(m)) as usize])
            .collect()
    }

    fn rotate(w: &mut [Complex<f32>], cycles_per_symbol: f64, phase0: f64) {
        for (k, s) in w.iter_mut().enumerate() {
            let theta = phase0 + TAU * cycles_per_symbol * k as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
    }

    fn worst_residual_turns(out: &[Complex<f32>], table: &Constellation) -> f64 {
        out[out.len() * 3 / 4..]
            .iter()
            .map(|&y| {
                let label = table.hard_slice(y);
                let i = table.labels().iter().position(|&l| l == label).unwrap();
                f64::from((y * table.points()[i].conj()).arg()).abs() / TAU
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn both_detectors_acquire_a_static_phase_offset() {
        for (name, table, detector, m) in [
            (
                "qpsk mth-power",
                tables::psk(4).unwrap(),
                PhaseDetector::MthPower { m: 4 },
                4u32,
            ),
            (
                "qpsk decision-directed",
                tables::psk(4).unwrap(),
                PhaseDetector::DecisionDirected,
                4,
            ),
            (
                "16-qam decision-directed",
                tables::qam_square(16).unwrap(),
                PhaseDetector::DecisionDirected,
                4,
            ),
        ] {
            let mut wave = stream(table.len() as u32, 4_000, 0xca47, &table);
            rotate(&mut wave, 0.0, 0.7);
            let mut loop_ = CarrierLoop::new(detector, 0.01);
            let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
            let _ = m;
            let residual = worst_residual_turns(&out, &table);
            assert!(residual < 0.01, "{name}: worst residual {residual} turns");
        }
    }

    #[test]
    fn the_integrator_reads_back_a_static_frequency_offset() {
        let table = tables::psk(4).unwrap();
        let offset = 1.5e-3;
        let mut wave = stream(4, 6_000, 0xf5e9, &table);
        rotate(&mut wave, offset, 0.0);
        let mut loop_ = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01);
        let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
        assert!(worst_residual_turns(&out, &table) < 0.02);
        let measured = loop_.freq_cycles_per_symbol();
        assert!(
            (measured - offset).abs() < 0.1 * offset,
            "loop reads {measured}, injected {offset}"
        );
    }

    fn symbols_to_lock(out: &[Complex<f32>], table: &Constellation, tol: f64) -> Option<usize> {
        const WINDOW: usize = 200;
        let error = |y: Complex<f32>| {
            let label = table.hard_slice(y);
            let i = table.labels().iter().position(|&l| l == label).unwrap();
            f64::from((y * table.points()[i].conj()).arg()).abs() / TAU
        };
        let mut run = 0usize;
        for (k, &y) in out.iter().enumerate() {
            run = if error(y) < tol { run + 1 } else { 0 };
            if run == WINDOW {
                return Some(k + 1 - WINDOW);
            }
        }
        None
    }

    #[test]
    fn the_frequency_aid_acquires_far_sooner_than_the_phase_loop_alone() {
        let table = tables::psk(4).unwrap();
        let lock_after = |aided: bool| {
            let mut wave = stream(4, 20_000, 0x7011, &table);
            rotate(&mut wave, 0.1, 0.0);
            let base = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.002);
            let mut loop_ = if aided {
                base.with_frequency_aid(0.01)
            } else {
                base
            };
            let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
            symbols_to_lock(&out, &table, 0.02)
        };
        let aided = lock_after(true).expect("the aided loop never locked");
        assert!(aided < 4_000, "the aided loop took {aided} symbols");
        assert_eq!(
            lock_after(false),
            None,
            "the plain loop acquired 0.1 cycles/symbol at a 0.002 loop bandwidth; \
             re-measure where the aid earns its keep"
        );
    }

    #[test]
    fn an_origin_symbol_casts_no_vote() {
        let table = tables::ook().unwrap();
        let mut loop_ = CarrierLoop::new(PhaseDetector::DecisionDirected, 0.05);
        let before = loop_.freq_cycles_per_symbol();
        for _ in 0..100 {
            let _ = loop_.advance(Complex::new(0.0, 0.0), &table);
        }
        assert_eq!(loop_.freq_cycles_per_symbol(), before);
        assert!(
            (PhaseDetector::DecisionDirected.error(Complex::new(0.0, 0.0), &table)).abs() < 1e-18
        );
    }

    #[test]
    fn reset_returns_the_loop_to_a_cold_start() {
        let table = tables::psk(4).unwrap();
        let mut wave = stream(4, 2_000, 0x1e5, &table);
        rotate(&mut wave, 1e-3, 0.4);
        let mut loop_ = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01);
        for &y in &wave {
            let _ = loop_.advance(y, &table);
        }
        assert!(loop_.freq_cycles_per_symbol().abs() > 1e-5);
        loop_.reset();
        assert_eq!(loop_.freq_cycles_per_symbol(), 0.0);
    }

    #[test]
    fn the_loop_allocates_nothing() {
        let table = tables::qam_square(16).unwrap();
        let mut dd = CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01);
        let mut mth =
            CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01).with_frequency_aid(0.01);
        let y = Complex::new(0.7f32, 0.3);
        let psk = tables::psk(4).unwrap();
        let _ = dd.advance(y, &table);
        let _ = mth.advance(y, &psk);
        assert_no_alloc("CarrierLoop::advance decision-directed", || {
            std::hint::black_box(dd.advance(y, &table));
        });
        assert_no_alloc("CarrierLoop::advance mth-power + FLL", || {
            std::hint::black_box(mth.advance(y, &psk));
        });
    }
}
