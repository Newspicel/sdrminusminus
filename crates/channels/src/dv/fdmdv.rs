mod decimator;
mod demap;
mod downconvert;
mod pilot;
mod sync;
mod tables;
mod timing;

#[cfg(test)]
mod modulator;

use std::ops::{Add, Mul};

use num_complex::Complex;

use decimator::Decimator;
use downconvert::{Downconverter, Filtered};
use pilot::CoarseFrequency;
use sync::SyncTracker;
use timing::TimingEstimator;

pub(crate) const BITS: usize = DATA_CARRIERS * 2;
const DATA_CARRIERS: usize = 16;
const CARRIERS: usize = DATA_CARRIERS + 1;
const PILOT: usize = DATA_CARRIERS;
const CARRIER_SPACING_HZ: f32 = 75.0;
const CENTRE_HZ: f32 = 1_500.0;
const SAMPLE_RATE_HZ: f64 = 8_000.0;
const SYMBOL_SAMPLES: usize = 160;
const OVERSAMPLING: usize = 4;
const OVERSAMPLED_STEP: usize = SYMBOL_SAMPLES / OVERSAMPLING;
const SYMBOL_SPAN: usize = 6;
const FILTER_TAPS: usize = SYMBOL_SPAN * SYMBOL_SAMPLES;
const MAX_FRAME: usize = SYMBOL_SAMPLES + OVERSAMPLED_STEP;
const FREQUENCY_TRACK_GAIN: f64 = 0.5;

#[expect(clippy::approx_constant)]
const CODEC2_PI: f64 = 3.141_592_654;

type Sample = Complex<f32>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameLength {
    Short,
    Nominal,
    Long,
}

impl FrameLength {
    const fn samples(self) -> usize {
        self.oversampled() * OVERSAMPLED_STEP
    }

    const fn oversampled(self) -> usize {
        match self {
            Self::Short => OVERSAMPLING - 1,
            Self::Nominal => OVERSAMPLING,
            Self::Long => OVERSAMPLING + 1,
        }
    }

    fn after(timing_samples: f32) -> Self {
        let limit = OVERSAMPLED_STEP as f32;
        if timing_samples > limit {
            Self::Long
        } else if timing_samples < -limit {
            Self::Short
        } else {
            Self::Nominal
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) bits: [bool; BITS],
    pub(crate) sync: bool,
    pub(crate) reliable_sync: bool,
}

pub(crate) struct Demodulator {
    input: [Sample; MAX_FRAME],
    filled: usize,
    length: FrameLength,
    to_baseband: Oscillator,
    correction: Oscillator,
    frequency_offset: f32,
    coarse: CoarseFrequency,
    decimator: Decimator,
    downconverter: Downconverter,
    timing: TimingEstimator,
    previous_symbols: [Sample; CARRIERS],
    sync: SyncTracker,
}

impl Demodulator {
    pub(crate) fn new() -> Self {
        Self::with_coarse(CoarseFrequency::new())
    }

