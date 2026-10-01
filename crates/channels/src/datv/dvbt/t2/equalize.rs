use num_complex::Complex;
use sdrmm_dsp::fft::Transform;

use super::{
    DecodeError,
    mapping::{Carrier, Mapping},
};
use crate::datv::dvbt::wiener::{Design, FrequencyFilter, TAPS, measure_span};

const PROFILE: usize = 16_384;
const SPAN_MARGIN: f32 = 4.0;
const ALIAS_SHARE: f32 = 0.9;
const PILOT_NOISE_SHARE: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub miso: bool,
    pub history: usize,
    pub guard: usize,
}

pub struct Equalizer {
    observations: [Vec<Complex<f32>>; 2],
    ages: [Vec<usize>; 2],
    estimates: [Vec<Complex<f32>>; 2],
    data_carriers: Vec<usize>,
    epoch: usize,
    pub noise: f32,
    filters: [FrequencyFilter; 2],
    profile: Vec<Complex<f32>>,
    inverse: Transform,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self {
            observations: std::array::from_fn(|_| vec![Complex::default(); 32768]),
            ages: std::array::from_fn(|_| vec![0; 32768]),
            estimates: std::array::from_fn(|_| vec![Complex::default(); 27841]),
            data_carriers: Vec::with_capacity(27841),
            epoch: 1,
            noise: 0.01,
            filters: std::array::from_fn(|_| FrequencyFilter::new()),
            profile: vec![Complex::default(); PROFILE],
            inverse: Transform::inverse(PROFILE),
        }
    }
}

impl Equalizer {
    pub fn reset(&mut self) {
        self.ages.iter_mut().for_each(|ages| ages.fill(0));
        self.epoch = 1;
        self.noise = 0.01;
    }

    pub fn shift(&mut self, fft: usize, samples: isize) {
        if samples == 0 {
            return;
        }
        let cycles = samples as f64 / fft as f64;
        for observations in &mut self.observations {
            for (bin, value) in observations[..fft].iter_mut().enumerate() {
                let carrier = if bin < fft / 2 {
                    bin as f64
                } else {
                    bin as f64 - fft as f64
                };
                let (sin, cos) = (std::f64::consts::TAU * cycles * carrier).sin_cos();
                *value *= Complex::new(cos as f32, sin as f32);
            }
        }
    }

    pub fn decode(
        &mut self,
        spectrum: &[Complex<f32>],
        map: &Mapping,
        shape: Shape,
        output: &mut [Complex<f32>],
        gains: &mut [f32],
    ) -> Result<(), DecodeError> {
        let Shape {
            miso,
            history,
            guard,
        } = shape;
        if spectrum.len() != map.fft
            || output.len() < map.data
            || gains.len() < map.data
            || spectrum.iter().any(|p| !p.norm_sqr().is_finite())
        {
            return Err(DecodeError::Length);
        }
        self.epoch += 1;
        self.track_phase(spectrum, map, miso, history);
        for k in 0..map.carriers {
            if let Carrier::Pilot { inverted, .. } = map.map[k] {
                let group = usize::from(miso && inverted);
                let bin = bin(map, k);
                self.observations[group][bin] = spectrum[bin] / map.pilots[k];
                self.ages[group][bin] = self.epoch;
            }
        }
        for group in 0..=usize::from(miso) {
            if !self.smooth(map, group, history, guard) {
                self.interpolate(map, group, history)?;
            }
        }
        self.data_carriers.clear();
        for k in 0..map.carriers {
            if map.map[k] == Carrier::Data {
                self.data_carriers.push(k);
            }
        }
        if miso {
            self.alamouti(spectrum, map, output)?;
            gains[..map.data].fill(1.0);
        } else {
            for (i, &k) in self.data_carriers.iter().enumerate() {
                let h = self.estimates[0][k];
                output[i] = spectrum[bin(map, k)] * h.conj() / h.norm_sqr().max(1e-12);
                gains[i] = h.norm_sqr();
            }
            let count = self.data_carriers.len().max(1) as f32;
            let mean = gains[..self.data_carriers.len()].iter().sum::<f32>() / count;
            if mean > 0.0 && mean.is_finite() {
                for gain in &mut gains[..self.data_carriers.len()] {
                    *gain /= mean;
                }
            }
        }
        Ok(())
    }

