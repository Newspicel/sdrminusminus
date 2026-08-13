//! Amplitude modulation: full-carrier AM, suppressed-carrier DSB, and vestigial sideband as a
//! *filter configuration of the same engine* rather than a third type (MODEM-PLAN §3.1
//! `analog/`, §6).
//!
//! One transmitter and one receiver cover the three, because they differ only in what the
//! carrier does and what the band filter keeps:
//!
//! | Mode | Baseband | Detectors |
//! |---|---|---|
//! | [`AmMode::FullCarrier`] | `1 + m·a(t)` | envelope, synchronous |
//! | [`AmMode::Suppressed`] | `a(t)` | synchronous only |
//! | vestigial ([`AmParams::vestige`]) | either, through [`design_vestigial`] | envelope, synchronous |
//!
//! **A suppressed carrier has no envelope to detect**, and that is a measurement rather than a
//! caveat: `a(t)` crosses zero, `|a(t)|` folds every negative excursion up, and the folded
//! output's second harmonic is the whole signal — the entry's own tests read it at ~50 % THD.
//! The receiver does not refuse the combination; it produces the rectified audio a real
//! envelope detector produces, and the catalog row records what that costs.
//!
//! **Envelope and synchronous detection are the same number above threshold.** Both recover
//! the message with the figure of merit [`am_fom`](crate::ber::theory::am_fom) states, and what
//! separates them is what happens *below* it: the envelope detector's nonlinearity turns noise
//! into a message-suppressing term and its curve knees, while the synchronous one degrades
//! linearly forever. The two tiers exist to put a number on that knee.

use num_complex::Complex;
use sdrmm_dsp::{Costas, DcBlocker, FirC, Pll, RealDecimator, design_lowpass};

use super::filter::{Band, BandFilter, design_vestigial};

/// What the carrier does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmMode {
    /// Full carrier at modulation depth `depth`: the baseband is `1 + depth·a(t)`, which stays
    /// positive — and therefore envelope-detectable — for `depth ≤ 1` and a message inside
    /// ±1. Broadcast practice leaves margin below 1; the engine allows the whole range and
    /// lets a measurement say what over-modulation costs.
    FullCarrier { depth: f64 },
    /// Suppressed carrier: the baseband is the message itself. All the transmitted power
    /// carries information, which is the 4.8 dB at depth 1 that
    /// [`am_fom`](crate::ber::theory::am_fom) prices — and no envelope survives it.
    Suppressed,
}

impl AmMode {
    /// Modulation depth, or 1 for a suppressed carrier, which is what the closed forms read as
    /// "every bit of the transmitted power is message".
    #[must_use]
    pub fn depth(self) -> f64 {
        match self {
            Self::FullCarrier { depth } => depth,
            Self::Suppressed => 1.0,
        }
    }
}

/// The waveform as data (§3.3): everything both ends need to agree on, and nothing about how
/// either end is implemented. Frequencies are cycles per sample throughout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmParams {
    pub mode: AmMode,
    /// Message bandwidth. The transmitter band-limits the message to it, the receiver's audio
    /// filter cuts at it, and every closed form the entry is held to reads the noise in it.
    pub bandwidth: f64,
    /// Half-width of the retained lower sideband. `None` is double sideband; `Some(v)` carves
    /// the lower sideband away over a complementary slope of half-width `v` about the carrier
    /// (see [`design_vestigial`]).
    pub vestige: Option<f64>,
    /// Taps in the transmitter's vestigial filter and the receiver's predetection band.
    pub band_taps: usize,
    /// Taps in the message-limiting and post-detection audio filters.
    pub audio_taps: usize,
}

impl AmParams {
    /// A double-sideband entry at the crate's default filter lengths — the shape most callers
    /// want, with the two tap counts stated once here instead of at every call site.
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

    /// The occupied band, as the receiver's predetection filter should be set: symmetric about
    /// the carrier for double sideband, and `[-vestige, bandwidth]` once a vestigial slope has
    /// carved the lower one away.
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

/// The transmitter: band-limited message onto an amplitude, optionally through the vestigial
/// slope.
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

    /// Replaces `out` with one complex-baseband sample per input audio sample.
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

/// The two detection tiers (§5 item 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmDetector {
    /// `|x|` — no carrier reference at all, which is why it is the tier every consumer in the
    /// repo runs and the one a suppressed carrier defeats.
    Envelope,
    /// A carrier loop, then the real part of what it de-rotates: a [`Pll`] on the residual
    /// carrier for [`AmMode::FullCarrier`], a [`Costas`] loop for [`AmMode::Suppressed`],
    /// because a suppressed carrier's baseband is real-valued and sign-bearing — which is to
    /// say it is BPSK, and its carrier is recovered the way BPSK's is.
    Synchronous { loop_bw: f64 },
}

