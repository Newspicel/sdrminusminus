//! The processing every voice channel carries between its demodulator and the listener, plus
//! the one stage that has to run before the demodulator.
//!
//! It lives here, once, rather than inside each mode: a blanker, a notch and an AGC are the same
//! things whether the carrier was FM, AM or a sideband, and a mode that grew its own copy would
//! be a second set of constants to get wrong.

use num_complex::Complex;
use sdrmm_dsp::{Agc, AutoNotch, Biquad, NoiseBlanker, SpectralDenoiser};
use sdrmm_wire::{AudioProcessing, NotchSettings};

use crate::AUDIO_RATE;

/// What the AGC levels to, in RMS. Well below full scale so a peaky talker has headroom.
const AGC_TARGET_RMS: f32 = 0.25;
/// Ceiling on AGC gain: 40 dB is enough to lift a weak signal without turning a silent channel
/// into a noise generator.
const AGC_MAX_GAIN: f32 = 100.0;
/// Butterworth pole `q`s for a cascaded pair, which is the 24 dB/octave skirt an operator
/// expects when they drag a passband edge onto a neighbouring station.
const BUTTERWORTH_Q: [f64; 2] = [0.541_196_1, 1.306_562_9];

/// One channel's audio chain, sized to the interleave the mode produces.
pub struct AudioChain {
    settings: AudioProcessing,
    iq_rate: f64,
    blanker: Option<NoiseBlanker>,
    /// One independent set of filters per interleaved audio channel: stereo WFM's two sides are
    /// two signals, and sharing a filter's state between them would fold one into the other.
    planes: Vec<Plane>,
    deinterleaved: Vec<Vec<f32>>,
}

impl AudioChain {
    #[must_use]
    pub fn new(iq_rate: f64, audio_channels: u8, settings: &AudioProcessing) -> Self {
        let mut chain = Self {
            settings: AudioProcessing::default(),
            iq_rate,
            blanker: None,
            planes: Vec::new(),
            deinterleaved: Vec::new(),
        };
        chain.configure(audio_channels, settings);
        chain
    }

    /// Rebuild whatever the new settings changed, keeping the state of the stages they did not.
    /// An operator dragging a noise-reduction slider must not restart the blanker's average.
    ///
    /// Runs where the host applies settings, which is the DSP thread, and switching a stage on
    /// there allocates — the same bounded deviation the channel filter and the demodulators
    /// beside it already make on a settings change. Steady-state processing allocates nothing.
    pub fn configure(&mut self, audio_channels: u8, settings: &AudioProcessing) {
        let planes = usize::from(audio_channels).max(1);
        if settings.blanker.enabled {
            match &mut self.blanker {
                Some(blanker) => blanker.set_threshold(settings.blanker.threshold),
                none => *none = Some(NoiseBlanker::new(self.iq_rate, settings.blanker.threshold)),
            }
        } else {
            self.blanker = None;
        }
        let rebuild = planes != self.planes.len();
        if rebuild {
            self.planes.clear();
            self.planes.resize_with(planes, Plane::default);
        }
        let agc_changed = rebuild || settings.agc != self.settings.agc;
        for plane in &mut self.planes {
            plane.configure(settings, agc_changed);
        }
        self.deinterleaved.resize_with(planes, Vec::new);
        self.settings = settings.clone();
    }

    /// Forget everything accreted from the signal the channel just left.
    pub fn reset(&mut self) {
        if let Some(blanker) = &mut self.blanker {
            blanker.reset();
        }
        for plane in &mut self.planes {
            plane.reset();
        }
    }

    /// The impulse blanker, on the channel's IQ ahead of its selectivity filter.
    pub fn process_iq(&mut self, iq: &mut [Complex<f32>]) {
        if let Some(blanker) = &mut self.blanker {
            blanker.process(iq);
        }
    }

