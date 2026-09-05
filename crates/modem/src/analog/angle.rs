use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{DcBlocker, FmDemod, Pll, RealDecimator, design_lowpass};

use super::filter::{Band, BandFilter};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AngleKind {
    Fm { deviation: f64 },
    Pm { deviation_rad: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleParams {
    pub kind: AngleKind,
    pub bandwidth: f64,
    pub band_taps: usize,
    pub audio_taps: usize,
}

impl AngleParams {
    #[must_use]
    pub fn new(kind: AngleKind, bandwidth: f64) -> Self {
        Self {
            kind,
            bandwidth,
            band_taps: 129,
            audio_taps: 129,
        }
    }

    #[must_use]
    pub fn deviation_ratio(&self) -> f64 {
        match self.kind {
            AngleKind::Fm { deviation } => deviation / self.bandwidth,
            AngleKind::Pm { deviation_rad } => deviation_rad,
        }
    }

    #[must_use]
    pub fn carson_bandwidth(&self) -> f64 {
        2.0 * (self.deviation_ratio() + 1.0) * self.bandwidth
    }

    #[must_use]
    pub fn band(&self) -> BandFilter {
        BandFilter::symmetric(0.5 * self.carson_bandwidth(), self.band_taps)
    }
}

pub struct AngleMod {
    kind: AngleKind,
    message: RealDecimator,
    phase: f64,
    limited: Vec<f32>,
}

impl AngleMod {
    #[must_use]
    pub fn new(params: &AngleParams) -> Self {
        Self {
            kind: params.kind,
            message: RealDecimator::new(&design_lowpass(params.audio_taps, params.bandwidth), 1),
            phase: 0.0,
            limited: Vec::new(),
        }
    }

    pub fn process(&mut self, audio: &[f32], out: &mut Vec<Complex<f32>>) {
        self.message.process(audio, &mut self.limited);
        out.clear();
        for &a in &self.limited {
            let phase = match self.kind {
                AngleKind::Fm { deviation } => {
                    self.phase = (self.phase + TAU * deviation * f64::from(a)).rem_euclid(TAU);
                    self.phase
                }
                AngleKind::Pm { deviation_rad } => deviation_rad * f64::from(a),
            };
            let z = Complex::from_polar(1.0, phase);
            out.push(Complex::new(z.re as f32, z.im as f32));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AngleDetector {
    Discriminator,
    Pll { loop_bw: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleRx {
    pub detector: AngleDetector,
    pub predetection: bool,
    pub audio_filter: bool,
    pub dc_block: bool,
}

impl AngleRx {
    #[must_use]
    pub fn new(detector: AngleDetector) -> Self {
        Self {
            detector,
            predetection: true,
            audio_filter: true,
            dc_block: false,
        }
    }

    #[must_use]
    pub fn detector_only(detector: AngleDetector) -> Self {
        Self {
            detector,
            predetection: false,
            audio_filter: false,
            dc_block: false,
        }
    }
}

enum Reader {
    Differential(FmDemod),
    Argument { scale: f32 },
    LoopFrequency { pll: Pll, scale: f64 },
    LoopArgument { pll: Pll, scale: f32 },
}

pub struct AngleDemod {
    band: Option<Band>,
    reader: Reader,
    dc: Option<DcBlocker>,
    audio: Option<RealDecimator>,
    filtered: Vec<Complex<f32>>,
    detected: Vec<f32>,
}

impl AngleDemod {
    const PULL_IN: f64 = 1.5;

    #[must_use]
    pub fn new(params: &AngleParams, rx: &AngleRx) -> Self {
        let reader = match (rx.detector, params.kind) {
            (AngleDetector::Discriminator, AngleKind::Fm { deviation }) => {
                Reader::Differential(FmDemod::new(1.0, deviation))
            }
            (AngleDetector::Discriminator, AngleKind::Pm { deviation_rad }) => Reader::Argument {
                scale: (1.0 / deviation_rad) as f32,
            },
            (AngleDetector::Pll { loop_bw }, AngleKind::Fm { deviation }) => {
                Reader::LoopFrequency {
                    pll: Pll::new(loop_bw, 0.707, 0.0, deviation * Self::PULL_IN),
                    scale: 1.0 / deviation,
                }
            }
            (AngleDetector::Pll { loop_bw }, AngleKind::Pm { deviation_rad }) => {
                Reader::LoopArgument {
                    pll: Pll::new(loop_bw, 0.707, 0.0, loop_bw * 10.0),
                    scale: (1.0 / deviation_rad) as f32,
                }
            }
        };
        Self {
            band: rx.predetection.then(|| params.band().build()),
            reader,
            dc: rx.dc_block.then(DcBlocker::new),
            audio: rx.audio_filter.then(|| {
                RealDecimator::new(&design_lowpass(params.audio_taps, params.bandwidth), 1)
            }),
            filtered: Vec::new(),
            detected: Vec::new(),
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        match self.band.as_mut() {
            Some(band) => band.process(iq, &mut self.filtered),
            None => {
                self.filtered.clear();
                self.filtered.extend_from_slice(iq);
            }
        }
        self.detected.clear();
        match &mut self.reader {
            Reader::Differential(fm) => fm.process(&self.filtered, &mut self.detected),
            Reader::Argument { scale } => self
                .detected
                .extend(self.filtered.iter().map(|z| z.arg() * *scale)),
            Reader::LoopFrequency { pll, scale } => {
                self.detected.extend(self.filtered.iter().map(|&z| {
                    let _ = pll.process(z);
                    (pll.increment_norm() * *scale) as f32
                }));
            }
            Reader::LoopArgument { pll, scale } => {
                self.detected.extend(self.filtered.iter().map(|&z| {
                    let reference = pll.process(z);
                    (z * reference.conj()).arg() * *scale
                }));
            }
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
        match &self.reader {
            Reader::Differential(_) | Reader::Argument { .. } => 1.0,
            Reader::LoopFrequency { pll, .. } | Reader::LoopArgument { pll, .. } => pll.lock(),
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::analog::{analyse_tone, tone};

    use super::*;

    const RATE: f64 = 48_000.0;
    const BANDWIDTH: f64 = 3_000.0 / RATE;
    const TONE: f64 = 1_000.0 / RATE;
    const WINDOW: std::ops::Range<usize> = 4_096..36_832;

    fn narrowband() -> AngleParams {
        AngleParams::new(
            AngleKind::Fm {
                deviation: 2_500.0 / RATE,
            },
            BANDWIDTH,
        )
    }

    fn phase() -> AngleParams {
        AngleParams::new(AngleKind::Pm { deviation_rad: 1.0 }, BANDWIDTH)
    }

    fn modulated(params: &AngleParams, amplitude: f32, samples: usize) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        AngleMod::new(params).process(&tone(TONE, amplitude, samples), &mut out);
        out
    }

    fn demodulated(params: &AngleParams, rx: &AngleRx, iq: &[Complex<f32>]) -> Vec<f32> {
        let mut out = Vec::new();
        AngleDemod::new(params, rx).process(iq, &mut out);
        out
    }

    #[test]
    fn the_transmitter_keeps_a_constant_envelope_at_its_stated_deviation() {
        let params = narrowband();
        let iq = modulated(&params, 1.0, 16_384);
        for (k, z) in iq.iter().enumerate().skip(512) {
            assert!(
                (z.norm() - 1.0).abs() < 1e-5,
                "envelope at {k}: {}",
                z.norm()
            );
        }
        let excursion = iq[512..]
            .windows(2)
            .map(|w| f64::from((w[1] * w[0].conj()).arg()) / TAU)
            .fold(0.0f64, |acc, f| acc.max(f.abs()));
        assert!(
            (excursion - 2_500.0 / RATE).abs() < 2e-5,
            "peak deviation {excursion} cycles/sample"
        );
    }

    #[test]
    fn both_fm_tiers_recover_the_message() {
        let params = narrowband();
        let iq = modulated(&params, 0.8, 49_152);
        for detector in [
            AngleDetector::Discriminator,
            AngleDetector::Pll {
                loop_bw: 2.0 * BANDWIDTH,
            },
        ] {
            let audio = demodulated(&params, &AngleRx::new(detector), &iq);
            let analysis = analyse_tone(&audio[WINDOW], TONE);
            assert!(
                (0.5..1.15).contains(&analysis.amplitude),
                "{detector:?}: amplitude {}",
                analysis.amplitude
            );
            assert!(
                analysis.thd() < 0.02,
                "{detector:?}: thd {}",
                analysis.thd()
            );
            assert!(
                analysis.sinad_db() > 35.0,
                "{detector:?}: sinad {}",
                analysis.sinad_db()
            );
        }
    }

    #[test]
    fn both_pm_tiers_recover_the_message() {
        let params = phase();
        let iq = modulated(&params, 0.8, 49_152);
        for detector in [
            AngleDetector::Discriminator,
            AngleDetector::Pll {
                loop_bw: 0.05 * BANDWIDTH,
            },
        ] {
            let audio = demodulated(&params, &AngleRx::new(detector), &iq);
            let analysis = analyse_tone(&audio[WINDOW], TONE);
            assert!(
                (analysis.amplitude - 0.8).abs() < 0.03,
                "{detector:?}: amplitude {}",
                analysis.amplitude
            );
            assert!(
                analysis.thd() < 0.02,
                "{detector:?}: thd {}",
                analysis.thd()
            );
        }
    }

    #[test]
    fn the_pm_loop_removes_a_carrier_offset_the_bare_argument_cannot() {
        let params = phase();
        let mut iq = modulated(&params, 0.5, 49_152);
        let offset = 20.0 / RATE;
        for (n, z) in iq.iter_mut().enumerate() {
            *z *= Complex::from_polar(1.0, (TAU * offset * n as f64) as f32);
        }
        let bare = demodulated(&params, &AngleRx::new(AngleDetector::Discriminator), &iq);
        let tracked = demodulated(
            &params,
            &AngleRx::new(AngleDetector::Pll {
                loop_bw: 0.05 * BANDWIDTH,
            }),
            &iq,
        );
        let bare = analyse_tone(&bare[WINDOW], TONE);
        let tracked = analyse_tone(&tracked[WINDOW], TONE);
        assert!(
            bare.sinad_db() < 6.0,
            "bare argument {} dB",
            bare.sinad_db()
        );
        assert!(
            tracked.sinad_db() > 30.0,
            "loop-referenced {} dB",
            tracked.sinad_db()
        );
    }

    #[test]
    fn carson_bandwidth_follows_the_deviation() {
        let narrow = narrowband();
        assert!((narrow.deviation_ratio() - 2_500.0 / 3_000.0).abs() < 1e-12);
        assert!((narrow.carson_bandwidth() * RATE - 11_000.0).abs() < 1e-6);
        let wide = AngleParams::new(
            AngleKind::Fm {
                deviation: 75_000.0 / 240_000.0,
            },
            15_000.0 / 240_000.0,
        );
        assert!((wide.deviation_ratio() - 5.0).abs() < 1e-12);
        assert!((wide.carson_bandwidth() * 240_000.0 - 180_000.0).abs() < 1e-6);
        assert!((wide.band().high * 240_000.0 - 90_000.0).abs() < 1e-6);
    }
}
