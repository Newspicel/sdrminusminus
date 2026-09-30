mod frame;
mod params;
mod synth;
mod tables;

use params::{Params, Quantized};
use synth::Noise;

pub(crate) use synth::FRAME_SAMPLES;

const MAX_CLEAN_ERRORS: u32 = 3;
const MAX_REPEATS: u32 = 3;

pub(crate) struct AmbeDecoder {
    current: Params,
    previous: Params,
    enhanced: Params,
    noise: Noise,
}

impl AmbeDecoder {
    pub(crate) fn new() -> Self {
        Self {
            current: Params::default(),
            previous: Params::default(),
            enhanced: Params::default(),
            noise: Noise::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn decode(&mut self, air: &[bool; 72], pcm: &mut [f32; FRAME_SAMPLES]) {
        let frame = frame::decode(air);
        let Some(quantized) = Quantized::read(frame.info) else {
            return self.mute(pcm);
        };
        params::decode(&quantized, &mut self.current, &mut self.previous);
        if frame.errors > MAX_CLEAN_ERRORS {
            self.current.clone_from(&self.previous);
            self.current.repeats += 1;
        } else {
            self.current.repeats = 0;
        }
        if self.current.repeats > MAX_REPEATS {
            return self.mute(pcm);
        }
        self.previous.clone_from(&self.current);
        synth::enhance(&mut self.current);
        synth::synthesize(pcm, &mut self.current, &mut self.enhanced, &mut self.noise);
        self.enhanced.clone_from(&self.current);
    }

    fn mute(&mut self, pcm: &mut [f32; FRAME_SAMPLES]) {
        pcm.fill(0.0);
        self.current = Params::default();
        self.previous = Params::default();
        self.enhanced = Params::default();
    }
}

#[cfg(test)]
mod tests {
    use super::{frame::Info, *};

    fn energy(pcm: &[f32]) -> f32 {
        pcm.iter().map(|sample| sample * sample).sum()
    }

    fn voice_frame(seed: u64) -> [bool; 72] {
        let bits = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 15;
        frame::encode(Info::from_bits(bits & !(0b11_1111 << 43)))
    }

    #[test]
    fn clean_voice_frames_make_sound() {
        let mut decoder = AmbeDecoder::new();
        let mut pcm = [0.0; FRAME_SAMPLES];
        let total: f32 = (1..40)
            .map(|seed| {
                decoder.decode(&voice_frame(seed), &mut pcm);
                energy(&pcm)
            })
            .sum();
        assert!(total > 0.0);
        assert!(pcm.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn a_tone_frame_is_silent() {
        let mut decoder = AmbeDecoder::new();
        let mut pcm = [1.0; FRAME_SAMPLES];
        decoder.decode(&frame::encode(Info::from_bits(0x7F << 42 | 1)), &mut pcm);
        assert_eq!(energy(&pcm), 0.0);
    }

    #[test]
    fn a_long_run_of_bad_frames_goes_silent() {
        let mut decoder = AmbeDecoder::new();
        let mut pcm = [0.0; FRAME_SAMPLES];
        decoder.decode(&voice_frame(7), &mut pcm);
        let damaged = frame::damage(voice_frame(8), &[(0, 23), (0, 22), (1, 22), (1, 21)]);
        for _ in 0..=MAX_REPEATS {
            decoder.decode(&damaged, &mut pcm);
        }
        assert_eq!(decoder.current.repeats, 0);
        assert_eq!(energy(&pcm), 0.0);
    }

    #[test]
    fn phase_tracks_stay_wrapped_through_a_long_over() {
        let mut decoder = AmbeDecoder::new();
        let mut pcm = [0.0; FRAME_SAMPLES];
        let air = voice_frame(3);
        for _ in 0..3_000 {
            decoder.decode(&air, &mut pcm);
        }
        let tau = std::f32::consts::TAU;
        assert!(
            decoder.current.phase_track[1..]
                .iter()
                .all(|phase| (0.0..tau).contains(phase))
        );
        assert!(pcm.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn reset_makes_decoding_repeatable() {
        let mut decoder = AmbeDecoder::new();
        let mut first = [0.0; FRAME_SAMPLES];
        let mut second = [0.0; FRAME_SAMPLES];
        for seed in 1..10 {
            decoder.decode(&voice_frame(seed), &mut first);
        }
        decoder.reset();
        for seed in 1..10 {
            decoder.decode(&voice_frame(seed), &mut second);
        }
        assert_eq!(first, second);
    }
}
