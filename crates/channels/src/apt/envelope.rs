use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FmDemod, Highpass, RealDecimator, design_lowpass};

use super::{
    geometry::{DEVIATION_HZ, SUBCARRIER_HZ},
    track::Track,
};

pub(crate) const DECIMATION: usize = 5;
pub(crate) const CHUNK: usize = 6_000;
const AUDIO_TAPS: usize = 121;
const AUDIO_CUTOFF: f64 = 0.5 / DECIMATION as f64;
const DC_CORNER_HZ: f64 = 10.0;
const ENVELOPE_TAPS: usize = 127;
const ENVELOPE_CUTOFF_HZ: f64 = 2_040.0;
const MIXER_STEPS: usize = 5;

pub(crate) struct Envelope {
    input_rate: f64,
    fm: FmDemod,
    audio: RealDecimator,
    dc: Highpass,
    lowpass: Decimator,
    mixer: [Complex<f32>; MIXER_STEPS],
    step: usize,
    freq: Vec<f32>,
    decimated: Vec<f32>,
    mixed: Vec<Complex<f32>>,
    baseband: Vec<Complex<f32>>,
}

pub(crate) fn track_rate(input_rate: f64) -> f64 {
    input_rate / DECIMATION as f64
}

impl Envelope {
    pub(crate) fn new(input_rate: f64) -> Self {
        let rate = track_rate(input_rate);
        let mixer = std::array::from_fn(|k| {
            Complex::from_polar(1.0, -(TAU * SUBCARRIER_HZ * k as f64 / rate) as f32)
        });
        let decimated = CHUNK / DECIMATION + DECIMATION;
        Self {
            input_rate,
            fm: FmDemod::new(input_rate, DEVIATION_HZ),
            audio: RealDecimator::new(&design_lowpass(AUDIO_TAPS, AUDIO_CUTOFF), DECIMATION),
            dc: Highpass::new(rate, DC_CORNER_HZ),
            lowpass: Decimator::new(&design_lowpass(ENVELOPE_TAPS, ENVELOPE_CUTOFF_HZ / rate), 1),
            mixer,
            step: 0,
            freq: Vec::with_capacity(CHUNK),
            decimated: Vec::with_capacity(decimated),
            mixed: Vec::with_capacity(decimated),
            baseband: Vec::with_capacity(decimated),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.fm = FmDemod::new(self.input_rate, DEVIATION_HZ);
        self.audio.reset();
        self.dc.reset();
        self.lowpass.reset();
        self.step = 0;
    }

    pub(crate) fn process(&mut self, iq: &[Complex<f32>], track: &mut Track) {
        self.fm.process(iq, &mut self.freq);
        self.audio.process(&self.freq, &mut self.decimated);
        self.dc.process(&mut self.decimated);
        self.mixed.clear();
        for &sample in &self.decimated {
            self.mixed.push(self.mixer[self.step] * sample);
            self.step = (self.step + 1) % MIXER_STEPS;
        }
        self.lowpass.process(&self.mixed, &mut self.baseband);
        for sample in &self.baseband {
            track.push(2.0 * sample.norm());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    const RATE: f64 = 60_000.0;

    #[test]
    fn the_mixer_closes_one_subcarrier_cycle() {
        let cycles = SUBCARRIER_HZ * MIXER_STEPS as f64 / track_rate(RATE);
        assert!((cycles - cycles.round()).abs() < 1e-9, "{cycles} cycles");
    }

    #[test]
    fn a_steady_subcarrier_reads_its_amplitude_through_a_carrier_offset() {
        let audio = synth::tone_audio(SUBCARRIER_HZ, 0.6, RATE, 60_000);
        let mut iq = synth::fm_modulate(&audio, DEVIATION_HZ, RATE);
        synth::shift(&mut iq, 3_000.0, RATE);
        let mut envelope = Envelope::new(RATE);
        let mut track = Track::new();
        for chunk in iq.chunks(CHUNK) {
            envelope.process(chunk, &mut track);
        }
        let level = track.get(track.head() - 100);
        assert!((level - 0.6).abs() < 0.01, "read {level}");
    }
}
