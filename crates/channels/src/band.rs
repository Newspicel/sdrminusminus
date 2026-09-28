use num_complex::Complex;
use sdrmm_dsp::Ddc;

use crate::ChannelError;
use crate::array_processor::{ArrayBlock, ArrayCtx, CorrectionView, LaneFormat, MAX_LANES};

pub const PASSBAND_FRACTION: f64 = 0.8;
pub const BAND_MARGIN: usize = 64;

const WARM_BLOCKS: usize = 3;
const ONE: Complex<f32> = Complex::new(1.0, 0.0);

#[must_use]
pub fn decimation(input_rate: f64, bandwidth_hz: Option<f64>) -> usize {
    bandwidth_hz.map_or(1, |bandwidth| {
        ((input_rate * PASSBAND_FRACTION / bandwidth).floor() as usize).max(1)
    })
}

#[must_use]
pub fn band_rate(input_rate: f64, bandwidth_hz: Option<f64>) -> f64 {
    input_rate / decimation(input_rate, bandwidth_hz) as f64
}

#[must_use]
pub fn band_capacity(max_block: usize, input_rate: f64, output_rate: f64) -> usize {
    (max_block as f64 * output_rate / input_rate).ceil() as usize + BAND_MARGIN
}

#[must_use]
pub fn band_lane_format(
    ctx: &ArrayCtx<'_>,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
) -> LaneFormat {
    let sample_rate = band_rate(ctx.sample_rate, bandwidth_hz);
    LaneFormat {
        center_hz: ctx.center_hz + bandwidth_hz.map_or(0.0, |_| offset_hz),
        sample_rate,
        capacity: band_capacity(ctx.max_block, ctx.sample_rate, sample_rate),
    }
}

pub struct LaneBand {
    lanes: usize,
    ddc: Vec<Ddc>,
    buffers: Vec<Vec<Complex<f32>>>,
    input_rate: f64,
    output_rate: f64,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
    factors: [Complex<f32>; MAX_LANES],
    factors_for: Option<u32>,
}

impl LaneBand {
    pub fn new(
        lanes: usize,
        input_rate: f64,
        offset_hz: f64,
        bandwidth_hz: Option<f64>,
        max_block: usize,
    ) -> Result<Self, ChannelError> {
        check(lanes, input_rate, offset_hz, bandwidth_hz)?;
        let output_rate = band_rate(input_rate, bandwidth_hz);
        let ddc = match bandwidth_hz {
            Some(_) => (0..lanes)
                .map(|_| {
                    Ddc::new(input_rate, output_rate, offset_hz)
                        .map_err(|_| ChannelError::Refused("Rate out of range"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let mut band = Self {
            lanes,
            ddc,
            buffers: (0..lanes)
                .map(|_| Vec::with_capacity(max_block + BAND_MARGIN))
                .collect(),
            input_rate,
            output_rate,
            offset_hz,
            bandwidth_hz,
            factors: [ONE; MAX_LANES],
            factors_for: None,
        };
        band.warm(max_block);
        Ok(band)
    }

    #[must_use]
    pub const fn output_rate(&self) -> f64 {
        self.output_rate
    }

    #[must_use]
    pub const fn input_rate(&self) -> f64 {
        self.input_rate
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.lanes
    }

    #[must_use]
    pub fn center_offset_hz(&self) -> f64 {
        self.bandwidth_hz.map_or(0.0, |_| self.offset_hz)
    }

    #[must_use]
    pub fn independent_fraction(&self) -> f64 {
        self.bandwidth_hz
            .map_or(1.0, |bandwidth| (bandwidth / self.output_rate).min(1.0))
    }

    pub fn set_offset(&mut self, offset_hz: f64) {
        self.offset_hz = offset_hz;
        for ddc in &mut self.ddc {
            ddc.set_offset(offset_hz);
        }
        self.factors_for = None;
    }

    pub fn reset(&mut self) {
        for ddc in &mut self.ddc {
            ddc.reset();
        }
        for buffer in &mut self.buffers {
            buffer.clear();
        }
        self.factors_for = None;
    }

    pub fn process<'a>(
        &'a mut self,
        block: &ArrayBlock<'a>,
        views: &mut [&'a [Complex<f32>]; MAX_LANES],
    ) -> usize {
        if block.lanes.len() != self.lanes {
            return 0;
        }
        let correct = !block.corrected;
        if correct {
            self.refresh(&block.correction);
        }
        if self.ddc.is_empty() && !correct {
            for (view, &lane) in views.iter_mut().zip(block.lanes) {
                *view = lane;
            }
            return shortest(&views[..self.lanes]);
        }
        self.fill(block.lanes, correct);
        let this: &'a Self = self;
        for (view, buffer) in views.iter_mut().zip(&this.buffers) {
            *view = buffer;
        }
        shortest(&views[..this.lanes])
    }

    fn fill(&mut self, lanes: &[&[Complex<f32>]], correct: bool) {
        for (lane, input) in lanes.iter().enumerate() {
            let buffer = &mut self.buffers[lane];
            match self.ddc.get_mut(lane) {
                Some(ddc) => ddc.process(input, buffer),
                None => {
                    buffer.clear();
                    buffer.extend_from_slice(input);
                }
            }
            if correct {
                let factor = self.factors[lane];
                for sample in buffer.iter_mut() {
                    *sample *= factor;
                }
            }
        }
    }

    fn refresh(&mut self, correction: &CorrectionView<'_>) {
        if self.factors_for == Some(correction.generation) {
            return;
        }
        let at = self.center_offset_hz();
        for (lane, factor) in self.factors.iter_mut().enumerate().take(self.lanes) {
            *factor = correction.response(lane, at);
        }
        self.factors_for = Some(correction.generation);
    }

    fn warm(&mut self, max_block: usize) {
        if self.ddc.is_empty() || max_block == 0 {
            return;
        }
        let silence = vec![Complex::default(); max_block];
        for (ddc, buffer) in self.ddc.iter_mut().zip(&mut self.buffers) {
            for _ in 0..WARM_BLOCKS {
                ddc.process(&silence, buffer);
            }
            ddc.reset();
            buffer.clear();
        }
    }
}

fn check(
    lanes: usize,
    input_rate: f64,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
) -> Result<(), ChannelError> {
    if lanes == 0 {
        return Err(ChannelError::Refused("Too few elements"));
    }
    if lanes > MAX_LANES {
        return Err(ChannelError::Refused("Too many elements"));
    }
    if !(input_rate.is_finite() && input_rate > 0.0) {
        return Err(ChannelError::Refused("Rate out of range"));
    }
    if !offset_hz.is_finite() {
        return Err(ChannelError::Refused("Offset out of range"));
    }
    if bandwidth_hz.is_some_and(|bandwidth| !(bandwidth.is_finite() && bandwidth > 0.0)) {
        return Err(ChannelError::Refused("Bandwidth out of range"));
    }
    Ok(())
}

fn shortest(views: &[&[Complex<f32>]]) -> usize {
    views.iter().map(|view| view.len()).min().unwrap_or(0)
}

#[cfg(test)]
mod tests;