/// What the receiver does around the detector. Both filters are optional and both defaults are
/// "on": a channel whose runtime already band-limited the input would otherwise pay for the
/// same filter twice, and a consumer reading a wideband baseband — a video raster — wants the
/// detector's output undecimated and unfiltered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmRx {
    pub detector: AmDetector,
    pub predetection: bool,
    pub audio_filter: bool,
    /// Remove the detector's DC. Always wanted after an envelope detector, where DC *is* the
    /// carrier; never wanted where the baseband's own level carries information (again, a
    /// video raster: its blanking level is the datum).
    pub dc_block: bool,
}

impl AmRx {
    /// The full receiver: IF filter, detector, DC block and audio filter.
    #[must_use]
    pub fn new(detector: AmDetector) -> Self {
        Self {
            detector,
            predetection: true,
            audio_filter: true,
            dc_block: true,
        }
    }

    /// The detector alone. What a channel wants when its own runtime already supplies the
    /// selectivity and owns whatever comes after the detector — `channels::atv` reads a video
    /// raster whose blanking level is its datum, so both the audio filter and the DC block would
    /// destroy the very thing it measures.
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

/// The receiver.
pub struct AmDemod {
    band: Option<Band>,
    carrier: Carrier,
    dc: Option<DcBlocker>,
    audio: Option<RealDecimator>,
    filtered: Vec<Complex<f32>>,
    detected: Vec<f32>,
}

impl AmDemod {
    /// Pull-in range of the carrier loop, as a multiple of its own bandwidth. A carrier loop
    /// with no range cannot acquire an offset at all, and one with unbounded range walks onto
    /// a strong sideband component and stays there; ten bandwidths is the span the entry's CFO
    /// row is measured over.
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

    /// Replaces `out` with one audio sample per input sample.
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

    /// Carrier-loop lock quality in 0..=1, or 1 for the envelope tier, which has nothing to
    /// lock and therefore nothing to lose.
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
    use super::*;
    use crate::ber::analog::{analyse_tone, tone};

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

    /// The envelope never folds while the depth stays inside 1, and the depth is readable back
    /// off the waveform — the transmitter's own calibration.
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

    /// Envelope detection of a full carrier: the tone comes back at `depth` times its own
    /// amplitude, with the carrier's DC removed and nothing else added.
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

    /// The same waveform through the synchronous tier: same amplitude, same cleanliness — the
    /// two tiers are one number above threshold, which is what the entry's curves then
    /// separate below it.
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

    /// The carrier loop earns its keep on an offset the envelope tier is simply blind to: a
    /// synchronous detector without one reads the message rotating through zero.
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

    /// The module's headline as a measurement: a suppressed carrier has no envelope. The
    /// synchronous tier reads it cleanly and the envelope tier rectifies it, and the rectified
    /// output's harmonic content is most of the signal.
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
        // A Costas loop resolves the carrier only up to π, so the recovered message may be
        // inverted — an amplitude, not a sign, is what the tier promises.
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

    /// Vestigial sideband recovers the message from a spectrum carrying barely more than half
    /// of it, and the complementary slope is why — measured through the synchronous detector,
    /// which is the one the slope's symmetry argument is about.
    #[test]
    fn a_vestigial_sideband_detects_undistorted() {
        let mut params = AmParams::new(AmMode::FullCarrier { depth: 0.8 }, BANDWIDTH);
        params.vestige = Some(500.0 / RATE);
        params.band_taps = 257;
        let iq = modulated(&params, 0.5, 65_536);
        let rx = AmRx::new(AmDetector::Synchronous { loop_bw: 1e-3 });
        let audio = demodulated(&params, &rx, &iq);
        let window = &audio[8_192..60_032];
        // The slope halves the carrier, and a detector's output scales with it.
        let amplitude = tone_amplitude(window, TONE);
        assert!((amplitude - 0.2).abs() < 0.02, "amplitude {amplitude}");
        assert!(
            harmonic_ratio(window, TONE) < 0.02,
            "distortion {}",
            harmonic_ratio(window, TONE)
        );
    }

    /// …and what an *envelope* detector costs on the same waveform, which is the reason
    /// broadcast television ran its carrier far above its sidebands. Carving one sideband
    /// leaves a quadrature component the magnitude cannot separate from the in-phase one, so
    /// the envelope is `√(i² + q²)` of a message plus its own Hilbert transform — a distortion
    /// no filter downstream removes, and one that shrinks only as the carrier is raised.
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
        // The quadrature term is proportional to the depth and the carrier it is measured
        // against is not, so quartering the depth quarters the distortion — asserted as the
        // ratio, which is the law rather than one of its values.
        params.mode = AmMode::FullCarrier { depth: 0.2 };
        let shallow = modulated(&params, 0.5, 65_536);
        let audio = demodulated(&params, &AmRx::new(AmDetector::Envelope), &shallow);
        let shallow = harmonic_ratio(&audio[8_192..60_032], TONE);
        let ratio = shallow / distortion;
        assert!((ratio - 0.25).abs() < 0.03, "distortion ratio {ratio}");
    }

    /// The optional stages are genuinely optional: with all three off the detector's own
    /// output arrives sample for sample, which is what a video consumer reads.
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