    /// Everything after the demodulator. `pcm` is interleaved at the channel count this chain
    /// was configured with, and comes back the same length.
    pub fn process_audio(&mut self, pcm: &mut [f32]) {
        if self.planes.is_empty() || pcm.is_empty() {
            return;
        }
        if self.planes.len() == 1 {
            self.planes[0].process(pcm);
            return;
        }
        let planes = self.planes.len();
        for (index, buffer) in self.deinterleaved.iter_mut().enumerate() {
            buffer.clear();
            buffer.extend(pcm.iter().skip(index).step_by(planes));
        }
        for (plane, buffer) in self.planes.iter_mut().zip(&mut self.deinterleaved) {
            plane.process(buffer);
        }
        for (index, buffer) in self.deinterleaved.iter().enumerate() {
            for (slot, &value) in pcm.iter_mut().skip(index).step_by(planes).zip(buffer) {
                *slot = value;
            }
        }
    }
}

/// The audio stages for one interleaved channel, in the order they run.
#[derive(Default)]
struct Plane {
    highpass: Vec<Biquad>,
    lowpass: Vec<Biquad>,
    notches: Vec<Biquad>,
    auto_notch: Option<AutoNotch>,
    denoise: Option<SpectralDenoiser>,
    agc: Option<Agc>,
}

impl Plane {
    fn configure(&mut self, settings: &AudioProcessing, agc_changed: bool) {
        let rate = f64::from(AUDIO_RATE);
        if settings.filter.enabled {
            self.highpass = butterworth(rate, settings.filter.low_hz, true);
            self.lowpass = butterworth(rate, settings.filter.high_hz, false);
        } else {
            self.highpass.clear();
            self.lowpass.clear();
        }
        self.notches = settings.notches.iter().map(|n| notch(rate, n)).collect();

        if settings.auto_notch {
            self.auto_notch.get_or_insert_with(AutoNotch::new);
        } else {
            self.auto_notch = None;
        }

        match (&mut self.denoise, settings.denoise.enabled) {
            (Some(denoise), true) => denoise.set_strength(settings.denoise.strength),
            (slot @ None, true) => *slot = Some(SpectralDenoiser::new(settings.denoise.strength)),
            (slot, false) => *slot = None,
        }

        // The AGC is rebuilt only when its speed changes: its gain is state, and restarting it
        // on an unrelated edit would be an audible jump on every slider move.
        match settings.agc.time_constants_s() {
            Some((attack_s, release_s)) if agc_changed || self.agc.is_none() => {
                self.agc = Some(Agc::new(
                    f64::from(AUDIO_RATE),
                    AGC_TARGET_RMS,
                    attack_s,
                    release_s,
                    AGC_MAX_GAIN,
                ));
            }
            Some(_) => {}
            None => self.agc = None,
        }
    }

    fn reset(&mut self) {
        for section in self.highpass.iter_mut().chain(&mut self.lowpass) {
            section.reset();
        }
        for section in &mut self.notches {
            section.reset();
        }
        if let Some(auto_notch) = &mut self.auto_notch {
            auto_notch.reset();
        }
        if let Some(denoise) = &mut self.denoise {
            denoise.reset();
        }
    }

    fn process(&mut self, pcm: &mut [f32]) {
        for section in self.highpass.iter_mut().chain(&mut self.lowpass) {
            section.process(pcm);
        }
        for section in &mut self.notches {
            section.process(pcm);
        }
        if let Some(auto_notch) = &mut self.auto_notch {
            auto_notch.process(pcm);
        }
        if let Some(denoise) = &mut self.denoise {
            denoise.process(pcm);
        }
        if let Some(agc) = &mut self.agc {
            agc.process(pcm);
        }
    }
}

fn butterworth(rate: f64, freq_hz: f64, high: bool) -> Vec<Biquad> {
    BUTTERWORTH_Q
        .iter()
        .map(|&q| {
            if high {
                Biquad::highpass(rate, freq_hz, q)
            } else {
                Biquad::lowpass(rate, freq_hz, q)
            }
        })
        .collect()
}

