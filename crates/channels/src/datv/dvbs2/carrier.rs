use num_complex::Complex;
use sdrmm_dsp::fft::Transform;

use super::frame::Modulation;

const POWER_BLOCK: usize = 128;
const POWER_SPAN: usize = 2;
const SLOPE_GAIN: f32 = 0.2;
const COARSE_BLOCK: usize = 16;
const COARSE_BINS: usize = 4096;

pub(super) struct CoarseFrequency {
    transform: Transform,
    bins: Vec<Complex<f32>>,
}

impl CoarseFrequency {
    pub(super) fn new() -> Self {
        Self {
            transform: Transform::forward(COARSE_BINS),
            bins: vec![Complex::new(0.0, 0.0); COARSE_BINS],
        }
    }

    pub(super) fn remove(
        &mut self,
        symbols: &mut [Complex<f32>],
        order: i32,
        reference: Complex<f32>,
    ) -> f32 {
        self.bins.fill(Complex::new(0.0, 0.0));
        for (slot, block) in self.bins.iter_mut().zip(symbols.chunks(COARSE_BLOCK)) {
            *slot = block
                .iter()
                .map(|&symbol| mth_power(symbol, order))
                .sum::<Complex<f32>>()
                * reference.conj();
        }
        self.transform.process(&mut self.bins);
        let power = |bin: usize| self.bins[bin % COARSE_BINS].norm_sqr();
        let peak = (0..COARSE_BINS)
            .max_by(|&a, &b| power(a).total_cmp(&power(b)))
            .unwrap_or(0);
        let (left, centre, right) = (
            power(peak + COARSE_BINS - 1).sqrt(),
            power(peak).sqrt(),
            power(peak + 1).sqrt(),
        );
        let curvature = left - 2.0 * centre + right;
        let fraction = if curvature.abs() > f32::MIN_POSITIVE {
            (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let signed = if peak > COARSE_BINS / 2 {
            peak as f32 - COARSE_BINS as f32
        } else {
            peak as f32
        } + fraction;
        let rotation =
            std::f32::consts::TAU * signed / (COARSE_BINS * COARSE_BLOCK) as f32 / order as f32;
        for (index, symbol) in symbols.iter_mut().enumerate() {
            let phase = (rotation * index as f32).rem_euclid(std::f32::consts::TAU);
            *symbol *= Complex::from_polar(1.0, -phase);
        }
        rotation
    }
}

fn mth_power(symbol: Complex<f32>, order: i32) -> Complex<f32> {
    let size = symbol.norm();
    if size > 0.0 {
        (symbol / size).powi(order) * size * size
    } else {
        Complex::new(0.0, 0.0)
    }
}

pub(super) fn power_order(modulation: Modulation) -> Option<i32> {
    match modulation {
        Modulation::Qpsk => Some(4),
        Modulation::Psk8 => Some(8),
        _ => None,
    }
}

pub(super) fn track_power(
    symbols: &mut [Complex<f32>],
    order: i32,
    reference: Complex<f32>,
    phases: &mut Vec<f32>,
) -> f32 {
    let residual = power_phases(symbols, order, reference, phases);
    for (index, symbol) in symbols.iter_mut().enumerate() {
        *symbol *= Complex::from_polar(1.0, -phase_at(phases, index));
    }
    residual
}

pub(super) fn power_phases(
    symbols: &[Complex<f32>],
    order: i32,
    reference: Complex<f32>,
    phases: &mut Vec<f32>,
) -> f32 {
    let step = std::f32::consts::TAU / order as f32;
    phases.clear();
    let (mut previous, mut slope) = (0.0f32, 0.0f32);
    for block in symbols.chunks(POWER_BLOCK) {
        let sum: Complex<f32> = block.iter().map(|&symbol| mth_power(symbol, order)).sum();
        let raw = (sum * reference.conj()).arg() / order as f32;
        let predicted = previous + slope;
        let phase = raw + ((predicted - raw) / step).round() * step;
        if !phases.is_empty() {
            slope += SLOPE_GAIN * ((phase - previous) - slope);
        }
        phases.push(phase);
        previous = phase;
    }
    let residual = drift(phases) / POWER_BLOCK as f32;
    smooth_blocks(phases);
    residual
}

pub(super) fn anchored_drift(anchors: &[(usize, f32)]) -> f32 {
    let count = anchors.len() as f32;
    if count < 2.0 {
        return 0.0;
    }
    let mean_position = anchors.iter().map(|a| a.0 as f32).sum::<f32>() / count;
    let mean_phase = anchors.iter().map(|a| a.1).sum::<f32>() / count;
    let (covariance, spread) = anchors
        .iter()
        .fold((0.0f32, 0.0f32), |(c, s), &(at, phase)| {
            let offset = at as f32 - mean_position;
            (c + offset * (phase - mean_phase), s + offset * offset)
        });
    if spread > 0.0 {
        covariance / spread
    } else {
        0.0
    }
}

fn drift(phases: &[f32]) -> f32 {
    let count = phases.len() as f32;
    if count < 2.0 {
        return 0.0;
    }
    let centre = (count - 1.0) / 2.0;
    let mean = phases.iter().sum::<f32>() / count;
    let (covariance, spread) =
        phases
            .iter()
            .enumerate()
            .fold((0.0f32, 0.0f32), |(c, s), (i, &phase)| {
                let offset = i as f32 - centre;
                (c + offset * (phase - mean), s + offset * offset)
            });
    covariance / spread
}

fn smooth_blocks(phases: &mut [f32]) {
    let count = phases.len();
    let mut ring = [0.0f32; POWER_SPAN];
    for block in 0..count {
        let fitted = {
            let original = |i: usize| {
                if i < block {
                    ring[i % POWER_SPAN]
                } else {
                    phases[i]
                }
            };
            let (low, high) = (
                block.saturating_sub(POWER_SPAN),
                (block + POWER_SPAN).min(count - 1),
            );
            let samples = (high - low + 1) as f32;
            let centre = (low + high) as f32 / 2.0;
            let mean = (low..=high).map(original).sum::<f32>() / samples;
            let (covariance, spread) = (low..=high).fold((0.0f32, 0.0f32), |(c, s), i| {
                let offset = i as f32 - centre;
                (c + offset * (original(i) - mean), s + offset * offset)
            });
            let slope = if spread > 0.0 {
                covariance / spread
            } else {
                0.0
            };
            mean + slope * (block as f32 - centre)
        };
        ring[block % POWER_SPAN] = phases[block];
        phases[block] = fitted;
    }
}

pub(super) fn phase_at(phases: &[f32], index: usize) -> f32 {
    let position = (index as f32 + 0.5) / POWER_BLOCK as f32 - 0.5;
    let low = (position.floor().max(0.0) as usize).min(phases.len().saturating_sub(1));
    let high = (low + 1).min(phases.len().saturating_sub(1));
    let fraction = (position - low as f32).clamp(0.0, 1.0);
    phases[low] + fraction * (phases[high] - phases[low])
}