    fn track_phase(
        &mut self,
        spectrum: &[Complex<f32>],
        map: &Mapping,
        miso: bool,
        history: usize,
    ) {
        let mut correlation = Complex::<f32>::default();
        let mut errors = 0.0;
        let mut power = 0.0;
        for k in 0..map.carriers {
            if let Carrier::Pilot { inverted, .. } = map.map[k] {
                let group = usize::from(miso && inverted);
                let bin = bin(map, k);
                if self.ages[group][bin] != 0 && self.epoch - self.ages[group][bin] <= history + 1 {
                    let old = self.observations[group][bin];
                    let new = spectrum[bin] / map.pilots[k];
                    correlation += new * old.conj();
                    errors += (new - old).norm_sqr();
                    power += old.norm_sqr();
                }
            }
        }
        if power > 1e-12 {
            self.noise = (errors / power).clamp(0.0001, 2.0);
            let rotation = Complex::from_polar(1.0, correlation.arg());
            for group in 0..=usize::from(miso) {
                for k in 0..map.carriers {
                    self.observations[group][bin(map, k)] *= rotation;
                }
            }
        }
    }

    fn smooth(&mut self, map: &Mapping, group: usize, history: usize, guard: usize) -> bool {
        let (grid, start) = (map.grid, map.grid_start);
        if grid == 0 || start >= map.carriers {
            return false;
        }
        let count = (map.carriers - 1 - start) / grid + 1;
        let fresh = |k: usize| {
            let age = self.ages[group][bin(map, k)];
            age != 0 && self.epoch - age <= history
        };
        if count < TAPS || !(start..map.carriers).step_by(grid).all(fresh) {
            return false;
        }
        let observations = &self.observations[group];
        let anchor = |m: usize| observations[bin(map, start + m * grid)];
        let period = map.fft as f32 / grid as f32;
        let (first, last) = measure_span(
            &mut self.profile,
            &mut self.inverse,
            anchor,
            count,
            period,
            guard,
        )
        .unwrap_or((0.0, guard as f32));
        let filter = &mut self.filters[group];
        filter.prepare(Design {
            spacing: grid,
            fft: map.fft,
            first: first - SPAN_MARGIN,
            width: (last - first + 2.0 * SPAN_MARGIN).min(ALIAS_SHARE * period),
            noise: self.noise * PILOT_NOISE_SHARE,
        });
        filter.apply(
            anchor,
            start,
            count,
            &mut self.estimates[group][..map.carriers],
        );
        true
    }

    fn interpolate(
        &mut self,
        map: &Mapping,
        group: usize,
        history: usize,
    ) -> Result<(), DecodeError> {
        let mut left = None;
        for right in 0..map.carriers {
            let bin = bin(map, right);
            let age = self.ages[group][bin];
            if age == 0 || self.epoch - age > history {
                continue;
            }
            let value = self.observations[group][bin];
            if let Some((start, previous)) = left {
                for k in start..right {
                    let fraction = (k - start) as f32 / (right - start) as f32;
                    self.estimates[group][k] = previous + (value - previous) * fraction;
                }
            } else {
                self.estimates[group][..right].fill(value);
            }
            self.estimates[group][right] = value;
            left = Some((right, value));
        }
        let (last, value) = left.ok_or(DecodeError::Acquisition)?;
        self.estimates[group][last..map.carriers].fill(value);
        Ok(())
    }

    fn alamouti(
        &self,
        spectrum: &[Complex<f32>],
        map: &Mapping,
        output: &mut [Complex<f32>],
    ) -> Result<(), DecodeError> {
        if !map.data.is_multiple_of(2) {
            return Err(DecodeError::Parameters);
        }
        for (pair, carriers) in self.data_carriers.as_chunks::<2>().0.iter().enumerate() {
            let [k0, k1] = *carriers;
            let a = (self.estimates[0][k0] + self.estimates[1][k0]) * 0.5;
            let b = (self.estimates[0][k0] - self.estimates[1][k0]) * 0.5;
            let c = (self.estimates[0][k1] + self.estimates[1][k1]) * 0.5;
            let d = (self.estimates[0][k1] - self.estimates[1][k1]) * 0.5;
            let y0 = spectrum[bin(map, k0)];
            let y1 = spectrum[bin(map, k1)].conj();
            let determinant = a * c.conj() + b * d.conj();
            let inverse = determinant.conj() / determinant.norm_sqr().max(1e-12);
            output[pair * 2] = (c.conj() * y0 + b * y1) * inverse;
            output[pair * 2 + 1] = ((a * y1 - d.conj() * y0) * inverse).conj();
        }
        Ok(())
    }
}

fn bin(map: &Mapping, carrier: usize) -> usize {
    (carrier + map.fft - map.carriers / 2) % map.fft
}