fn notch(rate: f64, settings: &NotchSettings) -> Biquad {
    let width = settings.width_hz.max(f64::MIN_POSITIVE);
    Biquad::notch(rate, settings.freq_hz, settings.freq_hz / width)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        AudioAgcMode, AudioFilterSettings, DenoiseSettings, NoiseBlankerSettings,
        NotchSettings as Notch,
    };

    use super::*;
    use crate::testutil::{complex_tone, rms, tone_amplitude};

    const IQ_RATE: f64 = 48_000.0;
    const RATE: f64 = AUDIO_RATE as f64;

    fn chain(settings: AudioProcessing) -> AudioChain {
        AudioChain::new(IQ_RATE, 1, &settings)
    }

    fn tone(freq_hz: f64, amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| amplitude * (std::f64::consts::TAU * freq_hz * n as f64 / RATE).sin() as f32)
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// Feed the chain in ragged blocks, as the engine does, and return everything it produced.
    fn run(chain: &mut AudioChain, pcm: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(pcm.len());
        let mut pos = 0;
        for len in [997usize, 4_096, 65, 2_048].iter().cycle() {
            if pos >= pcm.len() {
                break;
            }
            let end = (pos + len).min(pcm.len());
            let mut block = pcm[pos..end].to_vec();
            chain.process_audio(&mut block);
            out.extend_from_slice(&block);
            pos = end;
        }
        out
    }

    #[test]
    fn a_default_chain_passes_audio_through_untouched() {
        let input = tone(1_000.0, 0.3, 24_000);
        let output = run(&mut chain(AudioProcessing::default()), &input);
        assert_eq!(output, input);
    }

    /// Length in equals length out for every stage, in ragged blocks — the engine stamps PCM
    /// by counting samples, so a stage that swallowed or invented one would shift the stream.
    #[test]
    fn every_stage_returns_exactly_what_it_was_given() {
        let settings = AudioProcessing {
            blanker: NoiseBlankerSettings {
                enabled: true,
                threshold: 4.0,
            },
            filter: AudioFilterSettings {
                enabled: true,
                low_hz: 300.0,
                high_hz: 3_000.0,
            },
            notches: vec![Notch {
                freq_hz: 1_500.0,
                width_hz: 80.0,
            }],
            auto_notch: true,
            denoise: DenoiseSettings {
                enabled: true,
                strength: 0.8,
            },
            agc: AudioAgcMode::Medium,
        };
        let input = tone(1_000.0, 0.3, 48_000);
        assert_eq!(run(&mut chain(settings), &input).len(), input.len());
    }

    #[test]
    fn the_passband_keeps_the_voice_band_and_drops_what_is_outside_it() {
        let settings = AudioProcessing {
            filter: AudioFilterSettings {
                enabled: true,
                low_hz: 300.0,
                high_hz: 3_000.0,
            },
            ..AudioProcessing::default()
        };
        for (freq_hz, kept) in [(100.0, false), (1_000.0, true), (6_000.0, false)] {
            let output = run(&mut chain(settings.clone()), &tone(freq_hz, 0.5, 48_000));
            let level = rms(&output[24_000..]);
            if kept {
                assert!(level > 0.3, "{freq_hz} Hz was cut: {level}");
            } else {
                assert!(level < 0.02, "{freq_hz} Hz survived: {level}");
            }
        }
    }

    #[test]
    fn a_notch_removes_its_own_tone_and_leaves_the_voice_beside_it() {
        let settings = AudioProcessing {
            notches: vec![Notch {
                freq_hz: 1_500.0,
                width_hz: 60.0,
            }],
            ..AudioProcessing::default()
        };
        let input = mix(&tone(1_500.0, 0.4, 48_000), &tone(800.0, 0.4, 48_000));
        let output = run(&mut chain(settings), &input);
        let settled = &output[24_000..];
        assert!(tone_amplitude(settled, 1_500.0, RATE) < 0.02);
        assert!(tone_amplitude(settled, 800.0, RATE) > 0.35);
    }

    #[test]
    fn the_auto_notch_finds_a_carrier_nobody_pointed_at_it() {
        let settings = AudioProcessing {
            auto_notch: true,
            ..AudioProcessing::default()
        };
        let input = tone(1_200.0, 0.5, 96_000);
        let output = run(&mut chain(settings), &input);
        assert!(tone_amplitude(&output[48_000..], 1_200.0, RATE) < 0.1);
    }

    #[test]
    fn the_agc_lifts_a_quiet_signal_to_its_target() {
        let settings = AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        };
        let output = run(&mut chain(settings), &tone(1_000.0, 0.01, 96_000));
        let level = rms(&output[72_000..]);
        assert!((0.2..0.3).contains(&level), "levelled to {level}");
    }

    /// The blanker is the one stage on the IQ side, and it must leave a clean channel alone.
    #[test]
    fn the_blanker_cuts_impulses_off_the_iq_only_when_it_is_asked_to() {
        let mut spiked: Vec<Complex<f32>> = complex_tone(1_000.0 / IQ_RATE, 24_000)
            .iter()
            .map(|s| s * 0.1)
            .collect();
        for n in (500..spiked.len()).step_by(500) {
            spiked[n] = Complex::new(4.0, 0.0);
        }

        let mut off = chain(AudioProcessing::default());
        let mut untouched = spiked.clone();
        off.process_iq(&mut untouched);
        assert_eq!(untouched, spiked);

        let mut on = chain(AudioProcessing {
            blanker: NoiseBlankerSettings {
                enabled: true,
                threshold: 4.0,
            },
            ..AudioProcessing::default()
        });
        let mut blanked = spiked.clone();
        on.process_iq(&mut blanked);
        let peak = blanked[4_000..]
            .iter()
            .map(|s| s.norm())
            .fold(0.0f32, f32::max);
        assert!(peak < 0.2, "impulse survived at {peak}");
    }

    /// Stereo is two signals sharing one buffer: each side must be filtered by its own state,
    /// or one channel's history leaks into the other's audio.
    #[test]
    fn stereo_planes_are_filtered_independently() {
        let settings = AudioProcessing {
            notches: vec![Notch {
                freq_hz: 1_500.0,
                width_hz: 60.0,
            }],
            ..AudioProcessing::default()
        };
        let left = tone(1_500.0, 0.5, 48_000);
        let right = tone(800.0, 0.5, 48_000);
        let mut interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        AudioChain::new(IQ_RATE, 2, &settings).process_audio(&mut interleaved);

        let taken: Vec<f32> = interleaved
            .iter()
            .skip(2 * 24_000)
            .step_by(2)
            .copied()
            .collect();
        let kept: Vec<f32> = interleaved
            .iter()
            .skip(2 * 24_000 + 1)
            .step_by(2)
            .copied()
            .collect();
        assert!(
            tone_amplitude(&taken, 1_500.0, RATE) < 0.03,
            "notch missed its side"
        );
        assert!(
            tone_amplitude(&kept, 800.0, RATE) > 0.4,
            "the other side was cut"
        );
    }

    /// Reconfiguring is not restarting: an operator nudging one control must not reset the
    /// gain the AGC has already found.
    #[test]
    fn an_unrelated_edit_keeps_the_agc_where_it_was() {
        let mut chain = chain(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        });
        let quiet = tone(1_000.0, 0.01, 96_000);
        run(&mut chain, &quiet);
        chain.configure(
            1,
            &AudioProcessing {
                agc: AudioAgcMode::Fast,
                auto_notch: false,
                notches: vec![Notch {
                    freq_hz: 2_000.0,
                    width_hz: 100.0,
                }],
                ..AudioProcessing::default()
            },
        );
        let mut block = tone(1_000.0, 0.01, 1_200);
        chain.process_audio(&mut block);
        let level = rms(&block);
        assert!((0.15..0.35).contains(&level), "agc restarted: {level}");
    }

    #[test]
    fn turning_a_stage_off_takes_it_out_of_the_path() {
        let mut chain = chain(AudioProcessing {
            filter: AudioFilterSettings {
                enabled: true,
                low_hz: 300.0,
                high_hz: 3_000.0,
            },
            ..AudioProcessing::default()
        });
        run(&mut chain, &tone(100.0, 0.5, 24_000));
        chain.configure(1, &AudioProcessing::default());
        let output = run(&mut chain, &tone(100.0, 0.5, 24_000));
        assert!(rms(&output[12_000..]) > 0.3, "the filter kept filtering");
    }
}
