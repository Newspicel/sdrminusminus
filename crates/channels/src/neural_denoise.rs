use std::sync::{Arc, OnceLock};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use sdrmm_dsp::{RealDecimator, RealInterpolator, design_lowpass};
use tract_onnx::{pb::ModelProto, prelude::*};

use crate::AUDIO_RATE;

const MODEL: &[u8] = include_bytes!("../models/dpdfnet2.onnx");
const MODEL_RATE: u32 = 16_000;
const RATE_FACTOR: usize = (AUDIO_RATE / MODEL_RATE) as usize;
const RESAMPLE_TAPS: usize = 96;
const RESAMPLE_CUTOFF: f64 = 0.15;
const MODEL_DELAY_FRAMES: usize = 4;
const MAX_ATTENUATION_DB: f32 = 30.0;

#[derive(Debug, Clone, thiserror::Error)]
#[error("neural denoiser unavailable: {0}")]
pub struct NeuralDenoiseError(String);

struct Model {
    plan: Arc<TypedSimplePlan>,
    initial_state: Vec<f32>,
    window_len: usize,
    hop: usize,
}

fn model() -> Result<&'static Model, NeuralDenoiseError> {
    static MODEL_CELL: OnceLock<Result<Model, NeuralDenoiseError>> = OnceLock::new();
    MODEL_CELL
        .get_or_init(|| load().map_err(|error| NeuralDenoiseError(format!("{error:#}"))))
        .as_ref()
        .map_err(Clone::clone)
}

fn load() -> TractResult<Model> {
    let onnx = tract_onnx::onnx();
    let proto = onnx.proto_model_for_read(&mut &MODEL[..])?;
    let meta = |key: &str| metadata(&proto, key);
    let window_len: usize = meta("window_length")?.parse()?;
    let hop: usize = meta("hop_length")?.parse()?;
    let initial_state = initial_state(
        meta("state_size")?.parse()?,
        &floats(&meta("erb_norm_init")?)?,
        &floats(&meta("spec_norm_init")?)?,
    );
    let plan = onnx
        .model_for_proto_model(&proto)?
        .into_optimized()?
        .into_runnable()?;
    Ok(Model {
        plan,
        initial_state,
        window_len,
        hop,
    })
}

fn metadata(proto: &ModelProto, key: &str) -> TractResult<String> {
    proto
        .metadata_props
        .iter()
        .find(|prop| prop.key == key)
        .map(|prop| prop.value.clone())
        .ok_or_else(|| TractError::msg(format!("model metadata lacks {key}")))
}

fn floats(list: &str) -> TractResult<Vec<f32>> {
    list.split(',')
        .map(|value| Ok(value.trim().parse()?))
        .collect()
}

fn initial_state(size: usize, erb_norm: &[f32], spec_norm: &[f32]) -> Vec<f32> {
    let mut state = vec![0.0; size];
    state[..erb_norm.len()].copy_from_slice(erb_norm);
    state[erb_norm.len()..erb_norm.len() + spec_norm.len()].copy_from_slice(spec_norm);
    state
}

pub struct NeuralDenoiser {
    model: &'static Model,
    runner: TypedSimpleState,
    state: Tensor,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    decimator: RealDecimator,
    interpolator: RealInterpolator,
    narrow: Vec<f32>,
    wide: Vec<f32>,
    frame_in: Vec<f32>,
    frame_out: Vec<f32>,
    overlap: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    noisy: Vec<Vec<Complex<f32>>>,
    noisy_head: usize,
    packed: Vec<f32>,
    ready: std::collections::VecDeque<f32>,
    dry_mix: f32,
}

