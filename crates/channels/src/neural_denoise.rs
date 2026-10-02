use std::sync::Arc;

use num_complex::Complex;
use realfft::{ComplexToReal, FftError, RealFftPlanner, RealToComplex};
use sdrmm_dsp::{RealDecimator, RealInterpolator, design_lowpass};
use sdrmm_wire::DenoiseModel;

use crate::{
    AUDIO_RATE,
    neural::{Net, NetError, Session},
};

const TAPS_PER_FACTOR: usize = 32;
const CUTOFF_PER_FACTOR: f64 = 0.45;
const MODEL_DELAY_FRAMES: usize = 4;
const MAX_ATTENUATION_DB: f32 = 30.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("denoiser unavailable: {0}")]
pub struct NeuralDenoiseError(pub String);

impl From<FftError> for NeuralDenoiseError {
    fn from(error: FftError) -> Self {
        Self(error.to_string())
    }
}

impl From<NetError> for NeuralDenoiseError {
    fn from(error: NetError) -> Self {
        Self(error.to_string())
    }
}

struct Shape {
    window_len: usize,
    hop: usize,
    factor: usize,
}

impl Shape {
    fn of(net: &Net) -> Result<Self, NeuralDenoiseError> {
        let window_len: usize = parse(net, "n_fft")?;
        let hop: usize = parse(net, "hop_length")?;
        let rate: u32 = parse(net, "sample_rate")?;
        let factor = (AUDIO_RATE / rate.max(1)) as usize;
        let bins = window_len / 2 + 1;
        let fits = hop > 0
            && hop <= window_len
            && factor >= 1
            && AUDIO_RATE == rate * factor as u32
            && net.input_len(0) == Some(2 * bins)
            && net.output_len(0) == Some(2 * bins)
            && net.input_len(1) == net.output_len(1);
        if !fits {
            return Err(NeuralDenoiseError("model does not fit the denoiser".into()));
        }
        Ok(Self {
            window_len,
            hop,
            factor,
        })
    }
}

fn parse<T: std::str::FromStr>(net: &Net, key: &str) -> Result<T, NeuralDenoiseError> {
    net.meta(key)
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| NeuralDenoiseError(format!("model metadata lacks {key}")))
}

fn floats(net: &Net, key: &str) -> Result<Vec<f32>, NeuralDenoiseError> {
    net.meta(key)
        .ok_or_else(|| NeuralDenoiseError(format!("model metadata lacks {key}")))?
        .split(',')
        .map(|value| {
            value
                .trim()
                .parse()
                .map_err(|_| NeuralDenoiseError(format!("model metadata {key} is not numeric")))
        })
        .collect()
}

pub trait DenoiseNets: Send + Sync {
    fn net(&self, model: DenoiseModel) -> Result<Arc<Net>, NeuralDenoiseError>;
}

#[cfg(any(test, feature = "fixtures"))]
pub struct FixtureNets;

#[cfg(any(test, feature = "fixtures"))]
impl DenoiseNets for FixtureNets {
    fn net(&self, model: DenoiseModel) -> Result<Arc<Net>, NeuralDenoiseError> {
        match model {
            DenoiseModel::Dpdfnet2 => Ok(fixture_net()?),
            other => Err(NeuralDenoiseError(format!(
                "{} is not downloaded",
                other.name()
            ))),
        }
    }
}

#[cfg(any(test, feature = "fixtures"))]
pub fn fixture_net() -> Result<Arc<Net>, NetError> {
    Net::load(include_bytes!("../models/dpdfnet2.sdrmmnn")).map(Arc::new)
}

pub fn initial_state(net: &Net) -> Result<Vec<f32>, NeuralDenoiseError> {
    let size = net
        .input_len(1)
        .ok_or_else(|| NeuralDenoiseError("model takes no state".into()))?;
    let erb_norm = floats(net, "erb_norm_init")?;
    let spec_norm = floats(net, "spec_norm_init")?;
    if erb_norm.len() + spec_norm.len() > size {
        return Err(NeuralDenoiseError("model state is too small".into()));
    }
    let mut state = vec![0.0; size];
    state[..erb_norm.len()].copy_from_slice(&erb_norm);
    state[erb_norm.len()..erb_norm.len() + spec_norm.len()].copy_from_slice(&spec_norm);
    Ok(state)
}

pub struct NeuralDenoiser {
    session: Session,
    initial_state: Vec<f32>,
    window_len: usize,
    hop: usize,
    factor: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    decimator: RealDecimator,
    interpolator: RealInterpolator,
    narrow: Vec<f32>,
    wide: Vec<f32>,
    frame_in: Vec<f32>,
    frame_out: Vec<f32>,
    overlap: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    noisy: Vec<Vec<Complex<f32>>>,
    noisy_head: usize,
    ready: std::collections::VecDeque<f32>,
    dry_mix: f32,
}

impl NeuralDenoiser {
    pub fn new(net: Arc<Net>, strength: f32) -> Result<Self, NeuralDenoiseError> {
        let Shape {
            window_len,
            hop,
            factor,
        } = Shape::of(&net)?;
        let initial_state = initial_state(&net)?;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(window_len);
        let ifft = planner.plan_fft_inverse(window_len);
        let scratch_len = fft.get_scratch_len().max(ifft.get_scratch_len());
        let bins = window_len / 2 + 1;
        let taps = design_lowpass(TAPS_PER_FACTOR * factor, CUTOFF_PER_FACTOR / factor as f64);
        let latency = window_len * factor;
        let mut denoiser = Self {
            session: Session::new(net),
            initial_state,
            window_len,
            hop,
            factor,
            fft,
            ifft,
            window: vorbis(window_len),
            decimator: RealDecimator::new(&taps, factor),
            interpolator: RealInterpolator::new(&taps, factor),
            narrow: Vec::with_capacity(window_len * 4),
            wide: Vec::with_capacity(hop * factor),
            frame_in: Vec::with_capacity(window_len * 4),
            frame_out: vec![0.0; hop],
            overlap: vec![0.0; window_len],
            frame: vec![0.0; window_len],
            spectrum: vec![Complex::new(0.0, 0.0); bins],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            noisy: vec![vec![Complex::new(0.0, 0.0); bins]; MODEL_DELAY_FRAMES + 1],
            noisy_head: 0,
            ready: std::collections::VecDeque::with_capacity(latency * 4),
            dry_mix: 0.0,
        };
        denoiser.set_strength(strength);
        denoiser.reset();
        Ok(denoiser)
    }

