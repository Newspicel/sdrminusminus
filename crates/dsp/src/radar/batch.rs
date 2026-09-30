use std::f64::consts::TAU;

use num_complex::Complex;

use super::RadarDspError;
use super::wiener::WeightTable;
use crate::fft::FftPair;

type C32 = Complex<f32>;

pub const MIN_FFT: usize = 16;
pub const MAX_FFT: usize = 1 << 20;
pub const MAX_SURVEILLANCE: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchShape {
    pub batches: usize,
    pub batch_len: usize,
    pub gates: usize,
    pub lead: usize,
    pub taps: usize,
    pub lanes: usize,
    pub fft_len: usize,
}

impl BatchShape {
    pub fn new(
        batches: usize,
        batch_len: usize,
        gates: usize,
        lead: usize,
        taps: usize,
        lanes: usize,
    ) -> Result<Self, RadarDspError> {
        let empty = batches == 0 || batch_len == 0 || gates == 0;
        if empty || !(1..=MAX_SURVEILLANCE).contains(&lanes) || (lead > 0 && taps == 0) {
            return Err(RadarDspError::Shape);
        }
        let mut shape = Self {
            batches,
            batch_len,
            gates,
            lead,
            taps,
            lanes,
            fft_len: 0,
        };
        let reach = if shape.eca() {
            batch_len + shape.span() + shape.order() - 2
        } else {
            batch_len + shape.span() - 1
        };
        let fft_len = reach
            .checked_next_power_of_two()
            .ok_or(RadarDspError::Size)?
            .max(MIN_FFT);
        if fft_len > MAX_FFT {
            return Err(RadarDspError::Size);
        }
        shape.fft_len = fft_len;
        Ok(shape)
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.lead + self.taps
    }

    #[must_use]
    pub const fn eca(&self) -> bool {
        self.order() > 0
    }

    #[must_use]
    pub const fn span(&self) -> usize {
        if self.eca() {
            let reach = if self.gates > self.taps {
                self.gates
            } else {
                self.taps
            };
            reach + self.lead
        } else {
            self.gates
        }
    }

    #[must_use]
    pub const fn pre(&self) -> usize {
        self.order().saturating_sub(1)
    }

    #[must_use]
    pub const fn post(&self) -> usize {
        self.span() - 1
    }

    #[must_use]
    pub const fn samples(&self) -> usize {
        self.batches * self.batch_len
    }

    #[must_use]
    pub const fn window(&self) -> usize {
        self.samples() + self.pre() + self.post()
    }

    #[must_use]
    pub fn energy(&self, window: &[C32], batch: usize) -> f64 {
        let start = self.pre() + batch * self.batch_len;
        window
            .get(start..start + self.batch_len)
            .map_or(0.0, |samples| {
                samples.iter().map(|x| f64::from(x.norm_sqr())).sum()
            })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DopplerTaper {
    #[default]
    Hann,
    BlackmanHarris,
    Rectangular,
}

impl DopplerTaper {
    #[must_use]
    pub const fn enbw(self) -> f64 {
        match self {
            Self::Hann => 1.5,
            Self::BlackmanHarris => 2.0,
            Self::Rectangular => 1.0,
        }
    }

    pub fn fill(self, out: &mut [f32]) {
        let len = out.len() as f64;
        for (index, value) in out.iter_mut().enumerate() {
            let x = TAU * index as f64 / len;
            *value = match self {
                Self::Hann => 0.5 - 0.5 * x.cos(),
                Self::BlackmanHarris => {
                    0.358_75 - 0.488_29 * x.cos() + 0.141_28 * (2.0 * x).cos()
                        - 0.011_68 * (3.0 * x).cos()
                }
                Self::Rectangular => 1.0,
            } as f32;
        }
    }
}

#[derive(Clone, Copy)]
pub struct WeightsAt<'a> {
    table: Option<&'a WeightTable>,
    lane: usize,
    batch: usize,
}

impl<'a> WeightsAt<'a> {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            table: None,
            lane: 0,
            batch: 0,
        }
    }

    pub(crate) const fn of(table: &'a WeightTable, lane: usize, batch: usize) -> Self {
        Self {
            table: Some(table),
            lane,
            batch,
        }
    }
}

