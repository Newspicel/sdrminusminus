use num_complex::Complex;
use sdrmm_dsp::{Highpass, Nco, RealDecimator, SpectrumAnalyzer, design_bandpass, design_lowpass};
use sdrmm_modem::analog::{Delay, design_hilbert};

pub(crate) const MIN_INVERSION_HZ: f64 = 1_500.0;
pub(crate) const MAX_INVERSION_HZ: f64 = 4_500.0;
pub(crate) const DEFAULT_INVERSION_HZ: f64 = 3_300.0;

const AUDIO_EDGE_HZ: f64 = 250.0;
const HILBERT_TAPS: usize = 513;
const BAND_TAPS: usize = 769;

#[must_use]
pub(crate) fn is_supported_inversion(hz: f64) -> bool {
    (MIN_INVERSION_HZ..=MAX_INVERSION_HZ).contains(&hz)
}

fn band_filter(rate: f64, carrier_hz: f64) -> RealDecimator {
    let high = (carrier_hz - AUDIO_EDGE_HZ).max(2.0 * AUDIO_EDGE_HZ);
    RealDecimator::new(
        &design_bandpass(BAND_TAPS, AUDIO_EDGE_HZ / rate, high / rate),
        1,
    )
}

pub(crate) struct VoiceInverter {
    rate: f64,
    carrier_hz: f64,
    band: RealDecimator,
    delay: Delay,
    hilbert: RealDecimator,
    nco: Nco,
    cut: Vec<f32>,
    inphase: Vec<f32>,
    quadrature: Vec<f32>,
}

impl VoiceInverter {
    pub(crate) fn new(rate: f64, carrier_hz: f64) -> Self {
        let carrier_hz = carrier_hz.clamp(MIN_INVERSION_HZ, MAX_INVERSION_HZ);
        let taps = design_hilbert(HILBERT_TAPS);
        Self {
            rate,
            carrier_hz,
            band: band_filter(rate, carrier_hz),
            delay: Delay::new(taps.len() / 2),
            hilbert: RealDecimator::new(&taps, 1),
            nco: Nco::new(carrier_hz as f32, rate as f32),
            cut: Vec::new(),
            inphase: Vec::new(),
            quadrature: Vec::new(),
        }
    }

    pub(crate) fn carrier_hz(&self) -> f64 {
        self.carrier_hz
    }

    pub(crate) fn set_carrier(&mut self, carrier_hz: f64) {
        let carrier_hz = carrier_hz.clamp(MIN_INVERSION_HZ, MAX_INVERSION_HZ);
        if (carrier_hz - self.carrier_hz).abs() < 0.5 {
            return;
        }
        self.carrier_hz = carrier_hz;
        self.band = band_filter(self.rate, carrier_hz);
        self.nco.set_freq(carrier_hz as f32, self.rate as f32);
    }

    pub(crate) fn reset(&mut self) {
        self.delay.reset();
        self.band = band_filter(self.rate, self.carrier_hz);
        self.hilbert = RealDecimator::new(&design_hilbert(HILBERT_TAPS), 1);
    }

    pub(crate) fn process(&mut self, audio: &mut [f32]) {
        self.band.process(audio, &mut self.cut);
        self.delay.process(&self.cut, &mut self.inphase);
        self.hilbert.process(&self.cut, &mut self.quadrature);
        let nco = &mut self.nco;
        for ((slot, &i), &q) in audio
            .iter_mut()
            .zip(&self.inphase)
            .zip(self.quadrature.iter())
        {
            let carrier = nco.next_sample();
            *slot = i.mul_add(carrier.re, q * carrier.im);
        }
    }
}

const DETECT_DECIM: usize = 6;
const DETECT_TAPS: usize = 63;
const DETECT_CUTOFF_HZ: f64 = 3_600.0;
const FRAME: usize = 512;
const BAND_LOW_HZ: f64 = 300.0;
const BAND_HIGH_HZ: f64 = 3_800.0;
const SWEEP_MIN_HZ: f64 = 2_200.0;
const SWEEP_MAX_HZ: f64 = 4_000.0;
const SWEEP_STEP_HZ: f64 = 25.0;
const DECISION_FRAMES: u32 = 16;
const ACTIVE_RMS: f32 = 0.02;
const MIN_SCORE: f32 = 0.6;
const CLEAR_MARGIN: f32 = 0.12;
const VOICE_SPREAD_DB: f32 = 1.5;
const LOCK_TOLERANCE_HZ: f64 = 75.0;
const LOCK_AGREEMENTS: u32 = 2;
const CLEAR_DECISIONS: u32 = 2;