impl NeuralDenoiser {
    pub fn new(strength: f32) -> Result<Self, NeuralDenoiseError> {
        let model = model()?;
        let runner = model
            .plan
            .spawn()
            .map_err(|error| NeuralDenoiseError(format!("{error:#}")))?;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(model.window_len);
        let ifft = planner.plan_fft_inverse(model.window_len);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        let bins = model.window_len / 2 + 1;
        let taps = design_lowpass(RESAMPLE_TAPS, RESAMPLE_CUTOFF);
        let mut denoiser = Self {
            model,
            runner,
            state: Tensor::from_shape(&[model.initial_state.len()], &model.initial_state)
                .map_err(|error| NeuralDenoiseError(format!("{error:#}")))?,
            fft,
            ifft,
            window: vorbis(model.window_len),
            decimator: RealDecimator::new(&taps, RATE_FACTOR),
            interpolator: RealInterpolator::new(&taps, RATE_FACTOR),
            narrow: Vec::new(),
            wide: Vec::new(),
            frame_in: Vec::with_capacity(model.window_len * 4),
            frame_out: vec![0.0; model.hop],
            overlap: vec![0.0; model.window_len],
            spectrum: vec![Complex::new(0.0, 0.0); model.window_len],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            noisy: vec![vec![Complex::new(0.0, 0.0); bins]; MODEL_DELAY_FRAMES + 1],
            noisy_head: 0,
            packed: vec![0.0; 2 * bins],
            ready: std::collections::VecDeque::new(),
            dry_mix: 0.0,
        };
        denoiser.set_strength(strength);
        denoiser.prime();
        Ok(denoiser)
    }

    pub fn set_strength(&mut self, strength: f32) {
        let strength = strength.clamp(0.0, 1.0);
        self.dry_mix = 10f32.powf(-strength * MAX_ATTENUATION_DB / 20.0);
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        self.model.window_len * RATE_FACTOR
    }

    pub fn reset(&mut self) -> Result<(), NeuralDenoiseError> {
        self.state =
            Tensor::from_shape(&[self.model.initial_state.len()], &self.model.initial_state)
                .map_err(|error| NeuralDenoiseError(format!("{error:#}")))?;
        self.decimator.reset();
        self.interpolator.reset();
        self.frame_in.clear();
        self.overlap.fill(0.0);
        for frame in &mut self.noisy {
            frame.fill(Complex::new(0.0, 0.0));
        }
        self.prime();
        Ok(())
    }

    fn prime(&mut self) {
        self.ready.clear();
        self.ready.resize(self.latency(), 0.0);
    }

    pub fn process(&mut self, pcm: &mut [f32]) -> Result<(), NeuralDenoiseError> {
        self.decimator.process(pcm, &mut self.narrow);
        self.frame_in.extend_from_slice(&self.narrow);
        let mut consumed = 0;
        while self.frame_in.len() - consumed >= self.model.window_len {
            self.run_frame(consumed)?;
            self.interpolator.process(&self.frame_out, &mut self.wide);
            self.ready.extend(&self.wide);
            consumed += self.model.hop;
        }
        self.frame_in.drain(..consumed);
        for sample in pcm.iter_mut() {
            *sample = self.ready.pop_front().unwrap_or(0.0);
        }
        Ok(())
    }

