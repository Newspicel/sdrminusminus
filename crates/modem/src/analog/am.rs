use num_complex::Complex;
use sdrmm_dsp::{Costas, DcBlocker, FirC, Pll, RealDecimator, design_lowpass};

use super::filter::{Band, BandFilter, design_vestigial};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmMode {
    FullCarrier { depth: f64 },
    Suppressed,
}

impl AmMode {
    #[must_use]
    pub fn depth(self) -> f64 {
        match self {
            Self::FullCarrier { depth } => depth,
            Self::Suppressed => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmParams {
    pub mode: AmMode,
    pub bandwidth: f64,
    pub vestige: Option<f64>,
    pub band_taps: usize,
    pub audio_taps: usize,
}

impl AmParams {
    #[must_use]
    pub fn new(mode: AmMode, bandwidth: f64) -> Self {
        Self {
            mode,
            bandwidth,
            vestige: None,
            band_taps: 129,
            audio_taps: 129,
        }
    }

    #[must_use]
    pub fn band(&self) -> BandFilter {
        match self.vestige {
            None => BandFilter::symmetric(self.bandwidth, self.band_taps),
            Some(vestige) => BandFilter {
                low: -vestige,
                high: self.bandwidth,
                taps: self.band_taps,
            },
        }
    }
}

pub struct AmMod {
    mode: AmMode,
    message: RealDecimator,
    vestigial: Option<FirC>,
    limited: Vec<f32>,
    shaped: Vec<Complex<f32>>,
}

impl AmMod {
    #[must_use]
    pub fn new(params: &AmParams) -> Self {
        Self {
            mode: params.mode,
            message: RealDecimator::new(&design_lowpass(params.audio_taps, params.bandwidth), 1),
            vestigial: params.vestige.map(|vestige| {
                FirC::new(&design_vestigial(
                    params.band_taps,
                    vestige,
                    params.bandwidth,
                ))
            }),
            limited: Vec::new(),
            shaped: Vec::new(),
        }
    }

    pub fn process(&mut self, audio: &[f32], out: &mut Vec<Complex<f32>>) {
        self.message.process(audio, &mut self.limited);
        let depth = self.mode.depth() as f32;
        let carrier = f32::from(u8::from(matches!(self.mode, AmMode::FullCarrier { .. })));
        out.clear();
        out.extend(
            self.limited
                .iter()
                .map(|&a| Complex::new(carrier + depth * a, 0.0)),
        );
        if let Some(filter) = self.vestigial.as_mut() {
            filter.process(out, &mut self.shaped);
            std::mem::swap(out, &mut self.shaped);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmDetector {
    Envelope,
    Synchronous { loop_bw: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmRx {
    pub detector: AmDetector,
    pub predetection: bool,
    pub audio_filter: bool,
    pub dc_block: bool,
}

impl AmRx {
    #[must_use]
    pub fn new(detector: AmDetector) -> Self {
        Self {
            detector,
            predetection: true,
            audio_filter: true,
            dc_block: true,
        }
    }

    #[must_use]
    pub fn detector_only(detector: AmDetector) -> Self {
        Self {
            detector,
            predetection: false,
            audio_filter: false,
            dc_block: false,
        }
    }
}

enum Carrier {
    Envelope,
    Pll(Pll),
    Costas(Costas),
}

pub struct AmDemod {
    band: Option<Band>,
    carrier: Carrier,
    dc: Option<DcBlocker>,
    audio: Option<RealDecimator>,
    filtered: Vec<Complex<f32>>,
    detected: Vec<f32>,
}

impl AmDemod {
    const PULL_IN: f64 = 10.0;

    #[must_use]
    pub fn new(params: &AmParams, rx: &AmRx) -> Self {
        let carrier = match (rx.detector, params.mode) {
            (AmDetector::Envelope, _) => Carrier::Envelope,
            (AmDetector::Synchronous { loop_bw }, AmMode::FullCarrier { .. }) => {
                Carrier::Pll(Pll::new(loop_bw, 0.707, 0.0, loop_bw * Self::PULL_IN))
            }
            (AmDetector::Synchronous { loop_bw }, AmMode::Suppressed) => {
                Carrier::Costas(Costas::new(loop_bw, 0.707, 0.0, loop_bw * Self::PULL_IN))
            }
        };
        Self {
            band: rx.predetection.then(|| params.band().build()),
            carrier,
            dc: rx.dc_block.then(DcBlocker::new),
            audio: rx.audio_filter.then(|| {
                RealDecimator::new(&design_lowpass(params.audio_taps, params.bandwidth), 1)
            }),
            filtered: Vec::new(),
            detected: Vec::new(),
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        let input = match self.band.as_mut() {
            Some(band) => {
                band.process(iq, &mut self.filtered);
                &self.filtered
            }
            None => {
                self.filtered.clear();
                self.filtered.extend_from_slice(iq);
                &self.filtered
            }
        };
        self.detected.clear();
        match &mut self.carrier {
            Carrier::Envelope => self.detected.extend(input.iter().map(|z| z.norm())),
            Carrier::Pll(pll) => self.detected.extend(input.iter().map(|&x| {
                let reference = pll.process(x);
                (x * reference.conj()).re
            })),
            Carrier::Costas(costas) => self
                .detected
                .extend(input.iter().map(|&x| costas.process(x).re)),
        }
        if let Some(dc) = self.dc.as_mut() {
            dc.process(&mut self.detected);
        }
        match self.audio.as_mut() {
            Some(audio) => audio.process(&self.detected, out),
            None => {
                out.clear();
                out.extend_from_slice(&self.detected);
            }
        }
    }

    #[must_use]
    pub fn lock(&self) -> f32 {
        match &self.carrier {
            Carrier::Envelope => 1.0,
            Carrier::Pll(pll) => pll.lock(),
            Carrier::Costas(costas) => costas.lock(),
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::analog::{analyse_tone, tone};

    use super::*;

    const RATE: f64 = 48_000.0;
    const BANDWIDTH: f64 = 4_000.0 / RATE;
    const TONE: f64 = 1_000.0 / RATE;

    fn tone_amplitude(audio: &[f32], freq: f64) -> f64 {
        analyse_tone(audio, freq).amplitude
    }

    fn harmonic_ratio(audio: &[f32], freq: f64) -> f64 {
        analyse_tone(audio, freq).thd()
    }

    fn modulated(params: &AmParams, amplitude: f32, samples: usize) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        AmMod::new(params).process(&tone(TONE, amplitude, samples), &mut out);
        out
    }

    fn demodulated(params: &AmParams, rx: &AmRx, iq: &[Complex<f32>]) -> Vec<f32> {
        let mut out = Vec::new();
        AmDemod::new(params, rx).process(iq, &mut out);
        out
    }

    #[test]
    fn the_full_carrier_envelope_carries_its_stated_depth() {
        let params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        let iq = modulated(&params, 1.0, 16_384);
        let steady = &iq[512..];
        let peak = steady.iter().map(|z| z.norm()).fold(f32::MIN, f32::max);
        let trough = steady.iter().map(|z| z.norm()).fold(f32::MAX, f32::min);
        assert!((peak - 1.8).abs() < 0.02, "peak {peak}");
        assert!((trough - 0.2).abs() < 0.02, "trough {trough}");
        let depth = f64::from(peak - trough) / f64::from(peak + trough);
        assert!((depth - 0.8).abs() < 0.02, "measured depth {depth}");
    }

    #[test]
    fn the_envelope_tier_recovers_a_tone() {
        let params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        let iq = modulated(&params, 0.5, 32_768);
        let audio = demodulated(&params, &AmRx::new(AmDetector::Envelope), &iq);
        let window = &audio[4_096..28_672];
        let amplitude = tone_amplitude(window, TONE);
        assert!((amplitude - 0.4).abs() < 0.01, "amplitude {amplitude}");
        assert!(
            harmonic_ratio(window, TONE) < 0.01,
            "distortion {}",
            harmonic_ratio(window, TONE)
        );
    }

    #[test]
    fn the_synchronous_tier_recovers_the_same_tone() {
        let params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        let iq = modulated(&params, 0.5, 32_768);
        let rx = AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 });
        let audio = demodulated(&params, &rx, &iq);
        let window = &audio[8_192..28_736];
        let amplitude = tone_amplitude(window, TONE);
        assert!((amplitude - 0.4).abs() < 0.02, "amplitude {amplitude}");
        assert!(
            harmonic_ratio(window, TONE) < 0.02,
            "distortion {}",
            harmonic_ratio(window, TONE)
        );
    }

    #[test]
    fn the_carrier_loop_pulls_in_an_offset() {
        let params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        let mut iq = modulated(&params, 0.5, 65_536);
        let offset = 2e-4;
        for (n, s) in iq.iter_mut().enumerate() {
            *s *= Complex::from_polar(1.0, (2.0 * std::f64::consts::PI * offset * n as f64) as f32);
        }
        let rx = AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 });
        let mut demod = AmDemod::new(&params, &rx);
        let mut audio = Vec::new();
        demod.process(&iq, &mut audio);
        assert!(demod.lock() > 0.5, "lock {}", demod.lock());
        let amplitude = tone_amplitude(&audio[32_768..65_504], TONE);
        assert!((amplitude - 0.4).abs() < 0.03, "amplitude {amplitude}");
    }

    #[test]
    fn a_suppressed_carrier_defeats_the_envelope_tier() {
        let params = AmParams::new(AmMode::Suppressed, BANDWIDTH);
        let iq = modulated(&params, 0.8, 32_768);
        let coherent = demodulated(
            &params,
            &AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 }),
            &iq,
        );
        let window = &coherent[8_192..28_672];
        let amplitude = tone_amplitude(window, TONE);
        assert!(
            (amplitude - 0.8).abs() < 0.03,
            "coherent amplitude {amplitude}"
        );
        assert!(harmonic_ratio(window, TONE) < 0.02);

        let folded = demodulated(&params, &AmRx::new(AmDetector::Envelope), &iq);
        let window = &folded[8_192..28_672];
        let distortion = harmonic_ratio(window, TONE);
        assert!(distortion > 0.4, "rectified distortion {distortion}");
    }

    #[test]
    fn a_vestigial_sideband_detects_undistorted() {
        let mut params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        params.vestige = Some(500.0 / RATE);
        params.band_taps = 257;
        let iq = modulated(&params, 0.5, 65_536);
        let rx = AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 });
        let audio = demodulated(&params, &rx, &iq);
        let window = &audio[8_192..60_032];
        let amplitude = tone_amplitude(window, TONE);
        assert!((amplitude - 0.2).abs() < 0.02, "amplitude {amplitude}");
        assert!(
            harmonic_ratio(window, TONE) < 0.02,
            "distortion {}",
            harmonic_ratio(window, TONE)
        );
    }

    #[test]
    fn an_envelope_detector_pays_quadrature_distortion_on_a_vestigial_sideband() {
        let mut params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        params.vestige = Some(500.0 / RATE);
        params.band_taps = 257;
        let deep = modulated(&params, 0.5, 65_536);
        let audio = demodulated(&params, &AmRx::new(AmDetector::Envelope), &deep);
        let distortion = harmonic_ratio(&audio[8_192..60_032], TONE);
        assert!(
            (0.05..0.2).contains(&distortion),
            "distortion at depth 0.8: {distortion}"
        );
        params.mode = AmMode::FullCarrier { depth: 0.2 };
        let shallow = modulated(&params, 0.5, 65_536);
        let audio = demodulated(&params, &AmRx::new(AmDetector::Envelope), &shallow);
        let shallow = harmonic_ratio(&audio[8_192..60_032], TONE);
        let ratio = shallow / distortion;
        assert!((ratio - 0.25).abs() < 0.03, "distortion ratio {ratio}");
    }

    #[test]
    fn a_bare_receiver_passes_the_detector_output_through() {
        let params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        let iq = modulated(&params, 0.5, 4_096);
        let rx = AmRx {
            detector: AmDetector::Envelope,
            predetection: false,
            audio_filter: false,
            dc_block: false,
        };
        let audio = demodulated(&params, &rx, &iq);
        assert_eq!(audio.len(), iq.len());
        for (k, (&a, x)) in audio.iter().zip(&iq).enumerate() {
            assert!((a - x.norm()).abs() < 1e-6, "sample {k}");
        }
    }
}
