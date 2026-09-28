use std::f64::consts::TAU;

use num_complex::Complex;

use crate::{fft::FftPair, window::hann};

const STRUCTURE_RATIO: f64 = 2.0;
const DC_GUARD: usize = 2;

pub(super) struct SpectralShift {
    fft: FftPair,
    window: Vec<f32>,
    block: Vec<Complex<f32>>,
    reference: Vec<f64>,
    lane: Vec<f64>,
    sorted: Vec<f64>,
}

impl SpectralShift {
    pub(super) fn new(bins: usize) -> Self {
        let bins = bins.max(8);
        Self {
            fft: FftPair::new(bins),
            window: hann(bins),
            block: vec![Complex::default(); bins],
            reference: vec![0.0; bins],
            lane: vec![0.0; bins],
            sorted: Vec::with_capacity(bins),
        }
    }

    pub(super) fn bins(&self) -> usize {
        self.fft.len()
    }

    pub(super) fn estimate(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        reach: usize,
    ) -> f64 {
        let bins = self.bins();
        welch(
            &mut self.fft,
            &self.window,
            &mut self.block,
            reference,
            &mut self.reference,
        );
        welch(
            &mut self.fft,
            &self.window,
            &mut self.block,
            lane,
            &mut self.lane,
        );
        if !structured(&mut self.reference, &mut self.sorted)
            || !structured(&mut self.lane, &mut self.sorted)
        {
            return 0.0;
        }
        centre(&mut self.reference);
        centre(&mut self.lane);
        best_shift(&self.reference, &self.lane, reach.min(bins / 2 - 1))
            .map_or(0.0, |shift| shift / bins as f64)
    }
}

fn welch(
    fft: &mut FftPair,
    window: &[f32],
    block: &mut [Complex<f32>],
    data: &[Complex<f32>],
    psd: &mut [f64],
) {
    let bins = fft.len();
    psd.fill(0.0);
    let mut start = 0;
    while start + bins <= data.len() {
        for ((value, sample), weight) in
            block.iter_mut().zip(&data[start..start + bins]).zip(window)
        {
            *value = sample * weight;
        }
        fft.forward(block);
        for (power, value) in psd.iter_mut().zip(block.iter()) {
            *power += f64::from(value.norm_sqr());
        }
        start += bins / 2;
    }
}

fn structured(psd: &mut [f64], sorted: &mut Vec<f64>) -> bool {
    sorted.clear();
    sorted.extend_from_slice(psd);
    let middle = sorted.len() / 2;
    let median = *sorted.select_nth_unstable_by(middle, f64::total_cmp).1;
    if !median.is_finite() || median <= 0.0 {
        return false;
    }
    let bins = psd.len();
    for offset in 0..=DC_GUARD {
        psd[offset] = median;
        psd[(bins - offset) % bins] = median;
    }
    psd.iter().any(|&power| power >= STRUCTURE_RATIO * median)
}

fn centre(psd: &mut [f64]) {
    let mean = psd.iter().sum::<f64>() / psd.len() as f64;
    for power in psd.iter_mut() {
        *power -= mean;
    }
}

fn best_shift(reference: &[f64], lane: &[f64], reach: usize) -> Option<f64> {
    let bins = reference.len() as isize;
    let reach = reach as isize;
    let score = |shift: isize| -> f64 {
        reference
            .iter()
            .enumerate()
            .map(|(bin, value)| value * lane[(bin as isize + shift).rem_euclid(bins) as usize])
            .sum()
    };
    let (best, value) = (-reach..=reach)
        .map(|shift| (shift, score(shift)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let fraction = if best.abs() < reach {
        parabolic(score(best - 1), value, score(best + 1))
    } else {
        0.0
    };
    Some(best as f64 + fraction)
}

pub(super) fn parabolic(left: f64, centre: f64, right: f64) -> f64 {
    let denominator = left - 2.0 * centre + right;
    if denominator.abs() <= f64::MIN_POSITIVE {
        return 0.0;
    }
    (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
}

pub(super) fn derotate(
    source: &[Complex<f32>],
    first_index: usize,
    cycles_per_sample: f64,
    out: &mut [Complex<f32>],
) {
    if cycles_per_sample == 0.0 {
        out[..source.len()].copy_from_slice(source);
        return;
    }
    for (index, (value, sample)) in out.iter_mut().zip(source).enumerate() {
        let turns = (cycles_per_sample * (first_index + index) as f64).fract();
        let (sin, cos) = (-TAU * turns).sin_cos();
        *value = sample * Complex::new(cos as f32, sin as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array_sync::signals::Gaussian;

    fn tones(
        len: usize,
        tones: &[(f64, f32)],
        shift: f64,
        noise: &mut Gaussian,
    ) -> Vec<Complex<f32>> {
        (0..len)
            .map(|n| {
                tones
                    .iter()
                    .map(|&(frequency, amplitude)| {
                        let turns = ((frequency + shift) * n as f64).fract();
                        Complex::from_polar(amplitude, (TAU * turns) as f32)
                    })
                    .sum::<Complex<f32>>()
                    + noise.sample()
            })
            .collect()
    }

    #[test]
    fn a_spectral_shift_is_found_to_a_fraction_of_a_bin() {
        let mut noise = Gaussian::new(40);
        let lines = [(-0.21, 0.2), (0.047, 0.2), (0.33, 0.2)];
        let reference = tones(32_768, &lines, 0.0, &mut noise);
        let lane = tones(65_536, &lines, 0.0137, &mut noise);
        let mut shift = SpectralShift::new(1024);
        let estimate = shift.estimate(&reference, &lane, 140);
        assert!(
            (estimate - 0.0137).abs() < 0.2 / 1024.0,
            "{}",
            estimate * 1024.0
        );
    }

    #[test]
    fn a_flat_spectrum_gives_no_shift() {
        let mut noise = Gaussian::new(41);
        let reference = noise.block(32_768);
        let lane = noise.block(65_536);
        let mut shift = SpectralShift::new(1024);
        assert!(shift.estimate(&reference, &lane, 140).abs() < f64::EPSILON);
    }

    #[test]
    fn a_dc_spike_alone_is_no_structure() {
        let mut noise = Gaussian::new(42);
        let spike = |len: usize, noise: &mut Gaussian| -> Vec<Complex<f32>> {
            (0..len)
                .map(|_| noise.sample() + Complex::new(0.5, 0.0))
                .collect()
        };
        let reference = spike(32_768, &mut noise);
        let lane = spike(65_536, &mut noise);
        let mut shift = SpectralShift::new(1024);
        assert!(shift.estimate(&reference, &lane, 140).abs() < f64::EPSILON);
    }

    #[test]
    fn derotation_removes_a_known_offset() {
        let source: Vec<Complex<f32>> = (0..1000)
            .map(|n| Complex::from_polar(1.0, (TAU * 0.01 * (n + 50) as f64) as f32))
            .collect();
        let mut out = vec![Complex::default(); source.len()];
        derotate(&source, 50, 0.01, &mut out);
        assert!(
            out.iter()
                .all(|value| (value - Complex::new(1.0, 0.0)).norm() < 1e-4)
        );
    }
}
