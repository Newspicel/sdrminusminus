use num_complex::Complex;
use sdrmm_dsp::{Ddc, combine::Combiner, covariance::Covariance};
use sdrmm_wire::{CoherentParams, CombineMode, CombinerParams};

use crate::{
    ChannelError,
    coherent::{CoherentCtx, CoherentDescriptor, CoherentOutputs, CoherentRx},
};

static DESCRIPTOR: CoherentDescriptor = CoherentDescriptor {
    type_id: "combiner",
    name: "Combiner",
    min_lanes: 2,
    needs_phase: true,
};

const PASSBAND_FRACTION: f64 = 0.8;
/// What survives of the covariance from one solve to the next. Weights that jump every update
/// modulate the very signal they are meant to hand over cleanly.
const CARRY_OVER: f32 = 0.7;
/// A canceller solves a least-squares problem whose answer is a difference of large numbers, so it
/// wants far less bias on the diagonal than a bearing estimate does.
const CANCEL_LOADING: f32 = 1e-5;

pub struct CombinerProcessor {
    params: CombinerParams,
    lanes: usize,
    sample_rate: f64,
    ddc: Vec<Ddc>,
    baseband: Vec<Vec<Complex<f32>>>,
    covariance: Covariance,
    matrix: Vec<Complex<f32>>,
    combiner: Combiner,
    weights: Vec<Complex<f32>>,
    since_update: f64,
    update_samples: f64,
}

fn params_of(params: &CoherentParams) -> Result<&CombinerParams, ChannelError> {
    match params {
        CoherentParams::Combiner(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "combiner got {} settings",
            other.type_id()
        ))),
    }
}

fn check(params: &CombinerParams, lanes: usize) -> Result<(), ChannelError> {
    if !params.valid() {
        return Err(ChannelError::InvalidSettings(
            "combiner settings are outside their allowed ranges".to_owned(),
        ));
    }
    if params.lanes as usize != lanes {
        return Err(ChannelError::InvalidSettings(format!(
            "the combiner expects {} antennas but the radio has {lanes} lanes",
            params.lanes
        )));
    }
    Ok(())
}

impl CombinerProcessor {
    fn decimated_rate(sample_rate: f64, bandwidth_hz: f64) -> f64 {
        let factor = ((sample_rate * PASSBAND_FRACTION / bandwidth_hz).floor() as usize).max(1);
        sample_rate / factor as f64
    }

    fn build_ddc(&mut self) -> Result<(), ChannelError> {
        let output_rate = Self::decimated_rate(self.sample_rate, self.params.bandwidth_hz);
        self.ddc.clear();
        for _ in 0..self.lanes {
            let ddc = Ddc::new(self.sample_rate, output_rate, self.params.offset_hz)
                .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
            self.ddc.push(ddc);
        }
        self.update_samples = self.sample_rate * f64::from(self.params.update_ms) / 1_000.0;
        Ok(())
    }

    fn solve(&mut self, out: &mut CoherentOutputs) {
        self.covariance.matrix(&mut self.matrix);
        match self.params.mode {
            CombineMode::Diversity => self.combiner.diversity(&self.matrix, &mut self.weights),
            CombineMode::Cancel => {
                if self
                    .combiner
                    .cancel(&self.matrix, &mut self.weights)
                    .is_err()
                {
                    return;
                }
            }
        }
        out.weights = Some(self.weights.clone());
        self.covariance.decay(CARRY_OVER);
    }
}