    #[must_use]
    pub fn net(&self) -> &Arc<Net> {
        self.session.net()
    }

    pub fn set_strength(&mut self, strength: f32) {
        let strength = strength.clamp(0.0, 1.0);
        self.dry_mix = 10f32.powf(-strength * MAX_ATTENUATION_DB / 20.0);
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        self.window_len * self.factor
    }

    pub fn reset(&mut self) {
        self.session
            .input_mut(1)
            .copy_from_slice(&self.initial_state);
        self.decimator.reset();
        self.interpolator.reset();
        self.frame_in.clear();
        self.overlap.fill(0.0);
        for frame in &mut self.noisy {
            frame.fill(Complex::new(0.0, 0.0));
        }
        self.ready.clear();
        self.ready.resize(self.latency(), 0.0);
    }

    pub fn process(&mut self, pcm: &mut [f32]) -> Result<(), NeuralDenoiseError> {
        if self.factor == 1 {
            self.frame_in.extend_from_slice(pcm);
        } else {
            self.decimator.process(pcm, &mut self.narrow);
            self.frame_in.extend_from_slice(&self.narrow);
        }
        let mut consumed = 0;
        while self.frame_in.len() - consumed >= self.window_len {
            self.run_frame(consumed)?;
            if self.factor == 1 {
                self.ready.extend(&self.frame_out);
            } else {
                self.interpolator.process(&self.frame_out, &mut self.wide);
                self.ready.extend(&self.wide);
            }
            consumed += self.hop;
        }
        self.frame_in.drain(..consumed);
        for sample in pcm.iter_mut() {
            *sample = self.ready.pop_front().unwrap_or(0.0);
        }
        Ok(())
    }

    fn run_frame(&mut self, start: usize) -> Result<(), NeuralDenoiseError> {
        let len = self.window_len;
        let bins = self.spectrum.len();
        for ((slot, &x), &w) in self
            .frame
            .iter_mut()
            .zip(&self.frame_in[start..start + len])
            .zip(&self.window)
        {
            *slot = x * w;
        }
        self.fft
            .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)?;
        self.noisy_head = (self.noisy_head + 1) % self.noisy.len();
        self.noisy[self.noisy_head].copy_from_slice(&self.spectrum);
        self.infer();
        let delayed = &self.noisy[(self.noisy_head + 1) % self.noisy.len()];
        let wet = 1.0 - self.dry_mix;
        let enhanced = self.session.output(0);
        for (bin, slot) in self.spectrum.iter_mut().enumerate() {
            let model = Complex::new(enhanced[2 * bin], enhanced[2 * bin + 1]);
            *slot = delayed[bin] * self.dry_mix + model * wet;
        }
        self.spectrum[0].im = 0.0;
        if len.is_multiple_of(2) {
            self.spectrum[bins - 1].im = 0.0;
        }
        self.ifft
            .process_with_scratch(&mut self.spectrum, &mut self.frame, &mut self.scratch)?;
        let scale = 1.0 / len as f32;
        for ((acc, &value), &w) in self.overlap.iter_mut().zip(&self.frame).zip(&self.window) {
            *acc += value * scale * w;
        }
        let hop = self.hop;
        self.frame_out.copy_from_slice(&self.overlap[..hop]);
        self.overlap.copy_within(hop.., 0);
        self.overlap[len - hop..].fill(0.0);
        Ok(())
    }

    fn infer(&mut self) {
        for (pair, value) in self
            .session
            .input_mut(0)
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(&self.spectrum)
        {
            pair[0] = value.re;
            pair[1] = value.im;
        }
        self.session.run();
        self.session.carry(1, 1);
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
        let mut denoiser =
            NeuralDenoiser::new(fixture_net().expect("model loads"), 1.0).expect("denoiser builds");
        for len in [1usize, 159, 480, 997, 4_800] {
            let mut block = noise(len, 0.1, 7);
            denoiser.process(&mut block).expect("inference runs");
            assert_eq!(block.len(), len);
        }
    }

    #[test]
    fn it_quietens_noise_with_nobody_talking() {
        let input = noise(96_000, 0.1, 11);
        let output = run(
            &mut NeuralDenoiser::new(fixture_net().expect("model loads"), 1.0)
                .expect("denoiser builds"),
            &input,
        );
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
        let output = run(
            &mut NeuralDenoiser::new(fixture_net().expect("model loads"), 1.0)
                .expect("denoiser builds"),
            &noisy,
        );
        let kept = rms(&output[48_000..]);
        let voiced = rms(&voice[48_000..]);
        assert!(kept > voiced * 0.3, "voice fell from {voiced} to {kept}");
    }

    #[test]
    fn zero_strength_hands_back_the_input_delayed() {
        let input = vowel(48_000);
        let mut denoiser =
            NeuralDenoiser::new(fixture_net().expect("model loads"), 0.0).expect("denoiser builds");
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
