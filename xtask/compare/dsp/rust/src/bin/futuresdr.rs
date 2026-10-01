use std::f64::consts::TAU;

use futuredsp::{
    DecimatingFirFilter, FirFilter, PolyphaseResamplingFir, Rotator, firdes, prelude::Filter,
};
use num_complex::Complex32;
use sdrmm_compare_dsp::{
    CUTOFF, DDC_INPUT_RATE, DDC_OFFSET, DDC_OUTPUT_RATE, DECIMATION, TAPS, Timing, keep, ratio,
    run, signal, taps,
};

const RESAMPLE_INTERP: usize = 160;
const RESAMPLE_DECIM: usize = 147;
const HALF_POLYPHASE: usize = 12;
const MAX_RIPPLE: f64 = 0.0001;
const DDC_DECIMATION: usize = 25;
const DDC_INTERP: usize = 3;
const DDC_DECIM: usize = 50;
const DDC_RIPPLE: f64 = 0.001;

struct Carry<F> {
    filter: F,
    buffer: Vec<Complex32>,
    output: Vec<Complex32>,
}

impl<F: Filter<Complex32, Complex32, f32>> Carry<F> {
    fn new(filter: F, output: usize) -> Self {
        let history = filter.length().saturating_sub(1);
        Self {
            filter,
            buffer: vec![Complex32::default(); history],
            output: vec![Complex32::default(); output],
        }
    }

    fn process(&mut self, input: &[Complex32]) -> usize {
        self.buffer.extend_from_slice(input);
        let (consumed, produced, _) = self.filter.filter(&self.buffer, &mut self.output);
        self.buffer.drain(..consumed);
        produced
    }
}

fn padded(input: &[Complex32]) -> Vec<Complex32> {
    let mut padded = vec![Complex32::default(); TAPS - 1];
    padded.extend_from_slice(input);
    padded
}

fn filters(timing: &Timing, input: &[Complex32]) {
    let taps = taps(TAPS, CUTOFF);
    let padded = padded(input);
    let mut out = vec![Complex32::default(); input.len()];
    let fir = FirFilter::<Complex32, Complex32, _>::new(taps.clone());
    run("fir", timing.block, timing, || {
        fir.filter(&padded, &mut out);
        keep(&out);
    });
    let decimator = DecimatingFirFilter::<Complex32, Complex32, _>::new(DECIMATION, taps);
    run("decimate", timing.block, timing, || {
        decimator.filter(&padded, &mut out[..input.len() / DECIMATION]);
        keep(&out);
    });
    let bank = firdes::kaiser::multirate::<f32>(
        RESAMPLE_INTERP,
        RESAMPLE_DECIM,
        HALF_POLYPHASE,
        MAX_RIPPLE,
    );
    let resampler = PolyphaseResamplingFir::<Complex32, Complex32, _>::new(
        RESAMPLE_INTERP,
        RESAMPLE_DECIM,
        bank,
    );
    let mut resample = Carry::new(resampler, 2 * input.len());
    run("resample", timing.block, timing, || {
        resample.process(input);
        keep(&resample.output);
    });
    ratio("resample", timing.block, || resample.process(input));
}

fn ddc_taps() -> (Vec<f32>, Vec<f32>) {
    let passband = 0.4 * DDC_OUTPUT_RATE;
    let protect = 0.5 * DDC_OUTPUT_RATE;
    let stage_rate = DDC_INPUT_RATE / DDC_DECIMATION as f64;
    let first = firdes::kaiser::lowpass::<f32>(
        passband / DDC_INPUT_RATE,
        (stage_rate - protect - passband) / DDC_INPUT_RATE,
        DDC_RIPPLE,
    );
    let bank_rate = stage_rate * DDC_INTERP as f64;
    let mut second: Vec<f32> = firdes::kaiser::lowpass::<f32>(
        passband / bank_rate,
        (DDC_OUTPUT_RATE - protect - passband) / bank_rate,
        DDC_RIPPLE,
    )
    .iter()
    .map(|tap| tap * DDC_INTERP as f32)
    .collect();
    second.resize(second.len().next_multiple_of(DDC_INTERP), 0.0);
    (first, second)
}

fn tuning(timing: &Timing, input: &[Complex32]) {
    let increment = (TAU * DDC_OFFSET / DDC_INPUT_RATE) as f32;
    let mut mixed = vec![Complex32::default(); input.len()];
    let mut rotator = Rotator::new(increment);
    run("nco", timing.block, timing, || {
        rotator.rotate(input, &mut mixed);
        keep(&mixed);
    });
    let (first, second) = ddc_taps();
    let mut shift = Rotator::new(-increment);
    let mut decimate = Carry::new(
        DecimatingFirFilter::<Complex32, Complex32, _>::new(DDC_DECIMATION, first),
        input.len(),
    );
    let mut resample = Carry::new(
        PolyphaseResamplingFir::<Complex32, Complex32, _>::new(DDC_INTERP, DDC_DECIM, second),
        input.len(),
    );
    let mut step = || {
        shift.rotate(input, &mut mixed);
        let decimated = decimate.process(&mixed);
        resample.process(&decimate.output[..decimated])
    };
    run("ddc", timing.block, timing, || {
        keep(&step());
    });
    ratio("ddc", timing.block, step);
}

fn main() -> Result<(), String> {
    let timing = Timing::from_env()?;
    let input = signal(timing.block, 0x11D);
    filters(&timing, &input);
    tuning(&timing, &input);
    Ok(())
}
