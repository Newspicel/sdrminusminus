use num_complex::Complex;

use super::{
    DecodeError,
    mapping::{Carrier, Mapping},
};

pub struct Equalizer {
    observations: [Vec<Complex<f32>>; 2],
    ages: [Vec<usize>; 2],
    estimates: [Vec<Complex<f32>>; 2],
    data_carriers: Vec<usize>,
    epoch: usize,
    pub noise: f32,
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
        }
    }
}

impl Equalizer {
    pub fn reset(&mut self) {
        self.ages.iter_mut().for_each(|ages| ages.fill(0));
        self.epoch = 1;
        self.noise = 0.01;
    }

    pub fn decode(
        &mut self,
        spectrum: &[Complex<f32>],
        map: &Mapping,
        miso: bool,
        history: usize,
        output: &mut [Complex<f32>],
    ) -> Result<(), DecodeError> {
        if spectrum.len() != map.fft
            || output.len() < map.data
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
            self.interpolate(map, group, history)?;
        }
        self.data_carriers.clear();
        for k in 0..map.carriers {
            if map.map[k] == Carrier::Data {
                self.data_carriers.push(k);
            }
        }
        if miso {
            self.alamouti(spectrum, map, output)?;
        } else {
            for (i, &k) in self.data_carriers.iter().enumerate() {
                let h = self.estimates[0][k];
                output[i] = spectrum[bin(map, k)] * h.conj() / h.norm_sqr().max(1e-12);
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
