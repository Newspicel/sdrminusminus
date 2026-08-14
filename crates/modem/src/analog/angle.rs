use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{DcBlocker, FmDemod, Pll, RealDecimator, design_lowpass};

use super::filter::{Band, BandFilter};

/// Which angle the message is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AngleKind {
    /// Frequency modulation: `deviation` is the peak frequency excursion in cycles per sample,
    /// reached at message ±1.
    Fm { deviation: f64 },
    /// Phase modulation: `deviation_rad` is the peak phase excursion in radians, reached at
    /// message ±1. Kept below π by any sensible modulator — past it the argument wraps and no
    /// detector without memory can unwrap it.
    Pm { deviation_rad: f64 },
}

/// The waveform as data. Frequencies are cycles per sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleParams {
    pub kind: AngleKind,
    /// Message bandwidth: what the transmitter limits the message to, what the receiver's
    /// audio filter cuts at, and what the channel SNR is stated in.
    pub bandwidth: f64,
    pub band_taps: usize,
    pub audio_taps: usize,
}

impl AngleParams {
    /// An entry at the crate's default filter lengths.
    #[must_use]
    pub fn new(kind: AngleKind, bandwidth: f64) -> Self {
        Self {
            kind,
            bandwidth,
            band_taps: 129,
            audio_taps: 129,
        }
    }

    /// The deviation ratio β — peak deviation over message bandwidth for FM, and the peak
    /// phase excursion in radians for PM, which is the same quantity: a phase modulator at
    /// `β_p` radians produces a peak frequency excursion of `β_p·f_m`, so at the top of the
    /// band the two definitions coincide.
    #[must_use]
    pub fn deviation_ratio(&self) -> f64 {
        match self.kind {
            AngleKind::Fm { deviation } => deviation / self.bandwidth,
            AngleKind::Pm { deviation_rad } => deviation_rad,
        }
    }

    /// Carson's rule: `2(Δf + W)` — the band that holds essentially all of an angle-modulated
    /// signal's power, and therefore the predetection filter's width. Not a convention picked
    /// for convenience: the noise this filter admits is what the threshold is a property of,
    /// so a wider one moves the entry's knee and a narrower one distorts the waveform.
    #[must_use]
    pub fn carson_bandwidth(&self) -> f64 {
        2.0 * (self.deviation_ratio() + 1.0) * self.bandwidth
    }

    /// The predetection band, symmetric about the carrier at Carson's half-width.
    #[must_use]
    pub fn band(&self) -> BandFilter {
        BandFilter::symmetric(0.5 * self.carson_bandwidth(), self.band_taps)
    }
}

/// The transmitter: band-limited message onto a constant-envelope argument.
pub struct AngleMod {
    kind: AngleKind,
    message: RealDecimator,
    /// Phase in radians, kept in `f64` and wrapped every sample: an `f32` accumulator drifts
    /// by more than the deviations being modelled inside a single sweep point.
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

    /// Replaces `out` with one complex-baseband sample per input audio sample.
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
    /// Memoryless: `arg(x[n]·conj(x[n−1]))` for FM, `arg(x[n])` for PM. Tier 1 everywhere,
    /// because it is what every consumer in the repo runs and what a threshold is measured
    /// against.
    Discriminator,
    /// A carrier loop: its instantaneous frequency estimate for FM, the argument of what it
    /// de-rotates for PM. Tier 2, and the whole of its value is below threshold.
    Pll { loop_bw: f64 },
}

/// What the receiver does around the detector — the same three optional stages
/// [`AmRx`](super::am::AmRx) carries, for the same reasons.
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

    /// The detector alone — see [`AmRx::detector_only`](super::am::AmRx::detector_only). Every
    /// angle-modulated channel in the repo wants this one: the host runtime filters, and what
    /// follows the discriminator is the channel's own (a tone-squelch highpass, a stereo
    /// demultiplexer, a sync separator).
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
    /// FM through the shared quadrature discriminator, which already scales ±deviation to ±1.
    Differential(FmDemod),
    /// PM read straight off the argument, scaled by the peak phase excursion.
    Argument { scale: f32 },
    /// FM through a loop: the control voltage, scaled the same way the discriminator is.
    LoopFrequency { pll: Pll, scale: f64 },
    /// PM through a loop: the argument of the de-rotated sample, so a carrier offset the bare
    /// argument would read as a ramp is removed instead.
    LoopArgument { pll: Pll, scale: f32 },
}

/// The receiver.
pub struct AngleDemod {
    band: Option<Band>,
    reader: Reader,
    dc: Option<DcBlocker>,
    audio: Option<RealDecimator>,
    filtered: Vec<Complex<f32>>,
    detected: Vec<f32>,
}

impl AngleDemod {
    /// Pull-in range of the FM loop, as a multiple of the peak deviation. The loop must reach
    /// every frequency the message sends it to and no further: an integrator clamped at the
    /// deviation itself would flat-top the message peaks, and one with no clamp integrates a
    /// noise burst off the signal and never returns.
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

    /// Replaces `out` with one audio sample per input sample.
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

    /// Loop lock quality in 0..=1, or 1 for the memoryless tier, which has nothing to lock.
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
    use super::*;
    use crate::ber::analog::{analyse_tone, tone};

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

    /// The waveform is constant-envelope and its peak excursion is the stated one — the
    /// transmitter's own calibration, read off the phasor rather than off a detector.
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

    /// Both FM tiers recover the message at unit amplitude and essentially no distortion —
    /// the "same number above threshold" half of the tier comparison, before any noise.
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
            // The loop's closed-loop response is a *gain*, not a distortion, and at a message
            // bandwidth this close to the loop's own natural frequency it is a large one — a
            // second-order loop peaks by up to 1.8 dB and this message sits inside the peak.
            // SINAD and THD are ratios and do not read it, so the entry's curves are
            // unaffected; the level is still bounded here so the tier cannot silently become a
            // different one. What the shaping *does* cost is measured, above threshold, as the
            // tier's own oracle gap: the same `|H|²` lifts the parabolic output noise at the
            // top of the band more than it lifts a tone below it.
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

    /// And both PM tiers, on the same message.
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

    /// A phase-modulated carrier arriving with a frequency offset is a phase *ramp*: the bare
    /// argument reads it as the message and wraps, and the loop tier is what removes it. This
    /// is the whole reason PM's second tier exists.
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

    /// Carson's rule as arithmetic: wideband FM occupies far more than its message and the
    /// predetection filter says so, which is what its threshold is a property of.
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