const SPEECH_FLOOR_DB: f32 = -45.0;
const SPEECH_TEMPLATE: [(f64, f32); 16] = [
    (0.0, -45.0),
    (150.0, -28.0),
    (250.0, -10.0),
    (315.0, -2.0),
    (400.0, 0.0),
    (500.0, -1.0),
    (630.0, -3.0),
    (800.0, -6.0),
    (1_000.0, -8.0),
    (1_250.0, -10.0),
    (1_600.0, -12.0),
    (2_000.0, -14.0),
    (2_500.0, -17.0),
    (3_150.0, -20.0),
    (3_400.0, -32.0),
    (4_000.0, -45.0),
];

fn template_db(hz: f64) -> f32 {
    let last = SPEECH_TEMPLATE.len() - 1;
    if hz <= SPEECH_TEMPLATE[0].0 || hz >= SPEECH_TEMPLATE[last].0 {
        return SPEECH_FLOOR_DB;
    }
    let upper = SPEECH_TEMPLATE
        .iter()
        .position(|&(f, _)| f >= hz)
        .unwrap_or(last);
    let (f0, v0) = SPEECH_TEMPLATE[upper - 1];
    let (f1, v1) = SPEECH_TEMPLATE[upper];
    let t = ((hz - f0) / (f1 - f0)) as f32;
    v0 + t * (v1 - v0)
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    if n < 2.0 {
        return 0.0;
    }
    let mean_a = a.iter().sum::<f32>() / n;
    let mean_b = b.iter().sum::<f32>() / n;
    let (mut cross, mut var_a, mut var_b) = (0.0f32, 0.0f32, 0.0f32);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (x - mean_a, y - mean_b);
        cross += x * y;
        var_a += x * x;
        var_b += y * y;
    }
    let scale = (var_a * var_b).sqrt();
    if scale > f32::EPSILON {
        cross / scale
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Inversion {
    pub(crate) carrier_hz: Option<f64>,
    pub(crate) confidence: f32,
}

pub(crate) struct InversionDetector {
    bin_hz: f64,
    low_bin: usize,
    highpass: Highpass,
    decim: RealDecimator,
    filtered: Vec<f32>,
    decimated: Vec<f32>,
    frame: Vec<Complex<f32>>,
    fft: SpectrumAnalyzer,
    spectrum: Vec<f32>,
    average: Vec<f32>,
    frames: u32,
    levels: Vec<f32>,
    observed: Vec<f32>,
    model: Vec<f32>,
    pending: Option<f64>,
    agreements: u32,
    misses: u32,
    inversion: Inversion,
}

impl InversionDetector {
    pub(crate) fn new(rate: f64) -> Self {
        let detect_rate = rate / DETECT_DECIM as f64;
        let bin_hz = detect_rate / FRAME as f64;
        let low_bin = (BAND_LOW_HZ / bin_hz).ceil() as usize;
        let high_bin = (BAND_HIGH_HZ / bin_hz).floor() as usize;
        let bins = high_bin - low_bin + 1;
        Self {
            bin_hz,
            low_bin,
            highpass: Highpass::new(rate, AUDIO_EDGE_HZ),
            decim: RealDecimator::new(
                &design_lowpass(DETECT_TAPS, DETECT_CUTOFF_HZ / rate),
                DETECT_DECIM,
            ),
            filtered: Vec::new(),
            decimated: Vec::new(),
            frame: Vec::with_capacity(FRAME),
            fft: SpectrumAnalyzer::new(FRAME),
            spectrum: vec![0.0; FRAME],
            average: vec![0.0; bins],
            frames: 0,
            levels: Vec::with_capacity(DECISION_FRAMES as usize),
            observed: vec![0.0; bins],
            model: vec![0.0; bins],
            pending: None,
            agreements: 0,
            misses: 0,
            inversion: Inversion::default(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.highpass.reset();
        self.frame.clear();
        self.average.fill(0.0);
        self.levels.clear();
        self.frames = 0;
        self.pending = None;
        self.agreements = 0;
        self.misses = 0;
        self.inversion = Inversion::default();
    }

    pub(crate) fn process(&mut self, audio: &[f32]) -> Inversion {
        self.filtered.clear();
        self.filtered.extend_from_slice(audio);
        self.highpass.process(&mut self.filtered);
        self.decim.process(&self.filtered, &mut self.decimated);
        for index in 0..self.decimated.len() {
            self.frame.push(Complex::new(self.decimated[index], 0.0));
            if self.frame.len() == FRAME {
                self.accumulate();
            }
        }
        self.inversion
    }

    fn accumulate(&mut self) {
        let power = self.frame.iter().map(|s| s.re * s.re).sum::<f32>() / FRAME as f32;
        if power.sqrt() < ACTIVE_RMS {
            self.frame.clear();
            return;
        }
        self.fft.power_db(&self.frame, &mut self.spectrum);
        self.frame.clear();
        let centre = FRAME / 2;
        for (offset, slot) in self.average.iter_mut().enumerate() {
            *slot += self.spectrum[centre + self.low_bin + offset];
        }
        self.levels.push(10.0 * power.log10());
        self.frames += 1;
        if self.frames >= DECISION_FRAMES {
            self.decide();
        }
    }

    fn decide(&mut self) {
        let scale = self.frames as f32;
        self.frames = 0;
        let speaking = self.spread_db() >= VOICE_SPREAD_DB;
        self.levels.clear();
        if !speaking {
            self.average.fill(0.0);
            return;
        }
        for (slot, &sum) in self.observed.iter_mut().zip(self.average.iter()) {
            *slot = sum / scale;
        }
        self.average.fill(0.0);

        let clear = self.score(None);
        let mut best = (f32::MIN, SWEEP_MIN_HZ);
        let mut carrier = SWEEP_MIN_HZ;
        while carrier <= SWEEP_MAX_HZ {
            let score = self.score(Some(carrier));
            if score > best.0 {
                best = (score, carrier);
            }
            carrier += SWEEP_STEP_HZ;
        }
        let detected = (best.0 >= MIN_SCORE && best.0 > clear + CLEAR_MARGIN).then_some(best.1);
        self.settle(detected, best.0);
    }

    fn spread_db(&self) -> f32 {
        let n = self.levels.len() as f32;
        if n < 2.0 {
            return 0.0;
        }
        let mean = self.levels.iter().sum::<f32>() / n;
        (self
            .levels
            .iter()
            .map(|v| (v - mean) * (v - mean))
            .sum::<f32>()
            / n)
            .sqrt()
    }

    fn score(&mut self, carrier_hz: Option<f64>) -> f32 {
        for (offset, slot) in self.model.iter_mut().enumerate() {
            let hz = (self.low_bin + offset) as f64 * self.bin_hz;
            *slot = template_db(match carrier_hz {
                Some(carrier) => carrier - hz,
                None => hz,
            });
        }
        correlation(&self.observed, &self.model)
    }

    fn held(&self, estimate: f64) -> f64 {
        let snapped = (estimate / SWEEP_STEP_HZ).round() * SWEEP_STEP_HZ;
        match self.inversion.carrier_hz {
            Some(held) if (held - snapped).abs() <= SWEEP_STEP_HZ => held,
            _ => snapped,
        }
    }

    fn settle(&mut self, detected: Option<f64>, score: f32) {
        match detected {
            Some(hz) => {
                self.misses = 0;
                match self.pending {
                    Some(pending) if (pending - hz).abs() <= LOCK_TOLERANCE_HZ => {
                        self.agreements += 1;
                        self.pending = Some(0.5 * (pending + hz));
                    }
                    _ => {
                        self.pending = Some(hz);
                        self.agreements = 1;
                    }
                }
                if self.agreements >= LOCK_AGREEMENTS {
                    self.inversion = Inversion {
                        carrier_hz: self.pending.map(|hz| self.held(hz)),
                        confidence: score,
                    };
                }
            }
            None => {
                self.pending = None;
                self.agreements = 0;
                self.misses += 1;
                if self.misses >= CLEAR_DECISIONS {
                    self.inversion = Inversion::default();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        testgen::{nfm::speech_audio, tone_audio},
        testutil::{dominant_tone, tone_amplitude},
    };

    const RATE: f64 = 48_000.0;
    const CARRIER_HZ: f64 = 3_300.0;

    fn inverted(audio: &[f32], carrier_hz: f64) -> Vec<f32> {
        let mut out = audio.to_vec();
        VoiceInverter::new(RATE, carrier_hz).process(&mut out);
        out
    }

    #[test]
    fn a_tone_is_reflected_about_the_carrier() {
        let audio = tone_audio(1_000.0, 0.5, RATE, 24_000);
        let scrambled = inverted(&audio, CARRIER_HZ);
        let (freq, ratio) = dominant_tone(&scrambled[4_000..20_000], RATE);
        assert!((freq - 2_300.0).abs() < 20.0, "reflected tone at {freq} Hz");
        assert!(ratio > 0.8, "reflected tone holds {ratio} of the energy");
    }

    #[test]
    fn inverting_twice_restores_every_tone() {
        let voices = [500.0, 1_500.0, 2_500.0];
        let mut audio = vec![0.0; 24_000];
        for &hz in &voices {
            for (slot, sample) in audio.iter_mut().zip(tone_audio(hz, 0.3, RATE, 24_000)) {
                *slot += sample;
            }
        }
        let restored = inverted(&inverted(&audio, CARRIER_HZ), CARRIER_HZ);
        let window = &restored[6_000..22_000];
        for &hz in &voices {
            let level = tone_amplitude(window, hz, RATE);
            let mirrored = tone_amplitude(window, CARRIER_HZ - hz, RATE);
            assert!((level - 0.3).abs() < 0.05, "{hz} Hz came back at {level}");
            assert!(mirrored < 0.02, "{hz} Hz left {mirrored} at its mirror");
        }
    }

    #[test]
    fn the_detector_locks_onto_the_carrier_it_was_scrambled_with() {
        for carrier_hz in [2_632.0, 3_000.0, 3_300.0, 3_700.0] {
            let speech = speech_audio(RATE, 6 * RATE as usize);
            let scrambled = inverted(&speech, carrier_hz);
            let mut detector = InversionDetector::new(RATE);
            let mut found = Inversion::default();
            for block in scrambled.chunks(1_024) {
                found = detector.process(block);
            }
            let estimate = found.carrier_hz.unwrap_or_default();
            assert!(
                (estimate - carrier_hz).abs() <= LOCK_TOLERANCE_HZ,
                "scrambled at {carrier_hz} Hz, detected {estimate} Hz"
            );
        }
    }

    #[test]
    fn a_locked_carrier_holds_still_between_decisions() {
        let scrambled = inverted(&speech_audio(RATE, 15 * RATE as usize), 3_012.0);
        let mut detector = InversionDetector::new(RATE);
        let mut changes: Vec<Option<f64>> = vec![None];
        for block in scrambled.chunks(1_024) {
            let found = detector.process(block);
            if *changes.last().unwrap() != found.carrier_hz {
                changes.push(found.carrier_hz);
            }
        }
        assert_eq!(changes.len(), 2, "the carrier kept moving: {changes:?}");
    }

    #[test]
    fn the_detector_leaves_unscrambled_speech_alone() {
        let speech = speech_audio(RATE, 6 * RATE as usize);
        let mut detector = InversionDetector::new(RATE);
        let mut found = Inversion::default();
        for block in speech.chunks(1_024) {
            found = detector.process(block);
        }
        assert_eq!(found.carrier_hz, None, "clear speech was called scrambled");
    }

    #[test]
    fn silence_never_produces_an_estimate() {
        let mut detector = InversionDetector::new(RATE);
        let mut found = Inversion::default();
        for block in vec![0.0f32; 4 * RATE as usize].chunks(1_024) {
            found = detector.process(block);
        }
        assert_eq!(found.carrier_hz, None);
    }
    #[test]
    fn a_subaudible_tone_never_reaches_the_voice_band() {
        let audio = tone_audio(100.0, 0.5, RATE, 24_000);
        let scrambled = inverted(&audio, CARRIER_HZ);
        let level = tone_amplitude(&scrambled[6_000..22_000], CARRIER_HZ - 100.0, RATE);
        assert!(level < 0.005, "a 100 Hz tone reappeared at {level}");
    }
}
