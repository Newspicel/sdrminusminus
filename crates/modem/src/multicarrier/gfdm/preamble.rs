use num_complex::Complex;

use super::GfdmParams;
use crate::{
    framesync::{RepetitionDetector, conj_product, derotate},
    multicarrier::transform::Dft,
};

pub const TAP_FLOOR: f64 = 0.1;
pub const NOISE_FLOOR: f64 = 12.0;
pub const DEFAULT_INTEGER_SPAN: i32 = 2;
pub const DEFAULT_MIN_METRIC: f64 = 0.15;
pub const DEFAULT_BACKOFF: usize = 2;

const SEED: u32 = 0x6fd3_a5c1;

#[derive(Clone, Debug)]
pub struct GfdmPreamble {
    spectrum: Vec<Complex<f32>>,
    block: Vec<Complex<f32>>,
}

impl GfdmPreamble {
    #[must_use]
    pub fn new(params: &GfdmParams) -> Self {
        let n = params.block();
        assert!(
            n >= 4 && n.is_multiple_of(2),
            "a two-half preamble needs an even block, got {n}"
        );
        let spectrum = qpsk_sequence(n / 2);
        let mut half = spectrum.clone();
        Dft::new(n / 2).inverse(&mut half);
        let block = half.iter().chain(&half).copied().collect();
        Self { spectrum, block }
    }

    #[must_use]
    pub fn half(&self) -> usize {
        self.spectrum.len()
    }

    #[must_use]
    pub fn spectrum(&self) -> &[Complex<f32>] {
        &self.spectrum
    }

    #[must_use]
    pub fn block(&self) -> &[Complex<f32>] {
        &self.block
    }
}

fn qpsk_sequence(len: usize) -> Vec<Complex<f32>> {
    let mut state = SEED;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let i = if state & 1 == 0 { 1.0 } else { -1.0 };
            let q = if state & 2 == 0 { 1.0 } else { -1.0 };
            Complex::new(i, q) * std::f32::consts::FRAC_1_SQRT_2
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GfdmAcquisition {
    pub preamble_start: usize,
    pub data_start: usize,
    pub cfo: f64,
    pub metric: f64,
}

#[derive(Clone, Debug)]
pub struct GfdmSync {
    detector: RepetitionDetector,
    preamble: GfdmPreamble,
    cp: usize,
    integer_span: i32,
    min_metric: f64,
    backoff: usize,
    rotated: Vec<Complex<f32>>,
    energy: Vec<f64>,
    best: Vec<f64>,
    sorted: Vec<f64>,
}

impl GfdmSync {
    #[must_use]
    pub fn new(params: &GfdmParams) -> Self {
        let preamble = GfdmPreamble::new(params);
        let half = preamble.half();
        let span = half + params.cp + 1;
        Self {
            detector: RepetitionDetector::new(half, half),
            cp: params.cp,
            integer_span: DEFAULT_INTEGER_SPAN,
            min_metric: DEFAULT_MIN_METRIC,
            backoff: DEFAULT_BACKOFF.min(params.cp),
            rotated: vec![Complex::new(0.0, 0.0); span + params.block()],
            energy: vec![0.0; span],
            best: vec![0.0; span],
            sorted: vec![0.0; span],
            preamble,
        }
    }

    #[must_use]
    pub fn with_integer_span(mut self, span: i32) -> Self {
        self.integer_span = span.max(0);
        self
    }

    #[must_use]
    pub fn with_min_metric(mut self, metric: f64) -> Self {
        self.min_metric = metric;
        self
    }

    #[must_use]
    pub fn preamble(&self) -> &GfdmPreamble {
        &self.preamble
    }

    #[must_use]
    pub fn cfo_range(&self) -> f64 {
        (f64::from(self.integer_span) + 0.5) / self.preamble.half() as f64
    }

    pub fn acquire(&mut self, x: &[Complex<f32>], search: usize) -> Option<GfdmAcquisition> {
        let found = self.detector.detect(x, search)?;
        let lo = found.offset.saturating_sub(self.preamble.half() / 2);
        let (cfo, peak) = self.pick_integer(x, lo, found.cfo)?;
        let metric = self.normalised(x, lo, peak);
        if metric < self.min_metric {
            return None;
        }
        let start = lo + self.timing()?;
        let n = self.preamble.block().len();
        Some(GfdmAcquisition {
            preamble_start: start,
            data_start: start + n,
            cfo: self.fine_cfo(x, start, cfo),
            metric,
        })
    }

    fn normalised(&self, x: &[Complex<f32>], lo: usize, peak: f64) -> f64 {
        let n = self.preamble.block().len();
        let Some(at) = self.best.iter().position(|&e| e >= peak) else {
            return 0.0;
        };
        let received: f64 = x
            .iter()
            .skip(lo + at)
            .take(n)
            .map(|v| f64::from(v.norm_sqr()))
            .sum();
        let known: f64 = self
            .preamble
            .block()
            .iter()
            .map(|v| f64::from(v.norm_sqr()))
            .sum();
        if received > 0.0 {
            peak / (received * known)
        } else {
            0.0
        }
    }

    fn pick_integer(&mut self, x: &[Complex<f32>], lo: usize, coarse: f64) -> Option<(f64, f64)> {
        let n = self.preamble.block().len();
        if x.len() < lo + n {
            return None;
        }
        let half = self.preamble.half() as f64;
        let mut best = (coarse, f64::NEG_INFINITY);
        for m in -self.integer_span..=self.integer_span {
            let candidate = coarse + f64::from(m) / half;
            let peak = self.correlate(x, lo, candidate);
            if peak > best.1 {
                best = (candidate, peak);
                self.best.copy_from_slice(&self.energy);
            }
        }
        Some(best)
    }

    fn correlate(&mut self, x: &[Complex<f32>], lo: usize, cfo: f64) -> f64 {
        let n = self.preamble.block().len();
        derotate(x, lo, cfo, &mut self.rotated);
        let known = self.preamble.block();
        let mut peak = 0.0f64;
        for (t, slot) in self.energy.iter_mut().enumerate() {
            let mut acc = Complex::new(0.0f64, 0.0);
            for (&tap, &y) in known.iter().zip(&self.rotated[t..t + n]) {
                acc += conj_product(tap, y);
            }
            *slot = acc.norm_sqr();
            peak = peak.max(*slot);
        }
        peak
    }

    fn timing(&mut self) -> Option<usize> {
        let peak = self.best.iter().copied().fold(0.0f64, f64::max);
        let floor = (TAP_FLOOR * peak).max(NOISE_FLOOR * self.median());
        let first = self.best.iter().position(|&e| e > floor)?;
        let last = self.best.iter().rposition(|&e| e > floor)?;
        let earliest = first.saturating_sub(self.backoff);
        Some(earliest.max(last.saturating_sub(self.cp)).min(first))
    }

    fn median(&mut self) -> f64 {
        self.sorted.copy_from_slice(&self.best);
        let middle = self.sorted.len() / 2;
        *self.sorted.select_nth_unstable_by(middle, f64::total_cmp).1
    }

    fn fine_cfo(&self, x: &[Complex<f32>], start: usize, coarse: f64) -> f64 {
        let half = self.preamble.half();
        let mut acc = Complex::new(0.0f64, 0.0);
        for n in start..start + half {
            let (Some(&a), Some(&b)) = (x.get(n), x.get(n + half)) else {
                break;
            };
            acc += conj_product(a, b);
        }
        let period = half as f64;
        let measured = acc.arg() / (std::f64::consts::TAU * period);
        measured + ((coarse - measured) * period).round() / period
    }
}