    fn with_coarse(coarse: CoarseFrequency) -> Self {
        Self {
            input: [Sample::ZERO; MAX_FRAME],
            filled: 0,
            length: FrameLength::Nominal,
            to_baseband: Oscillator::new(-CENTRE_HZ),
            correction: Oscillator::new(0.0),
            frequency_offset: 0.0,
            coarse,
            decimator: Decimator::new(),
            downconverter: Downconverter::new(),
            timing: TimingEstimator::new(),
            previous_symbols: [Sample::ONE; CARRIERS],
            sync: SyncTracker::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        let coarse = self.coarse.restarted();
        *self = Self::with_coarse(coarse);
    }

    pub(crate) fn push(&mut self, sample: Sample) -> Option<Frame> {
        self.input[self.filled] = if sample.is_finite() {
            sample
        } else {
            Sample::ZERO
        };
        self.filled += 1;
        if self.filled < self.length.samples() {
            return None;
        }
        self.filled = 0;
        Some(self.demodulate())
    }

    fn demodulate(&mut self) -> Frame {
        let length = self.length;
        let samples = &mut self.input[..length.samples()];
        self.to_baseband.mix(samples);
        let locked = self.sync.locked();
        let coarse = self.coarse.estimate(samples, !locked);
        if !locked {
            self.frequency_offset = coarse;
        }
        self.correction.retune(-self.frequency_offset);
        self.correction.mix(samples);
        self.decimator
            .filter(samples, self.downconverter.admit(length.samples()));
        let mut filtered: Filtered = [[Sample::ZERO; OVERSAMPLING + 1]; CARRIERS];
        self.downconverter.filter(length, &mut filtered);
        let mut symbols = [Sample::ZERO; CARRIERS];
        let timing = self.timing.estimate(&filtered, length, &mut symbols);
        self.length = FrameLength::after(timing);

        let decision = demap::decide(&symbols, &self.previous_symbols);
        self.previous_symbols = symbols;
        let (sync, reliable_sync) = self.sync.update(decision.sync_bit);
        self.track_frequency(decision.frequency_error);
        Frame {
            bits: decision.bits,
            sync,
            reliable_sync,
        }
    }

    fn track_frequency(&mut self, error: f32) {
        let tracked =
            (f64::from(self.frequency_offset) - FREQUENCY_TRACK_GAIN * f64::from(error)) as f32;
        if tracked.is_finite() {
            self.frequency_offset = tracked;
        }
    }
}

struct Oscillator {
    phase: Sample,
    step: Sample,
}

impl Oscillator {
    fn new(frequency_hz: f32) -> Self {
        Self {
            phase: Sample::ONE,
            step: rotation(frequency_hz),
        }
    }

    fn retune(&mut self, frequency_hz: f32) {
        self.step = rotation(frequency_hz);
    }

    fn mix(&mut self, samples: &mut [Sample]) {
        for sample in samples {
            self.phase *= self.step;
            *sample *= self.phase;
        }
        self.phase = unit(self.phase);
    }
}

fn carrier_radians(index: usize) -> f32 {
    let half = (DATA_CARRIERS / 2) as i32;
    let slot = match index {
        PILOT => 0,
        low if (low as i32) < half => low as i32 - half,
        high => high as i32 - half + 1,
    };
    radians_per_sample(slot as f32 * CARRIER_SPACING_HZ)
}

fn rotation(frequency_hz: f32) -> Sample {
    cis(radians_per_sample(frequency_hz))
}

fn radians_per_sample(frequency_hz: f32) -> f32 {
    (2.0 * CODEC2_PI * f64::from(frequency_hz) / SAMPLE_RATE_HZ) as f32
}

fn cis(radians: f32) -> Sample {
    Sample::new(radians.cos(), radians.sin())
}

fn magnitude(value: Sample) -> f32 {
    value.norm_sqr().sqrt()
}

fn unit(value: Sample) -> Sample {
    value / magnitude(value)
}

fn fir(taps: &[f32], history: &[Sample], output: &mut [Sample]) {
    output.fill(Sample::ZERO);
    for (offset, &tap) in taps.iter().enumerate() {
        for (out, &sample) in output.iter_mut().zip(&history[offset..]) {
            *out += sample.scale(tap);
        }
    }
}

fn shaped<T>(history: &[T; SYMBOL_SPAN], offset: usize) -> T
where
    T: Copy + Add<Output = T> + Mul<f32, Output = T> + Default,
{
    history
        .iter()
        .zip(
            tables::ROOT_RAISED_COSINE[SYMBOL_SAMPLES - 1 - offset..]
                .iter()
                .step_by(SYMBOL_SAMPLES),
        )
        .fold(T::default(), |acc, (&symbol, &tap)| {
            acc + symbol * SYMBOL_SAMPLES as f32 * tap
        })
}

#[cfg(test)]
mod tests {
    use super::{modulator::Modulator, *};

    const MODEM_DELAY_FRAMES: usize = 11;

    fn random_bits(seed: &mut u64) -> [bool; BITS] {
        std::array::from_fn(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed & 1 == 1
        })
    }

