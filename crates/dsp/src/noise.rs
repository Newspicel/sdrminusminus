use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{iir::one_pole_coeff, window::hann};

#[derive(Clone, Debug)]
pub struct NoiseBlanker {
    delay: Vec<Complex<f32>>,
    write: usize,
    average: f32,
    coeff: f32,
    threshold: f32,
    remaining: usize,
    window: usize,
    blanked: u64,
}

impl NoiseBlanker {
    const HALF_WINDOW_S: f64 = 40e-6;
    const AVERAGE_TAU_S: f64 = 0.05;

    #[must_use]
    pub fn new(rate: f64, threshold: f32) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let half = ((rate * Self::HALF_WINDOW_S).round() as usize).max(1);
        Self {
            delay: vec![Complex::new(0.0, 0.0); half],
            write: 0,
            average: 0.0,
            coeff: one_pole_coeff(rate, Self::AVERAGE_TAU_S),
            threshold: threshold.max(1.0),
            remaining: 0,
            window: 2 * half + 1,
            blanked: 0,
        }
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.max(1.0);
    }

    #[must_use]
    pub fn blanked_samples(&self) -> u64 {
        self.blanked
    }

    pub fn reset(&mut self) {
        self.delay.fill(Complex::new(0.0, 0.0));
        self.write = 0;
        self.average = 0.0;
        self.remaining = 0;
    }

    pub fn process(&mut self, iq: &mut [Complex<f32>]) {
        if !self.average.is_finite() {
            self.reset();
        }
        for sample in iq {
            let input = if sample.re.is_finite() && sample.im.is_finite() {
                *sample
            } else {
                Complex::new(0.0, 0.0)
            };
            let delayed = self.delay[self.write];
            self.delay[self.write] = input;
            self.write = (self.write + 1) % self.delay.len();

            let magnitude = input.norm();
            if self.remaining == 0 {
                if self.average <= 0.0 {
                    self.average = magnitude;
                } else {
                    self.average += self.coeff * (magnitude - self.average);
                }
                if magnitude > self.threshold * self.average.max(f32::MIN_POSITIVE) {
                    self.remaining = self.window;
                }
            }
            if self.remaining > 0 {
                self.remaining -= 1;
                self.blanked += 1;
                *sample = Complex::new(0.0, 0.0);
            } else {
                *sample = delayed;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClickRemover {
    window: Vec<f32>,
    write: usize,
    sorted: Vec<f32>,
    half: usize,
    average: f32,
    coeff: f32,
    threshold: f32,
    removed: u64,
}

impl ClickRemover {
    const AVERAGE_TAU_S: f64 = 0.05;

    #[must_use]
    pub fn new(rate: f64, width_s: f64, threshold: f32) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let half = ((rate * width_s / 2.0).round() as usize).max(1);
        let span = 2 * half + 1;
        Self {
            window: vec![0.0; span],
            write: 0,
            sorted: vec![0.0; span],
            half,
            average: 0.0,
            coeff: one_pole_coeff(rate, Self::AVERAGE_TAU_S),
            threshold: threshold.max(1.0),
            removed: 0,
        }
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.max(1.0);
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        self.half
    }

    #[must_use]
    pub fn removed_samples(&self) -> u64 {
        self.removed
    }

    pub fn reset(&mut self) {
        self.window.fill(0.0);
        self.write = 0;
        self.average = 0.0;
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.average.is_finite() {
            self.reset();
        }
        let span = self.window.len();
        for slot in samples {
            let x = if slot.is_finite() { *slot } else { 0.0 };
            self.write = (self.write + 1) % span;
            self.window[self.write] = x;

            let centre = self.window[(self.write + self.half + 1) % span];
            let magnitude = centre.abs();
            if self.average <= 0.0 {
                self.average = magnitude;
            } else {
                self.average += self.coeff * (magnitude - self.average);
            }

            let limit = self.threshold * self.average.max(f32::MIN_POSITIVE);
            *slot = if magnitude > limit {
                let median = self.median();
                if (centre - median).abs() > limit {
                    self.removed += 1;
                    median
                } else {
                    centre
                }
            } else {
                centre
            };
        }
    }

    fn median(&mut self) -> f32 {
        self.sorted.copy_from_slice(&self.window);
        self.sorted.sort_unstable_by(f32::total_cmp);
        self.sorted[self.window.len() / 2]
    }
}

#[derive(Clone, Debug)]
pub struct AutoNotch {
    weights: Vec<f32>,
    history: Vec<f32>,
    write: usize,
    delay: usize,
    mu: f32,
}

impl AutoNotch {
    const TAPS: usize = 64;
    const DELAY: usize = 4;
    const MU: f32 = 0.02;

    #[must_use]
    pub fn new() -> Self {
        Self {
            weights: vec![0.0; Self::TAPS],
            history: vec![0.0; (Self::TAPS + Self::DELAY).next_power_of_two()],
            write: 0,
            delay: Self::DELAY,
            mu: Self::MU,
        }
    }

    pub fn reset(&mut self) {
        self.weights.fill(0.0);
        self.history.fill(0.0);
        self.write = 0;
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.weights.iter().all(|w| w.is_finite()) {
            self.reset();
        }
        let mask = self.history.len() - 1;
        let taps = self.weights.len();
        for s in samples {
            let x = if s.is_finite() { *s } else { 0.0 };
            self.write = (self.write + 1) & mask;
            self.history[self.write] = x;

            let base = self.write + self.history.len() - self.delay;
            let mut predicted = 0.0;
            let mut reference_power = 0.0;
            for k in 0..taps {
                let r = self.history[(base - k) & mask];
                predicted += self.weights[k] * r;
                reference_power += r * r;
            }
            let error = x - predicted;
            let step = self.mu * error / (reference_power + 1e-6);
            for k in 0..taps {
                self.weights[k] += step * self.history[(base - k) & mask];
            }
            *s = error;
        }
    }
}

impl Default for AutoNotch {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SpectralDenoiser {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    pending: Vec<f32>,
    overlap: Vec<f32>,
    ready: Vec<f32>,
    read: usize,
    smoothed: Vec<f32>,
    floor: Vec<f32>,
    gains: Vec<f32>,
    strength: f32,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl SpectralDenoiser {
    pub const FRAME: usize = 512;
    pub const HOP: usize = Self::FRAME / 2;
    const GAIN_FLOOR: f32 = 0.1;
    const POWER_SMOOTHING: f32 = 0.3;
    const FLOOR_RISE: f32 = 0.003;
    const GAIN_SMOOTHING: f32 = 0.5;
    const FLOOR_BIAS: f32 = 5.0;

    #[must_use]
    pub fn new(strength: f32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(Self::FRAME);
        let ifft = planner.plan_fft_inverse(Self::FRAME);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        Self {
            fft,
            ifft,
            window: hann(Self::FRAME).iter().map(|w| w.sqrt()).collect(),
            pending: Vec::with_capacity(Self::FRAME * 4),
            overlap: vec![0.0; Self::FRAME],
            ready: vec![0.0; Self::FRAME],
            read: 0,
            smoothed: vec![0.0; Self::FRAME],
            floor: vec![0.0; Self::FRAME],
            gains: vec![1.0; Self::FRAME],
            strength: strength.clamp(0.0, 1.0),
            spectrum: vec![Complex::new(0.0, 0.0); Self::FRAME],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
        }
    }

    pub fn set_strength(&mut self, strength: f32) {
        self.strength = strength.clamp(0.0, 1.0);
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        Self::FRAME
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.overlap.fill(0.0);
        self.ready.clear();
        self.ready.resize(Self::FRAME, 0.0);
        self.read = 0;
        self.smoothed.fill(0.0);
        self.floor.fill(0.0);
        self.gains.fill(1.0);
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if self.read > 0 {
            self.ready.drain(..self.read.min(self.ready.len()));
            self.read = 0;
        }
        self.pending
            .extend(samples.iter().map(|s| if s.is_finite() { *s } else { 0.0 }));
        let mut consumed = 0;
        while self.pending.len() - consumed >= Self::FRAME {
            self.transform(consumed);
            consumed += Self::HOP;
        }
        self.pending.drain(..consumed);
        for s in samples.iter_mut() {
            *s = self.ready.get(self.read).copied().unwrap_or(0.0);
            self.read += 1;
        }
    }

    fn transform(&mut self, offset: usize) {
        let frame = &self.pending[offset..offset + Self::FRAME];
        for ((dst, &s), &w) in self.spectrum.iter_mut().zip(frame).zip(&self.window) {
            *dst = Complex::new(s * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        self.shape();
        self.ifft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);

        let norm = 1.0 / Self::FRAME as f32;
        for ((slot, bin), &w) in self
            .overlap
            .iter_mut()
            .zip(&self.spectrum)
            .zip(&self.window)
        {
            *slot += bin.re * norm * w;
        }
        self.ready.extend_from_slice(&self.overlap[..Self::HOP]);
        self.overlap.copy_within(Self::HOP.., 0);
        self.overlap[Self::FRAME - Self::HOP..].fill(0.0);
    }

    fn shape(&mut self) {
        let strength = self.strength;
        for (((bin, smoothed), floor), gain) in self
            .spectrum
            .iter_mut()
            .zip(&mut self.smoothed)
            .zip(&mut self.floor)
            .zip(&mut self.gains)
        {
            *smoothed += Self::POWER_SMOOTHING * (bin.norm_sqr() - *smoothed);
            if *smoothed < *floor {
                *floor = *smoothed;
            } else {
                *floor += Self::FLOOR_RISE * (*smoothed - *floor);
            }
            let wanted = if *smoothed > f32::MIN_POSITIVE {
                (1.0 - strength * Self::FLOOR_BIAS * *floor / *smoothed)
                    .clamp(Self::GAIN_FLOOR, 1.0)
            } else {
                1.0
            };
            *gain += Self::GAIN_SMOOTHING * (wanted - *gain);
            *bin *= *gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{XorShift32, real_tone, rms_c, rms_r};

    const RATE: f64 = 48_000.0;

    fn noise_iq(seed: u32, amplitude: f32, len: usize) -> Vec<Complex<f32>> {
        let mut rng = XorShift32(seed);
        (0..len)
            .map(|_| Complex::new(rng.next_f32() * amplitude, rng.next_f32() * amplitude))
            .collect()
    }

    fn carrier(len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|n| Complex::from_polar(0.1, (n as f32) * 0.01))
            .collect()
    }

    fn with_impulses(len: usize, period: usize, amplitude: f32) -> Vec<Complex<f32>> {
        let mut iq = carrier(len);
        for n in (period..len).step_by(period) {
            iq[n] = Complex::new(amplitude, 0.0);
        }
        iq
    }

    #[test]
    fn blanker_cuts_impulses_out_and_leaves_the_carrier() {
        let mut blanked = with_impulses(24_000, 500, 5.0);
        NoiseBlanker::new(RATE, 4.0).process(&mut blanked);

        let settled = 4_000;
        let peak = blanked[settled..]
            .iter()
            .map(|s| s.norm())
            .fold(0.0f32, f32::max);
        assert!(peak < 0.2, "impulse survived at {peak}");
        let kept = rms_c(&blanked[settled..]);
        let original = rms_c(&carrier(24_000)[settled..]);
        assert!(
            kept > 0.9 * original,
            "carrier lost: {kept} of {original} left"
        );
        assert!(kept < 1.1 * original);
    }

    #[test]
    fn blanker_leaves_a_clean_channel_alone() {
        let clean = noise_iq(0x51D3_2AA1, 0.2, 48_000);
        let mut through = clean.clone();
        let mut blanker = NoiseBlanker::new(RATE, 6.0);
        blanker.process(&mut through);
        let before = rms_c(&clean[8_000..]);
        let after = rms_c(&through[8_000..]);
        assert!(
            (after / before - 1.0).abs() < 0.05,
            "an undisturbed channel was chewed up: {before} -> {after}"
        );
    }

    #[test]
    fn a_lower_threshold_blanks_more() {
        let iq = with_impulses(48_000, 300, 0.5);
        let counts: Vec<u64> = [2.5f32, 8.0]
            .iter()
            .map(|&t| {
                let mut blanker = NoiseBlanker::new(RATE, t);
                blanker.process(&mut iq.clone());
                blanker.blanked_samples()
            })
            .collect();
        assert!(counts[0] > counts[1], "{counts:?}");
    }

    #[test]
    fn blanker_recovers_after_a_non_finite_sample() {
        let mut blanker = NoiseBlanker::new(RATE, 4.0);
        let mut poisoned = vec![Complex::new(f32::NAN, 0.0); 64];
        poisoned[10] = Complex::new(f32::INFINITY, 0.0);
        blanker.process(&mut poisoned);
        assert!(
            poisoned
                .iter()
                .all(|s| s.re.is_finite() && s.im.is_finite())
        );
        let mut back = carrier(24_000);
        blanker.process(&mut back);
        assert!(rms_c(&back[8_000..]) > 0.09, "channel stayed muted");
    }

    const CLICK_WIDTH_S: f64 = 100e-6;
    const CLICK_THRESHOLD: f32 = 6.0;

    fn click_remover() -> ClickRemover {
        ClickRemover::new(RATE, CLICK_WIDTH_S, CLICK_THRESHOLD)
    }

    fn speech_with_clicks(len: usize, period: usize, amplitude: f32) -> Vec<f32> {
        let mut audio = real_tone(700.0 / RATE, len)
            .iter()
            .map(|s| s * 0.2)
            .collect::<Vec<_>>();
        for n in (period..len).step_by(period) {
            audio[n] = amplitude;
        }
        audio
    }

    fn run_clicks(remover: &mut ClickRemover, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(block) {
            let mut buf = chunk.to_vec();
            remover.process(&mut buf);
            out.extend_from_slice(&buf);
        }
        out
    }

    #[test]
    fn clicks_are_cut_out_and_the_audio_under_them_is_kept() {
        let input = speech_with_clicks(48_000, 500, 3.0);
        let mut remover = click_remover();
        let output = run_clicks(&mut remover, &input, 480);

        let settled = 8_000;
        let peak = output[settled..].iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak < 0.3, "a click survived at {peak}");
        assert!(remover.removed_samples() > 0);
        let latency = remover.latency();
        let kept = tone_amplitude(&output[settled + latency..], 700.0);
        assert!((kept - 0.2).abs() < 0.02, "audio was chewed up: {kept}");
    }

    #[test]
    fn clean_audio_passes_through_untouched() {
        let mut rng = XorShift32(0x4B7C_1E39);
        let input: Vec<f32> = real_tone(900.0 / RATE, 48_000)
            .iter()
            .map(|t| t * 0.3 + rng.next_f32() * 0.05)
            .collect();
        let mut remover = click_remover();
        let output = run_clicks(&mut remover, &input, 997);
        let latency = remover.latency();
        for n in 4_000..input.len() - latency {
            assert!(
                (output[n + latency] - input[n]).abs() < 1e-6,
                "sample {n} was altered: {} vs {}",
                output[n + latency],
                input[n]
            );
        }
        assert_eq!(remover.removed_samples(), 0);
    }

    #[test]
    fn a_loud_transient_that_is_not_an_impulse_is_left_alone() {
        let mut input = vec![0.0f32; 24_000];
        input.extend(real_tone(500.0 / RATE, 24_000).iter().map(|s| s * 0.4));
        let mut remover = click_remover();
        let output = run_clicks(&mut remover, &input, 480);
        let latency = remover.latency();
        let burst = tone_amplitude(&output[28_000 + latency..], 500.0);
        assert!(burst > 0.35, "the burst was cut down to {burst}");
    }

    #[test]
    fn click_removal_returns_one_sample_for_every_sample_it_is_given() {
        let mut remover = click_remover();
        let input = speech_with_clicks(24_000, 300, 4.0);
        let (mut produced, mut pos) = (0, 0);
        for len in [13usize, 1_024, 97, 4_096].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            let mut buf = input[pos..end].to_vec();
            remover.process(&mut buf);
            assert_eq!(buf.len(), end - pos);
            produced += buf.len();
            pos = end;
        }
        assert_eq!(produced, input.len());
    }

    #[test]
    fn a_wider_window_removes_a_wider_click() {
        let mut input = real_tone(700.0 / RATE, 48_000)
            .iter()
            .map(|s| s * 0.2)
            .collect::<Vec<_>>();
        for n in (2_000..input.len()).step_by(1_000) {
            input[n..n + 6].fill(3.0);
        }
        let peak_after = |width_s: f64| {
            let mut remover = ClickRemover::new(RATE, width_s, CLICK_THRESHOLD);
            let output = run_clicks(&mut remover, &input, 480);
            output[8_000..].iter().fold(0.0f32, |a, s| a.max(s.abs()))
        };
        assert!(
            peak_after(100e-6) > 1.0,
            "the narrow window kept up somehow"
        );
        assert!(peak_after(400e-6) < 0.3, "the wide window missed the click");
    }

    #[test]
    fn click_removal_recovers_after_a_non_finite_sample() {
        let mut remover = click_remover();
        let mut poisoned = vec![f32::NAN; 256];
        poisoned[3] = f32::INFINITY;
        remover.process(&mut poisoned);
        assert!(poisoned.iter().all(|s| s.is_finite()));
        let mut back = real_tone(700.0 / RATE, 24_000)
            .iter()
            .map(|s| s * 0.2)
            .collect::<Vec<_>>();
        remover.process(&mut back);
        assert!(rms_r(&back[8_000..]) > 0.1, "audio stayed muted");
    }

    fn tone_amplitude(x: &[f32], freq_hz: f64) -> f32 {
        let w = std::f64::consts::TAU * freq_hz / RATE;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &s) in x.iter().enumerate() {
            re += f64::from(s) * (w * n as f64).cos();
            im += f64::from(s) * (w * n as f64).sin();
        }
        (2.0 * re.hypot(im) / x.len() as f64) as f32
    }

    fn tone_in_noise(tone_hz: f64, len: usize) -> Vec<f32> {
        let mut rng = XorShift32(0x1B3F_9C05);
        real_tone(tone_hz / RATE, len)
            .iter()
            .map(|t| t * 0.5 + rng.next_f32() * 0.2)
            .collect()
    }

    #[test]
    fn auto_notch_removes_an_unannounced_carrier() {
        let input = tone_in_noise(1_500.0, 96_000);
        let mut output = input.clone();
        AutoNotch::new().process(&mut output);

        let settled = 48_000;
        let before = tone_amplitude(&input[settled..], 1_500.0);
        let after = tone_amplitude(&output[settled..], 1_500.0);
        assert!(after < 0.2 * before, "tone {before} -> {after}");
    }

    #[test]
    fn auto_notch_keeps_the_noise_the_carrier_was_in() {
        let mut rng = XorShift32(0x77AA_0913);
        let input: Vec<f32> = (0..96_000).map(|_| rng.next_f32() * 0.2).collect();
        let mut output = input.clone();
        AutoNotch::new().process(&mut output);
        let settled = 48_000;
        let ratio = rms_r(&output[settled..]) / rms_r(&input[settled..]);
        assert!((0.8..1.2).contains(&ratio), "noise gain {ratio}");
    }

    #[test]
    fn auto_notch_recovers_after_a_non_finite_sample() {
        let mut notch = AutoNotch::new();
        let mut poisoned = vec![f32::NAN; 256];
        notch.process(&mut poisoned);
        assert!(poisoned.iter().all(|s| s.is_finite()));
        let mut clean = tone_in_noise(1_000.0, 48_000);
        notch.process(&mut clean);
        assert!(clean.iter().all(|s| s.is_finite()), "state still poisoned");
    }

    fn run_denoiser(denoiser: &mut SpectralDenoiser, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(block) {
            let mut buf = chunk.to_vec();
            denoiser.process(&mut buf);
            out.extend_from_slice(&buf);
        }
        out
    }

    #[test]
    fn denoiser_returns_one_sample_for_every_sample_it_is_given() {
        let mut denoiser = SpectralDenoiser::new(0.5);
        let input = real_tone(1_000.0 / RATE, 48_000);
        let (mut produced, mut pos) = (0, 0);
        for len in [37usize, 1_024, 511, 4_096].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            let mut buf = input[pos..end].to_vec();
            denoiser.process(&mut buf);
            assert_eq!(buf.len(), end - pos);
            produced += buf.len();
            pos = end;
        }
        assert_eq!(produced, input.len());
    }

    #[test]
    fn denoiser_at_zero_strength_reconstructs_the_input() {
        let mut denoiser = SpectralDenoiser::new(0.0);
        let input = real_tone(1_000.0 / RATE, 24_000);
        let output = run_denoiser(&mut denoiser, &input, 480);
        let latency = denoiser.latency();
        for n in 2_000..input.len() - latency {
            let error = (output[n + latency] - input[n]).abs();
            assert!(
                error < 1e-3,
                "sample {n}: {} vs {}",
                output[n + latency],
                input[n]
            );
        }
    }

    fn bursts_in_hiss(len: usize) -> Vec<f32> {
        let mut rng = XorShift32(0x2C41_66B7);
        let burst = (RATE * 0.2) as usize;
        real_tone(1_000.0 / RATE, len)
            .iter()
            .enumerate()
            .map(|(n, t)| {
                let on = (n / burst).is_multiple_of(2);
                (if on { t * 0.4 } else { 0.0 }) + rng.next_f32() * 0.1
            })
            .collect()
    }

    #[test]
    fn denoiser_quietens_the_gaps_and_keeps_the_bursts() {
        let input = bursts_in_hiss(480_000);
        let mut denoiser = SpectralDenoiser::new(1.0);
        let output = run_denoiser(&mut denoiser, &input, 960);
        let burst = (RATE * 0.2) as usize;
        let latency = denoiser.latency();

        let inside = |index: usize| {
            let start = index * burst + burst / 4 + latency;
            start..start + burst / 2
        };
        let tone_before = tone_amplitude(&input[inside(4)], 1_000.0);
        let tone_after = tone_amplitude(&output[inside(4)], 1_000.0);
        assert!(
            tone_after > 0.7 * tone_before,
            "speech was eaten: {tone_before} -> {tone_after}"
        );

        let gap_before = rms_r(&input[inside(5)]);
        let gap_after = rms_r(&output[inside(5)]);
        assert!(
            gap_after < 0.4 * gap_before,
            "hiss survived: {gap_before} -> {gap_after}"
        );
    }

    #[test]
    fn denoiser_recovers_after_a_non_finite_sample() {
        let mut denoiser = SpectralDenoiser::new(0.8);
        let mut poisoned = vec![f32::NAN; 2_048];
        denoiser.process(&mut poisoned);
        assert!(poisoned.iter().all(|s| s.is_finite()));
        let input = bursts_in_hiss(240_000);
        let output = run_denoiser(&mut denoiser, &input, 480);
        assert!(output.iter().all(|s| s.is_finite()), "state still poisoned");
        let burst = (RATE * 0.2) as usize;
        let start = 4 * burst + burst / 4 + denoiser.latency();
        let amplitude = tone_amplitude(&output[start..start + burst / 2], 1_000.0);
        assert!(amplitude > 0.2, "audio never came back: {amplitude}");
    }
}
