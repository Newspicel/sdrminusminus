use num_complex::Complex;
use sdrmm_dsp::stitch::Stitcher;
use sdrmm_wire::{CoherentParams, StitchParams};

use crate::{
    ChannelError,
    coherent::{CoherentCtx, CoherentDescriptor, CoherentOutputs, CoherentRx},
};

static DESCRIPTOR: CoherentDescriptor = CoherentDescriptor {
    type_id: "stitch",
    name: "Stitch",
    min_lanes: 2,
    needs_phase: false,
};

pub struct StitchProcessor {
    lanes: usize,
    stitcher: Stitcher,
    offsets: Vec<f64>,
}

fn params_of(params: &CoherentParams) -> Result<&StitchParams, ChannelError> {
    match params {
        CoherentParams::Stitch(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "stitch got {} settings",
            other.type_id()
        ))),
    }
}

fn check(params: &StitchParams, lanes: usize) -> Result<(), ChannelError> {
    if !params.valid() {
        return Err(ChannelError::InvalidSettings(
            "stitch settings are outside their allowed ranges".to_owned(),
        ));
    }
    if params.lanes as usize != lanes {
        return Err(ChannelError::InvalidSettings(format!(
            "the stitch expects {} lanes but {lanes} are wired",
            params.lanes
        )));
    }
    Ok(())
}

impl CoherentRx for StitchProcessor {
    fn descriptor() -> &'static CoherentDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError> {
        check(params_of(params)?, ctx.lanes)?;
        let stitcher = Stitcher::new(ctx.lanes, ctx.sample_rate)
            .map_err(|error| ChannelError::InvalidSettings(error.to_string()))?;
        Ok(Self {
            lanes: ctx.lanes,
            stitcher,
            offsets: vec![0.0; ctx.lanes],
        })
    }

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError> {
        check(params_of(params)?, self.lanes)
    }

    fn retuned(&mut self, _center_hz: f64) {
        self.stitcher.reset();
    }

    fn tuned(&mut self, lanes_hz: &[f64], center_hz: f64) {
        for (offset, lane_hz) in self.offsets.iter_mut().zip(lanes_hz) {
            *offset = lane_hz - center_hz;
        }
        self.stitcher.retune(&self.offsets);
    }

    fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut CoherentOutputs) {
        if lanes.len() != self.lanes {
            return;
        }
        self.stitcher.process(lanes, &mut out.wide);
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_wire::StitchMode;

    use super::*;

    const RATE: f64 = 1_024_000.0;

    fn params(lanes: u32) -> CoherentParams {
        CoherentParams::Stitch(StitchParams {
            mode: StitchMode::Auto,
            lanes,
        })
    }

    fn ctx(lanes: usize) -> CoherentCtx {
        CoherentCtx {
            lanes,
            sample_rate: RATE,
            center_hz: 100e6,
        }
    }

    fn tone(len: usize, hz: f64) -> Vec<Complex<f32>> {
        (0..len)
            .map(|index| {
                let phase = TAU * hz * index as f64 / RATE;
                Complex::new(phase.cos() as f32, phase.sin() as f32)
            })
            .collect()
    }

    #[test]
    fn a_lane_count_that_does_not_match_the_wiring_is_refused() {
        assert!(StitchProcessor::new(ctx(3), &params(2)).is_err());
    }

    #[test]
    fn two_lanes_come_out_twice_as_fast() {
        let mut processor = StitchProcessor::new(ctx(2), &params(2)).expect("builds");
        processor.tuned(&[99.6e6, 100.4e6], 100e6);
        let lane = tone(65_536, 10_000.0);
        let mut out = CoherentOutputs::default();
        processor.process(&[&lane, &lane], &mut out);
        let expected = lane.len() * 2;
        assert!(
            out.wide.len() + 8_192 >= expected && out.wide.len() <= expected,
            "{} samples for {expected}",
            out.wide.len()
        );
    }

    #[test]
    fn reset_clears_the_wide_output() {
        let mut out = CoherentOutputs::default();
        out.wide.push(Complex::new(1.0, 0.0));
        out.reset();
        assert!(out.wide.is_empty());
    }
}