impl CoherentRx for CombinerProcessor {
    fn descriptor() -> &'static CoherentDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError> {
        let params = *params_of(params)?;
        check(&params, ctx.lanes)?;
        let mut covariance = Covariance::new(ctx.lanes);
        covariance.set_forward_backward(false);
        if params.mode == CombineMode::Cancel {
            covariance.set_loading(CANCEL_LOADING);
        }
        let mut processor = Self {
            params,
            lanes: ctx.lanes,
            sample_rate: ctx.sample_rate,
            ddc: Vec::new(),
            baseband: vec![Vec::new(); ctx.lanes],
            covariance,
            matrix: Vec::new(),
            combiner: Combiner::new(ctx.lanes)
                .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?,
            weights: Vec::new(),
            since_update: 0.0,
            update_samples: 0.0,
        };
        processor.build_ddc()?;
        Ok(processor)
    }

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError> {
        let params = *params_of(params)?;
        check(&params, self.lanes)?;
        let rebuild = params.bandwidth_hz != self.params.bandwidth_hz
            || params.offset_hz != self.params.offset_hz
            || params.update_ms != self.params.update_ms;
        self.covariance
            .set_loading(if params.mode == CombineMode::Cancel {
                CANCEL_LOADING
            } else {
                1e-3
            });
        self.params = params;
        if rebuild {
            self.build_ddc()?;
            self.covariance.reset();
            self.since_update = 0.0;
        }
        Ok(())
    }

    fn retuned(&mut self, _center_hz: f64) {
        self.covariance.reset();
        self.since_update = 0.0;
    }

    fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut CoherentOutputs) {
        let count = lanes.len().min(self.lanes);
        if count < 2 {
            return;
        }
        let mut shortest = usize::MAX;
        for (index, source) in lanes.iter().enumerate().take(count) {
            self.ddc[index].process(source, &mut self.baseband[index]);
            shortest = shortest.min(self.baseband[index].len());
        }
        if shortest == 0 {
            return;
        }
        let view: Vec<&[Complex<f32>]> = self.baseband[..count]
            .iter()
            .map(|lane| &lane[..shortest])
            .collect();
        self.covariance.accumulate(&view);
        self.since_update += lanes[0].len() as f64;
        if self.since_update >= self.update_samples && self.covariance.samples() > 0.0 {
            self.since_update = 0.0;
            self.solve(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use sdrmm_wire::CalParams;

    use super::*;

    const RATE: f64 = 1_024_000.0;
    const CENTRE_HZ: f64 = 145e6;

    fn params(mode: CombineMode) -> CoherentParams {
        CoherentParams::Combiner(CombinerParams {
            mode,
            lanes: 2,
            offset_hz: 0.0,
            bandwidth_hz: 200_000.0,
            update_ms: 100,
            cal: CalParams::default(),
        })
    }

    fn tone(len: usize, hz: f32, amplitude: f32, phase: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|index| {
                Complex::from_polar(amplitude, TAU * hz * index as f32 / RATE as f32 + phase)
            })
            .collect()
    }

    fn combined(lanes: &[Vec<Complex<f32>>], weights: &[Complex<f32>]) -> Vec<Complex<f32>> {
        (0..lanes[0].len())
            .map(|index| {
                lanes
                    .iter()
                    .zip(weights)
                    .map(|(lane, weight)| lane[index] * weight)
                    .sum()
            })
            .collect()
    }

    fn power(samples: &[Complex<f32>]) -> f32 {
        samples.iter().map(Complex::norm_sqr).sum::<f32>() / samples.len() as f32
    }

    fn run(mode: CombineMode, lanes: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        let mut processor = CombinerProcessor::new(ctx, &params(mode)).expect("builds");
        let mut out = CoherentOutputs::default();
        for _ in 0..8 {
            out.reset();
            let view: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
            processor.process(&view, &mut out);
            if let Some(weights) = out.weights.clone() {
                return weights;
            }
        }
        panic!("the combiner never solved its weights");
    }

    #[test]
    fn two_antennas_hearing_the_same_signal_are_added_in_step() {
        let len = 262_144;
        let lanes = vec![tone(len, 20_000.0, 1.0, 0.0), tone(len, 20_000.0, 1.0, 2.0)];
        let weights = run(CombineMode::Diversity, &lanes);
        let sum = combined(&lanes, &weights);
        let naive = combined(&lanes, &[Complex::new(1.0, 0.0), Complex::new(1.0, 0.0)]);
        assert!(
            power(&sum) > power(&naive) * 1.5,
            "turning the lanes into step beats adding them as they arrive: {} then {}",
            power(&naive),
            power(&sum)
        );
        assert!(
            (power(&sum) - 2.0).abs() < 0.2,
            "unit-norm weights leave the noise alone and double the signal power: {}",
            power(&sum)
        );
    }

    #[test]
    fn a_reference_antenna_puts_a_null_on_what_it_hears() {
        let len = 262_144;
        let interferer = tone(len, 30_000.0, 3.0, 0.0);
        let wanted = tone(len, -40_000.0, 0.5, 0.0);
        let leak = Complex::from_polar(0.7, 1.3);
        let lanes = vec![
            wanted
                .iter()
                .zip(&interferer)
                .map(|(a, b)| a + b)
                .collect::<Vec<_>>(),
            interferer.iter().map(|sample| sample * leak).collect(),
        ];
        let weights = run(CombineMode::Cancel, &lanes);
        let out = combined(&lanes, &weights);
        let residue = out
            .iter()
            .zip(&wanted)
            .map(|(got, want)| (got - want).norm_sqr())
            .sum::<f32>()
            / len as f32;
        assert!(
            residue < power(&wanted) / 100.0,
            "the interferer should be nulled, not merely quieter: {residue}"
        );
    }

    #[test]
    fn settings_for_another_processor_are_refused() {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        assert!(
            CombinerProcessor::new(ctx, &CoherentParams::Df(sdrmm_wire::DfParams::default()))
                .is_err()
        );
    }

    #[test]
    fn a_combiner_that_does_not_match_the_radio_is_refused() {
        let ctx = CoherentCtx {
            lanes: 3,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        let Err(ChannelError::InvalidSettings(message)) =
            CombinerProcessor::new(ctx, &params(CombineMode::Diversity))
        else {
            panic!("two antennas on a three-lane radio must be refused");
        };
        assert!(message.contains("antennas"), "{message}");
    }
}