    fn loopback(frames: usize, offset_hz: f32) -> (Vec<[bool; BITS]>, Vec<Frame>) {
        let mut modulator = Modulator::new();
        let mut demodulator = Demodulator::new();
        let mut shift = Oscillator::new(offset_hz);
        let mut seed = 0x1234_5678;
        let mut sent = Vec::new();
        let mut received = Vec::new();
        for _ in 0..frames {
            let bits = random_bits(&mut seed);
            let mut signal = modulator.modulate(&bits);
            shift.mix(&mut signal);
            sent.push(bits);
            received.extend(signal.iter().filter_map(|&s| demodulator.push(s)));
        }
        (sent, received)
    }

    fn decodes_with_modem_delay(sent: &[[bool; BITS]], received: &[Frame]) -> bool {
        let start = received.len() - 20;
        received[start..]
            .iter()
            .zip(&sent[start - MODEM_DELAY_FRAMES..])
            .all(|(frame, bits)| frame.bits == *bits)
    }

    #[test]
    fn a_clean_signal_locks_and_decodes_every_bit() {
        let (sent, received) = loopback(200, 0.0);
        assert!(received[60..].iter().all(|frame| frame.sync));
        assert!(decodes_with_modem_delay(&sent, &received));
    }

    #[test]
    fn a_frequency_offset_is_pulled_in() {
        for offset in [-60.0, 35.0, 90.0] {
            let (sent, received) = loopback(250, offset);
            assert!(
                received.last().is_some_and(|frame| frame.sync),
                "{offset} Hz"
            );
            assert!(decodes_with_modem_delay(&sent, &received), "{offset} Hz");
        }
    }

    #[test]
    fn silence_never_syncs() {
        let mut demodulator = Demodulator::new();
        let frames: Vec<Frame> = (0..16_000)
            .filter_map(|_| demodulator.push(Sample::ZERO))
            .collect();
        assert_eq!(frames.len(), 100);
        assert!(
            frames
                .iter()
                .all(|frame| !frame.sync && !frame.reliable_sync)
        );
    }

    #[test]
    fn non_finite_samples_do_not_poison_the_modem() {
        let (_, clean) = loopback(120, 0.0);
        assert!(clean.last().is_some_and(|frame| frame.sync));
        let mut modulator = Modulator::new();
        let mut demodulator = Demodulator::new();
        let mut seed = 7;
        let mut last = None;
        for frame in 0..200 {
            let mut signal = modulator.modulate(&random_bits(&mut seed));
            if frame == 50 {
                signal[3] = Sample::new(f32::NAN, f32::INFINITY);
            }
            for sample in signal {
                last = demodulator.push(sample).or(last);
            }
        }
        assert!(last.is_some_and(|frame| frame.sync));
    }

    #[test]
    fn an_overflowing_burst_is_survived() {
        let mut modulator = Modulator::new();
        let mut demodulator = Demodulator::new();
        let mut seed = 11;
        let mut frames = Vec::new();
        for frame in 0..400 {
            let gain = if (100..110).contains(&frame) {
                1e30
            } else {
                1.0
            };
            for sample in modulator.modulate(&random_bits(&mut seed)) {
                frames.extend(demodulator.push(sample * gain));
            }
        }
        assert!(frames[90].sync);
        assert!(frames.last().is_some_and(|frame| frame.sync));
        assert!(demodulator.frequency_offset.is_finite());
    }

    #[test]
    fn reset_restarts_from_scratch() {
        let mut modulator = Modulator::new();
        let mut seed = 3;
        let signal: Vec<Sample> = (0..40)
            .flat_map(|_| modulator.modulate(&random_bits(&mut seed)))
            .collect();
        let mut demodulator = Demodulator::new();
        let first: Vec<Frame> = signal.iter().filter_map(|&s| demodulator.push(s)).collect();
        demodulator.reset();
        let second: Vec<Frame> = signal.iter().filter_map(|&s| demodulator.push(s)).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn frame_length_follows_the_timing_estimate() {
        assert_eq!(FrameLength::after(41.0), FrameLength::Long);
        assert_eq!(FrameLength::after(-41.0), FrameLength::Short);
        assert_eq!(FrameLength::after(40.0), FrameLength::Nominal);
        assert_eq!(
            [FrameLength::Short, FrameLength::Nominal, FrameLength::Long].map(FrameLength::samples),
            [120, 160, 200]
        );
    }
}