    fn run_frame(&mut self, start: usize) -> Result<(), NeuralDenoiseError> {
        let len = self.model.window_len;
        let bins = len / 2 + 1;
        for ((slot, &x), &w) in self
            .spectrum
            .iter_mut()
            .zip(&self.frame_in[start..start + len])
            .zip(&self.window)
        {
            *slot = Complex::new(x * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        self.noisy_head = (self.noisy_head + 1) % self.noisy.len();
        self.noisy[self.noisy_head].copy_from_slice(&self.spectrum[..bins]);
        self.infer(bins)?;
        let delayed = &self.noisy[(self.noisy_head + 1) % self.noisy.len()];
        let wet = 1.0 - self.dry_mix;
        for (bin, slot) in self.spectrum[..bins].iter_mut().enumerate() {
            let model = Complex::new(self.packed[2 * bin], self.packed[2 * bin + 1]);
            *slot = delayed[bin] * self.dry_mix + model * wet;
        }
        for bin in 1..len - bins + 1 {
            self.spectrum[len - bin] = self.spectrum[bin].conj();
        }
        self.ifft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let scale = 1.0 / len as f32;
        for ((acc, value), &w) in self
            .overlap
            .iter_mut()
            .zip(&self.spectrum)
            .zip(&self.window)
        {
            *acc += value.re * scale * w;
        }
        let hop = self.model.hop;
        self.frame_out.copy_from_slice(&self.overlap[..hop]);
        self.overlap.copy_within(hop.., 0);
        self.overlap[len - hop..].fill(0.0);
        Ok(())
    }

    fn infer(&mut self, bins: usize) -> Result<(), NeuralDenoiseError> {
        let fail = |error: TractError| NeuralDenoiseError(format!("{error:#}"));
        for (pair, value) in self
            .packed
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(&self.spectrum[..bins])
        {
            pair[0] = value.re;
            pair[1] = value.im;
        }
        let spec = Tensor::from_shape(&[1, 1, bins, 2], &self.packed).map_err(fail)?;
        let state = std::mem::take(&mut self.state);
        let mut outputs = self
            .runner
            .run(tvec!(TValue::from(spec), TValue::from(state)))
            .map_err(fail)?;
        if outputs.len() < 2 {
            return Err(NeuralDenoiseError("model returned too few outputs".into()));
        }
        self.state = outputs.remove(1).into_tensor();
        let enhanced = outputs.remove(0).into_tensor();
        let view = enhanced.to_plain_array_view::<f32>().map_err(fail)?;
        if view.len() != self.packed.len() {
            return Err(NeuralDenoiseError(format!(
                "model returned {} values for {} bins",
                view.len(),
                bins
            )));
        }
        for (slot, &value) in self.packed.iter_mut().zip(view.iter()) {
            *slot = value;
        }
        Ok(())
    }
}

fn vorbis(len: usize) -> Vec<f32> {
    let half = len as f32 / 2.0;
    (0..len)
        .map(|n| {
            let s = (0.5 * std::f32::consts::PI * (n as f32 + 0.5) / half).sin();
            (0.5 * std::f32::consts::PI * s * s).sin()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::rms;

    const RATE: f64 = AUDIO_RATE as f64;

    fn noise(len: usize, amplitude: f32, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                amplitude * ((state >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn vowel(len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                let t = n as f64 / RATE;
                let syllable = (std::f64::consts::PI * t * 3.0).sin().abs();
                let pitch = 120.0 + 20.0 * (std::f64::consts::TAU * 0.7 * t).sin();
                let voice: f64 = (1..=25)
                    .map(|k| {
                        let f = pitch * f64::from(k);
                        let formant = (-((f - 700.0) / 300.0).powi(2)).exp()
                            + 0.6 * (-((f - 1_200.0) / 400.0).powi(2)).exp()
                            + 0.3 * (-((f - 2_500.0) / 500.0).powi(2)).exp();
                        formant * (std::f64::consts::TAU * f * t).sin()
                    })
                    .sum();
                (0.3 * syllable * voice) as f32
            })
            .collect()
    }

    fn run(denoiser: &mut NeuralDenoiser, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for block in input.chunks(997) {
            let mut block = block.to_vec();
            denoiser.process(&mut block).expect("inference runs");
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn it_returns_one_sample_for_every_sample_it_is_given() {
        let mut denoiser = NeuralDenoiser::new(1.0).expect("model loads");
        for len in [1usize, 159, 480, 997, 4_800] {
            let mut block = noise(len, 0.1, 7);
            denoiser.process(&mut block).expect("inference runs");
            assert_eq!(block.len(), len);
        }
    }

    #[test]
    fn it_quietens_noise_with_nobody_talking() {
        let input = noise(96_000, 0.1, 11);
        let output = run(&mut NeuralDenoiser::new(1.0).expect("model loads"), &input);
        let before = rms(&input[48_000..]);
        let after = rms(&output[48_000..]);
        assert!(
            after < before * 0.1,
            "noise only fell from {before} to {after}"
        );
    }

    #[test]
    fn it_keeps_a_voice_while_it_takes_the_noise() {
        let voice = vowel(144_000);
        let hiss = noise(voice.len(), 0.05, 3);
        let noisy: Vec<f32> = voice.iter().zip(&hiss).map(|(v, n)| v + n).collect();
        let output = run(&mut NeuralDenoiser::new(1.0).expect("model loads"), &noisy);
        let kept = rms(&output[48_000..]);
        let voiced = rms(&voice[48_000..]);
        assert!(kept > voiced * 0.3, "voice fell from {voiced} to {kept}");
    }

    #[test]
    fn zero_strength_hands_back_the_input_delayed() {
        let input = vowel(48_000);
        let mut denoiser = NeuralDenoiser::new(0.0).expect("model loads");
        let output = run(&mut denoiser, &input);
        let settled = 24_000;
        let best = (0..6_000)
            .map(|lag| {
                let err: f32 = output[settled..]
                    .iter()
                    .zip(&input[settled - lag..])
                    .map(|(a, b)| (a - b).powi(2))
                    .sum();
                (err / (output.len() - settled) as f32).sqrt()
            })
            .fold(f32::MAX, f32::min);
        assert!(best < 0.05 * rms(&input), "residual {best}");
    }
}
