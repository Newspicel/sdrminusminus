use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, RealDecimator, design_bandpass, design_lowpass};

use super::filter::{Band, BandFilter, Delay, design_hilbert};

/// Which side of the carrier carries the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sideband {
    Upper,
    Lower,
}

impl Sideband {
    /// +1 for the upper sideband, −1 for the lower — the sign every quadrature path and every
    /// band edge in this module is built from, so the two cases are one code path.
    #[must_use]
    pub fn sign(self) -> f64 {
        match self {
            Self::Upper => 1.0,
            Self::Lower => -1.0,
        }
    }
}

/// How the one-sided spectrum is built and read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsbMethod {
    /// Phasing: a wideband 90° network on the quadrature path.
    Hilbert,
    /// Weaver's third method: two mixers around a lowpass at half the bandwidth.
    Weaver,
}

/// The waveform as data. Frequencies are cycles per sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsbParams {
    pub sideband: Sideband,
    pub method: SsbMethod,
    /// Lower passband edge, measured from the carrier. Keeps a detector's own DC and the
    /// rumble under it out of the audio; zero is legal and is what a measurement entry uses,
    /// since the closed form it is held against assumes a message that starts at DC.
    pub low_cut: f64,
    /// Upper passband edge — the message bandwidth, and what the channel SNR is stated in.
    pub bandwidth: f64,
    pub band_taps: usize,
    pub audio_taps: usize,
}

impl SsbParams {
    /// A sideband at the crate's default filter lengths, cut at DC.
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

    /// The occupied band as the receiver's filter should be set — `[low_cut, bandwidth]` on
    /// whichever side of the carrier the sideband sits.
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

    /// Band centre and half-width — Weaver's two mixer frequencies, and the only place the
    /// method's geometry is written down.
    fn weaver(&self) -> (f64, f64) {
        (
            0.5 * (self.bandwidth + self.low_cut),
            0.5 * (self.bandwidth - self.low_cut),
        )
    }

    /// The message-limiting filter both exciters put in front of themselves: a lowpass when the
    /// message starts at DC, a bandpass when it does not.
    fn message_filter(&self) -> RealDecimator {
        let taps = if self.low_cut > 0.0 {
            design_bandpass(self.audio_taps, self.low_cut, self.bandwidth)
        } else {
            design_lowpass(self.audio_taps, self.bandwidth)
        };
        RealDecimator::new(&taps, 1)
    }
}

/// A complex exponential advanced sample by sample, with the phase kept in `f64` so a long run
/// cannot drift — the mixer both Weaver paths are built from.
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

/// The transmitter.
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

    /// Replaces `out` with one complex-baseband sample per input audio sample.
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

/// How the receiver keeps the unwanted sideband out of the product.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsbDetector {
    /// One complex band filter over the wanted side, then the real part — the receiver
    /// [`SsbMethod::Hilbert`] is deliberately paired with, since its rejection comes from a
    /// filter's stopband rather than from a second quadrature network.
    Filter,
    /// Weaver's method run backwards.
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

/// The receiver. Product detection either way — what differs is how the unwanted sideband is
/// kept out of the product.
pub struct SsbDemod {
    detector: Detector,
    audio: Option<RealDecimator>,
    detected: Vec<f32>,
}

impl SsbDemod {
    /// `detector` is the receiver's own choice, independent of how the waveform was built:
    /// running one method's exciter into the other's detector is the arrangement the module
    /// docs describe, and this signature is what makes it expressible.
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

    /// Replaces `out` with one audio sample per input sample.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.detected.clear();
        match &mut self.detector {
            Detector::Filter { band, filtered } => {
                band.process(iq, filtered);
                // The received sideband is already one-sided at full amplitude, so the real
                // part alone *is* the product-detector output — any extra gain would put a
                // strong signal past full scale.
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
    use super::*;
    use crate::ber::analog::{analyse_tone, tone};

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

    /// The waveform's defining property: one-sided. Measured at the tone's own frequency, on a
    /// whole number of its cycles, as the power at `+f` against the power at `−f` — the number
    /// a phasing exciter is judged by and the one a quadrature error spoils first.
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

    /// Both exciters build a one-sided spectrum, both sidebands, and the rejection is deep
    /// enough that nothing downstream can be measuring the wrong half.
    #[test]
    fn both_methods_radiate_one_sideband() {
        for method in [SsbMethod::Hilbert, SsbMethod::Weaver] {
            for sideband in [Sideband::Upper, Sideband::Lower] {
                let iq = excite(&params(sideband, method), 16_384);
                // 8160 samples is exactly 170 cycles of the 1 kHz tone at 48 kHz, so the two
                // components are read without either leaking into the other.
                let rejection = sideband_rejection_db(&iq[1_024..9_184], sideband);
                assert!(rejection > 40.0, "{method:?} {sideband:?}: {rejection} dB");
            }
        }
    }

    /// Every exciter into every detector, both sidebands: the tone comes back at its own
    /// amplitude and essentially undistorted. Eight combinations, because the point of having
    /// two methods is that neither can hide the other's error.
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
