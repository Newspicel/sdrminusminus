use nnnoiseless::DenoiseState;

const FRAME: usize = DenoiseState::FRAME_SIZE;
const PCM_SCALE: f32 = 32_768.0;
const MAX_ATTENUATION_DB: f32 = 30.0;

pub struct RnnoiseDenoiser {
    state: Box<DenoiseState<'static>>,
    input: [f32; FRAME],
    previous: [f32; FRAME],
    wet: [f32; FRAME],
    mixed: [f32; FRAME],
    filled: usize,
    dry_mix: f32,
}

impl RnnoiseDenoiser {
    #[must_use]
    pub fn new(strength: f32) -> Self {
        let mut denoiser = Self {
            state: DenoiseState::new(),
            input: [0.0; FRAME],
            previous: [0.0; FRAME],
            wet: [0.0; FRAME],
            mixed: [0.0; FRAME],
            filled: 0,
            dry_mix: 0.0,
        };
        denoiser.set_strength(strength);
        denoiser
    }

    pub fn set_strength(&mut self, strength: f32) {
        let strength = strength.clamp(0.0, 1.0);
        self.dry_mix = 10f32.powf(-strength * MAX_ATTENUATION_DB / 20.0);
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        2 * FRAME
    }

    pub fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.input.fill(0.0);
        self.previous.fill(0.0);
        self.mixed.fill(0.0);
        self.filled = 0;
    }

    pub fn process(&mut self, pcm: &mut [f32]) {
        for sample in pcm {
            self.input[self.filled] = *sample * PCM_SCALE;
            *sample = self.mixed[self.filled];
            self.filled += 1;
            if self.filled == FRAME {
                self.run_frame();
                self.filled = 0;
            }
        }
    }

    fn run_frame(&mut self) {
        self.state.process_frame(&mut self.wet, &self.input);
        let wet = 1.0 - self.dry_mix;
        for ((out, &dry), &clean) in self.mixed.iter_mut().zip(&self.previous).zip(&self.wet) {
            *out = (dry * self.dry_mix + clean * wet) / PCM_SCALE;
        }
        self.previous = self.input;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::rms;

    fn hiss(len: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let white: Vec<f32> = (0..len + 7)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                amplitude * ((state >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
            })
            .collect();
        white
            .windows(8)
            .map(|w| w.iter().sum::<f32>() / 8.0)
            .collect()
    }

    fn tone(len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| 0.3 * (std::f32::consts::TAU * 440.0 * n as f32 / 48_000.0).sin())
            .collect()
    }

    fn run(denoiser: &mut RnnoiseDenoiser, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for block in input.chunks(733) {
            let mut block = block.to_vec();
            denoiser.process(&mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn it_quietens_noise_with_nobody_talking() {
        let input = hiss(96_000, 0.1);
        let output = run(&mut RnnoiseDenoiser::new(1.0), &input);
        let (before, after) = (rms(&input[48_000..]), rms(&output[48_000..]));
        assert!(
            after < before * 0.1,
            "noise only fell from {before} to {after}"
        );
    }

    #[test]
    fn zero_strength_hands_back_the_input_delayed_by_its_latency() {
        let input = tone(48_000);
        let mut denoiser = RnnoiseDenoiser::new(0.0);
        let lag = denoiser.latency();
        let output = run(&mut denoiser, &input);
        let error = rms(&output[lag + 4_800..]
            .iter()
            .zip(&input[4_800..])
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        assert!(error < 1e-4, "residual {error}");
    }

    #[test]
    fn full_strength_lines_the_model_up_with_the_dry_path() {
        let input = tone(48_000);
        let mut denoiser = RnnoiseDenoiser::new(0.5);
        let lag = denoiser.latency();
        let output = run(&mut denoiser, &input);
        let error = rms(&output[lag + 9_600..]
            .iter()
            .zip(&input[9_600..])
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        assert!(
            error < 0.5 * rms(&input),
            "dry and wet are out of step: {error}"
        );
    }
}
