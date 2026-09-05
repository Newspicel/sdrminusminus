use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, RealDecimator, design_bandpass, design_lowpass};

use super::filter::{Band, BandFilter, Delay, design_hilbert};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sideband {
    Upper,
    Lower,
}

impl Sideband {
    #[must_use]
    pub fn sign(self) -> f64 {
        match self {
            Self::Upper => 1.0,
            Self::Lower => -1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsbMethod {
    Hilbert,
    Weaver,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsbParams {
    pub sideband: Sideband,
    pub method: SsbMethod,
    pub low_cut: f64,
    pub bandwidth: f64,
    pub band_taps: usize,
    pub audio_taps: usize,
}

impl SsbParams {
    #[must_use]
    pub fn new(sideband: Sideband, method: SsbMethod, bandwidth: f64) -> Self {
        Self {
            sideband,
            method,
            low_cut: 0.0,
            bandwidth,
            band_taps: 257,
            audio_taps: 257,
        }
    }

    #[must_use]
    pub fn band(&self) -> BandFilter {
        let (low, high) = (self.low_cut, self.bandwidth);
        match self.sideband {
            Sideband::Upper => BandFilter {
                low,
                high,
                taps: self.band_taps,
            },
            Sideband::Lower => BandFilter {
                low: -high,
                high: -low,
                taps: self.band_taps,
            },
        }
    }

    fn weaver(&self) -> (f64, f64) {
        (
            0.5 * (self.bandwidth + self.low_cut),
            0.5 * (self.bandwidth - self.low_cut),
        )
    }

    fn message_filter(&self) -> RealDecimator {
        let taps = if self.low_cut > 0.0 {
            design_bandpass(self.audio_taps, self.low_cut, self.bandwidth)
        } else {
            design_lowpass(self.audio_taps, self.bandwidth)
        };
        RealDecimator::new(&taps, 1)
    }
}

struct Mixer {
    step: f64,
    phase: f64,
}

impl Mixer {
    fn new(freq: f64) -> Self {
        Self {
            step: TAU * freq,
            phase: 0.0,
        }
    }

    fn next(&mut self) -> Complex<f32> {
        let phasor = Complex::from_polar(1.0, self.phase);
        self.phase = (self.phase + self.step).rem_euclid(TAU);
        Complex::new(phasor.re as f32, phasor.im as f32)
    }
}

enum Exciter {
    Hilbert {
        quadrature: RealDecimator,
        inphase: Delay,
        sign: f64,
        i: Vec<f32>,
        q: Vec<f32>,
    },
    Weaver {
        down: Mixer,
        up: Mixer,
        lowpass: Decimator,
        sign: f64,
        mixed: Vec<Complex<f32>>,
        cut: Vec<Complex<f32>>,
    },
}

pub struct SsbMod {
    message: RealDecimator,
    exciter: Exciter,
    limited: Vec<f32>,
}

impl SsbMod {
    #[must_use]
    pub fn new(params: &SsbParams) -> Self {
        let sign = params.sideband.sign();
        let exciter = match params.method {
            SsbMethod::Hilbert => {
                let taps = design_hilbert(params.band_taps | 1);
                Exciter::Hilbert {
                    inphase: Delay::new(taps.len() / 2),
                    quadrature: RealDecimator::new(&taps, 1),
                    sign,
                    i: Vec::new(),
                    q: Vec::new(),
                }
            }
            SsbMethod::Weaver => {
                let (centre, half) = params.weaver();
                Exciter::Weaver {
                    down: Mixer::new(-centre),
                    up: Mixer::new(sign * centre),
                    lowpass: Decimator::new(&design_lowpass(params.band_taps, half.max(1e-4)), 1),
                    sign,
                    mixed: Vec::new(),
                    cut: Vec::new(),
                }
            }
        };
        Self {
            message: params.message_filter(),
            exciter,
            limited: Vec::new(),
        }
    }

    pub fn process(&mut self, audio: &[f32], out: &mut Vec<Complex<f32>>) {
        self.message.process(audio, &mut self.limited);
        out.clear();
        match &mut self.exciter {
            Exciter::Hilbert {
                quadrature,
                inphase,
                sign,
                i,
                q,
            } => {
                inphase.process(&self.limited, i);
                quadrature.process(&self.limited, q);
                out.extend(
                    i.iter()
                        .zip(q.iter())
                        .map(|(&i, &q)| Complex::new(i, *sign as f32 * q)),
                );
            }
            Exciter::Weaver {
                down,
                up,
                lowpass,
                sign,
                mixed,
                cut,
            } => {
                mixed.clear();
                mixed.extend(self.limited.iter().map(|&a| down.next() * a));
                lowpass.process(mixed, cut);
                out.extend(cut.iter().map(|&z| {
                    let z = if *sign > 0.0 { z } else { z.conj() };
                    z * up.next() * 2.0
                }));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsbDetector {
    Filter,
    Weaver,
}

enum Detector {
    Filter {
        band: Band,
        filtered: Vec<Complex<f32>>,
    },
    Weaver {
        down: Mixer,
        up: Mixer,
        lowpass: Decimator,
        sign: f64,
        mixed: Vec<Complex<f32>>,
        cut: Vec<Complex<f32>>,
    },
}

pub struct SsbDemod {
    detector: Detector,
    audio: Option<RealDecimator>,
    detected: Vec<f32>,
}

impl SsbDemod {
    #[must_use]
    pub fn new(params: &SsbParams, detector: SsbDetector, audio_filter: bool) -> Self {
        let detector = match detector {
            SsbDetector::Filter => Detector::Filter {
                band: params.band().build(),
                filtered: Vec::new(),
            },
            SsbDetector::Weaver => {
                let (centre, half) = params.weaver();
                let sign = params.sideband.sign();
                Detector::Weaver {
                    down: Mixer::new(-sign * centre),
                    up: Mixer::new(centre),
                    lowpass: Decimator::new(&design_lowpass(params.band_taps, half.max(1e-4)), 1),
                    sign,
                    mixed: Vec::new(),
                    cut: Vec::new(),
                }
            }
        };
        Self {
            detector,
            audio: audio_filter.then(|| params.message_filter()),
            detected: Vec::new(),
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.detected.clear();
        match &mut self.detector {
            Detector::Filter { band, filtered } => {
                band.process(iq, filtered);
                self.detected.extend(filtered.iter().map(|z| z.re));
            }
            Detector::Weaver {
                down,
                up,
                lowpass,
                sign,
                mixed,
                cut,
            } => {
                mixed.clear();
                mixed.extend(iq.iter().map(|&z| z * down.next()));
                lowpass.process(mixed, cut);
                self.detected.extend(cut.iter().map(|&z| {
                    let z = if *sign > 0.0 { z } else { z.conj() };
                    (z * up.next()).re
                }));
            }
        }
        match self.audio.as_mut() {
            Some(audio) => audio.process(&self.detected, out),
            None => {
                out.clear();
                out.extend_from_slice(&self.detected);
            }
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
    const WINDOW: std::ops::Range<usize> = 8_192..40_928;

    fn params(sideband: Sideband, method: SsbMethod) -> SsbParams {
        SsbParams::new(sideband, method, BANDWIDTH)
    }

    fn excite(params: &SsbParams, samples: usize) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        SsbMod::new(params).process(&tone(TONE, 0.5, samples), &mut out);
        out
    }

    fn detect(params: &SsbParams, detector: SsbDetector, iq: &[Complex<f32>]) -> Vec<f32> {
        let mut out = Vec::new();
        SsbDemod::new(params, detector, true).process(iq, &mut out);
        out
    }

    fn sideband_rejection_db(iq: &[Complex<f32>], sideband: Sideband) -> f64 {
        let component = |f: f64| {
            iq.iter()
                .enumerate()
                .map(|(n, &z)| {
                    Complex::new(f64::from(z.re), f64::from(z.im))
                        * Complex::from_polar(1.0, -TAU * f * n as f64)
                })
                .sum::<Complex<f64>>()
                .norm_sqr()
        };
        let (up, down) = (component(TONE), component(-TONE));
        let (wanted, unwanted) = match sideband {
            Sideband::Upper => (up, down),
            Sideband::Lower => (down, up),
        };
        10.0 * (wanted / unwanted).log10()
    }

    #[test]
    fn both_methods_radiate_one_sideband() {
        for method in [SsbMethod::Hilbert, SsbMethod::Weaver] {
            for sideband in [Sideband::Upper, Sideband::Lower] {
                let iq = excite(&params(sideband, method), 16_384);
                let rejection = sideband_rejection_db(&iq[1_024..9_184], sideband);
                assert!(rejection > 40.0, "{method:?} {sideband:?}: {rejection} dB");
            }
        }
    }

    #[test]
    fn every_exciter_detector_pair_round_trips() {
        for method in [SsbMethod::Hilbert, SsbMethod::Weaver] {
            for detector in [SsbDetector::Filter, SsbDetector::Weaver] {
                for sideband in [Sideband::Upper, Sideband::Lower] {
                    let params = params(sideband, method);
                    let audio = detect(&params, detector, &excite(&params, 49_152));
                    let analysis = analyse_tone(&audio[WINDOW], TONE);
                    assert!(
                        (analysis.amplitude - 0.5).abs() < 0.02,
                        "{method:?}/{detector:?}/{sideband:?}: amplitude {}",
                        analysis.amplitude
                    );
                    assert!(
                        analysis.thd() < 0.01,
                        "{method:?}/{detector:?}/{sideband:?}: thd {}",
                        analysis.thd()
                    );
                }
            }
        }
    }

    #[test]
    fn a_receiver_on_the_wrong_sideband_hears_nothing() {
        let sent = params(Sideband::Upper, SsbMethod::Hilbert);
        let mut listening = sent;
        listening.sideband = Sideband::Lower;
        let iq = excite(&sent, 49_152);
        let right = analyse_tone(&detect(&sent, SsbDetector::Filter, &iq)[WINDOW], TONE);
        let wrong = analyse_tone(&detect(&listening, SsbDetector::Filter, &iq)[WINDOW], TONE);
        let rejection = 20.0 * (right.amplitude / wrong.amplitude).log10();
        assert!(
            rejection > 40.0,
            "opposite-sideband rejection {rejection} dB"
        );
    }
}