pub struct BatchKernel {
    shape: BatchShape,
    fft: FftPair,
    doppler: FftPair,
    work: Vec<C32>,
    kernel: Vec<C32>,
}

impl BatchKernel {
    #[must_use]
    pub fn new(shape: BatchShape) -> Self {
        Self {
            shape,
            fft: FftPair::new(shape.fft_len),
            doppler: FftPair::new(shape.batches),
            work: vec![C32::default(); shape.fft_len],
            kernel: vec![C32::default(); shape.fft_len],
        }
    }

    #[must_use]
    pub const fn shape(&self) -> BatchShape {
        self.shape
    }

    pub fn reference(
        &mut self,
        window: &[C32],
        batch: usize,
        spectrum: &mut [C32],
        model: &mut [C32],
    ) -> Result<(), RadarDspError> {
        let shape = self.shape;
        let m = shape.fft_len;
        if !self.fits(window, batch) || spectrum.len() < m || (shape.eca() && model.len() < m) {
            return Err(RadarDspError::Shape);
        }
        let spectrum = &mut spectrum[..m];
        let start = shape.pre() + batch * shape.batch_len;
        load(spectrum, &window[start..start + shape.batch_len]);
        self.fft.forward(spectrum);
        if shape.eca() {
            let model = &mut model[..m];
            let from = batch * shape.batch_len;
            let len = shape.batch_len + shape.span() + shape.order() - 2;
            load(model, &window[from..from + len]);
            self.fft.forward(model);
            for (value, r) in model.iter_mut().zip(spectrum.iter()) {
                *value *= r.conj();
            }
        }
        Ok(())
    }

    pub fn surveillance(
        &mut self,
        window: &[C32],
        batch: usize,
        spectrum: &[C32],
        product: &mut [C32],
    ) -> Result<(), RadarDspError> {
        let shape = self.shape;
        let m = shape.fft_len;
        if !self.fits(window, batch) || spectrum.len() < m || product.len() < m {
            return Err(RadarDspError::Shape);
        }
        let product = &mut product[..m];
        let start = shape.pre() + batch * shape.batch_len - shape.lead;
        let len = shape.batch_len + shape.span() - 1;
        load(product, &window[start..start + len]);
        self.fft.forward(product);
        for (value, r) in product.iter_mut().zip(spectrum) {
            *value *= r.conj();
        }
        Ok(())
    }

    pub fn residual(
        &mut self,
        product: &[C32],
        model: &[C32],
        weights: WeightsAt<'_>,
        gates: &mut [C32],
    ) -> Result<(), RadarDspError> {
        let shape = self.shape;
        let m = shape.fft_len;
        if product.len() < m || gates.len() < shape.gates {
            return Err(RadarDspError::Shape);
        }
        let combined = match weights.table {
            Some(table) if shape.eca() => {
                if model.len() < m {
                    return Err(RadarDspError::Shape);
                }
                table.combine(weights.lane, weights.batch, &mut self.kernel)?
            }
            _ => false,
        };
        if combined {
            for (((out, q), w), p) in self
                .work
                .iter_mut()
                .zip(product)
                .zip(&self.kernel)
                .zip(model)
            {
                *out = q - w * p;
            }
        } else {
            self.work.copy_from_slice(&product[..m]);
        }
        self.fft.inverse_scaled(&mut self.work);
        let lead = shape.lead;
        gates[..shape.gates].copy_from_slice(&self.work[lead..lead + shape.gates]);
        Ok(())
    }

    pub fn doppler(&mut self, series: &mut [C32], window: &[f32]) -> Result<(), RadarDspError> {
        let batches = self.shape.batches;
        if series.len() < batches || window.len() < batches {
            return Err(RadarDspError::Shape);
        }
        let series = &mut series[..batches];
        for (value, weight) in series.iter_mut().zip(window) {
            *value *= *weight;
        }
        self.doppler.forward(series);
        series.rotate_right(batches / 2);
        Ok(())
    }

    fn fits(&self, window: &[C32], batch: usize) -> bool {
        batch < self.shape.batches && window.len() >= self.shape.window()
    }
}

fn load(buffer: &mut [C32], samples: &[C32]) {
    let (head, tail) = buffer.split_at_mut(samples.len());
    head.copy_from_slice(samples);
    tail.fill(C32::default());
}

#[cfg(test)]
pub(crate) mod tests;
