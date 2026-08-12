//! The discriminator-tier CPM demodulator: carrier gate → detector front end → matched receive
//! filter → `SymbolSync` → M-level normalisation. One soft symbol per symbol period comes out,
//! gaps included, scaled so the transmitted levels sit at the mapping table's values whatever
//! the transmitter's actual deviation — a narrowband transmitter 20 % under-deviated is a
//! signal to decode, not to reject.
//!
//! This generalises `sdrmm_dsp::Fsk4Demod`'s burst policy piece by piece, because that policy
//! is the hard-won part (MODEM-PLAN §2.1): the floor-settled carrier gate, the keyed and idle
//! peak estimates, attack-limited level tracking, learn-only-while-settled, and the per-run
//! `process`/`process_held` split that keeps the clock coasting through TDMA dead time. Every
//! constant that carries a measured judgment is kept, restated where needed in the unit that
//! made it work (symbols rather than seconds — the gate races the matched filter's group
//! delay, which is symbol-denominated, so a 9600-baud entry gets the proportionally faster
//! gate the same reasoning demands).
//!
//! **All estimates are learned only while a carrier is present.** A discriminator fed a dead
//! channel emits noise that swings an order of magnitude past any symbol, and loops that
//! integrated it would arrive at the next burst having learned the receiver's noise floor
//! instead of the transmitter: the clock dragged off by percent, the centre averaged halfway
//! to zero, the level latched onto a noise spike. The gate is what makes a burst mode decode
//! at all, and a continuously-keyed mode never notices it.
//!
//! **Real-valued input is first-class** (MODEM-PLAN §3.5): [`CpmDemod::real`] builds the same
//! chain with the quadrature discriminator replaced by an audio-domain detector — an
//! analytic-signal discriminator about a subcarrier, or a two-tone correlator filterbank —
//! chosen per entry as [`RealDetector`] *data*. Everything downstream of the detector (matched
//! filter, timing, gate policy, levels, the known-symbol hook) is the identical code path.

use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, FmDemod, Nco, RealDecimator, SymbolSync, ToneCorrelator, design_lowpass,
    one_pole_coeff,
};

use super::params::CpmParams;

/// Timing loop bandwidth for burst/TDMA entries, in cycles per symbol — `fsk4`'s measured
/// 0.015. A transmitter's symbol clock is crystal-derived, so the loop only has to acquire:
/// quickly enough to be locked before a 30 ms burst's sync pattern arrives (~80 symbols from
/// a cold phase), and wide enough that a static sample-clock error is pulled in within an
/// acquisition preamble — the committed DMR limits row (23 047 ppm through an 88-symbol
/// preamble) is only reachable at this width. The cost is the continuous-stream self-noise
/// documented at [`TIMING_BW_CONTINUOUS`]; a TDMA gate freezes the loop through every gap
/// before the walk can accumulate, so burst entries never pay it.
pub const TIMING_BW_BURST: f64 = 0.015;

/// Timing loop bandwidth for continuously-keyed entries. Measured for this engine (see the
/// module docs in [`super`]): at 0.015 the Gardner detector's self-noise — mid-point ISI on
/// transitions the symmetry gate cannot reject — integrates into the loop until the rate
/// estimate walks ±0.09 % on a signal with zero clock error, and six clean 20 000-symbol
/// 4-level runs collect 879 symbol errors (7e-3: the committed phase-0 floor). At 0.003 the
/// same six runs collect one error and the rate stays within ±0.02 %. The cost is agility: a
/// static sample-clock offset pulls in ~25× slower, which the crystal-disciplined
/// transmitters behind continuous modes (tens of ppm) never make visible.
pub const TIMING_BW_CONTINUOUS: f64 = 0.003;

/// Floor on the symbol periods the centre estimate averages over — `fsk4`'s 150. A receiver
/// frequency error is static, so this is deliberately far longer than any run of one symbol a
/// mode can transmit (RTTY's idle is *continuous* mark — no alphabet argument may shorten the
/// guard against chasing it), and the committed DMR frequency-drift limit row was measured at
/// this speed.
const CENTRE_SYMBOLS_FLOOR: f32 = 150.0;

/// Calibration of the mapping-derived centre averaging. The centre learns from the filtered
/// signal's mean, so random data wobbles it by √(E[level²]/N) — a fixed N leaves the wobble
/// growing with the alphabet's power while the slicing margin (half a level spacing) stays
/// put: at `fsk4`'s N = 150 the wobble is 0.18 margins on the DMR table but 0.37 on an
/// 8-level one, which brushes the ±5/±7 boundary on clean signal. So N scales with the table:
/// `N = CENTRE_POWER_SYMBOLS · E[level²] / (spacing/2)²`, which holds the wobble at 0.18
/// margins for every alphabet and reproduces `fsk4`'s measured 150 exactly on the DMR table
/// (30 · 5 / 1). The cost is proportionally slower drift tracking for the bigger alphabets —
/// which need it: their margins shrink as fast as their wobble would have grown.
const CENTRE_POWER_SYMBOLS: f32 = 30.0;

/// Tracking rate of the peak estimate toward an outer symbol, in symbols carrying a signal —
/// `fsk4`'s decay constant, re-aimed (see below). Fast enough to follow a fading transmitter,
/// slow enough that noise on one outer symbol does not shrink the eye.
///
/// Two measured corrections to `fsk4`'s attack-above/decay-below dynamics, both invisible at
/// M = 4's 33 % slicing margins and fatal at M = 8's 14 % (isolated on the noise-free
/// 8-level loopback, sliced output against the open-loop chain, which reads the levels
/// exactly):
///
/// - **Only outer symbols may move the estimate down.** Decaying on every non-exceeding
///   symbol equilibrates the peak below the true outer level by a factor set by the outer
///   symbols' *density*: measured gain ×1.13 at M = 4 (half the symbols are outer — DMR
///   absorbed it silently) but ×1.30 at M = 8 (a quarter are). So downward tracking is gated
///   to the outer decision region of the current estimate.
/// - **The pull is proportional, toward the measured magnitude.** A fixed-step decay against
///   an attack-limited rise churns: the estimate falls a full step on any in-region symbol
///   and recovers only an eighth of the gap per symbol above it, riding the population's low
///   tail — measured ×1.09 residual. Pulling toward the magnitude settles closer to the
///   population and still follows a fading transmitter on the same timescale.
///
/// A residual bias of order the population's own spread remains — the attack/pull rate
/// asymmetry keeps the equilibrium in the population's lower half (measured peak 6.49 on an
/// 8-level population at 6.88, gain ×1.087; a symmetric-band variant was measured *worse*,
/// 6.20, because at M = 8 the band unavoidably swallows the next level's high tail: the band
/// spans ±14 % while the levels sit 29 % apart). That is the measured boundary of *blind*
/// magnitude-only normalisation: ample inside M ≤ 4's margins, most of the margin at M = 8 —
/// where the known-symbol hook (MODEM-PLAN §3.4, [`super::KnownSymbols`]) is the designed
/// level reference, exactly as every burst standard's sync pattern anticipates.
const PEAK_SYMBOLS: f32 = 60.0;

/// Decay of the peak estimate while the symbol sits *below* the outer region — the safety
/// path that keeps the estimate from latching high when the outer region goes quiet: a
/// transmitter that dropped more than the region width (a TDMA level step, gross
/// under-deviation) would otherwise never re-enter the region and never be followed. Four
/// times [`PEAK_SYMBOLS`], measured against both sides of the trade: slow enough that the
/// M = 8 equilibrium stays within its slicing margin (out-of-region loss must stay under the
/// in-region dynamics), fast enough that a −7.3 dB burst-to-burst level step — the committed
/// DMR limits row — decays within one 132-symbol burst to a gain the known-symbol hook's
/// 0.5..2.0 plausibility gate accepts (e^(−132/240) ⇒ measured gain ≈ 0.74).
const PEAK_HOLD_SYMBOLS: f32 = 4.0 * PEAK_SYMBOLS;

/// How much of a rise the peak estimate takes per symbol. A discriminator click, or the tail
/// of a keying edge that got past the gate, is one symbol at two or three times any level the
/// transmitter used; a plain maximum would scale the whole eye down by that for as long as its
/// decay, and an outer symbol scaled below the decision level slices as an inner one — the one
/// error the outer levels cannot absorb. Following a rise over a few symbols keeps a real
/// change in level and leaves a spike behind. Under-reading the level is safe, over-reading
/// is not.
const PEAK_ATTACK: f32 = 0.125;

/// Smoothing of the channel power the carrier gate reads, in symbol periods — `fsk4`'s 10⁻⁴ s,
/// which its comment already names "half a symbol at the fastest mode here". Deliberately
/// short: the gate has to fall below its threshold within the matched filter's group delay of
/// a transmitter keying down, and that delay is denominated in symbols, so the smoothing must
/// be too. Noise cannot open the gate however much this jitters, because opening also takes a
/// whole filter span of it.
const ENVELOPE_TAU_SYMBOLS: f64 = 0.5;

/// How fast the gate's noise-floor estimate follows the channel, in symbol periods — `fsk4`'s
/// 20 ms at 4800 baud — while nothing is keyed, and only then. A floor that went on learning
/// through a transmission would climb to the signal and gate out the very mode that never
/// stops transmitting.
const FLOOR_TAU_SYMBOLS: f64 = 96.0;

/// Multiples of [`FLOOR_TAU_SYMBOLS`] the floor is measured over before the gate may open at
/// all. Held shut rather than open: a gate that counted the startup transient as a carrier
/// would hand the level estimate the noise, and the first real burst would spend itself
/// decaying back down. Nothing is lost by waiting — there is no estimate worth making yet
/// either.
const FLOOR_SETTLE: f64 = 4.0;

/// How far above the noise floor the channel power has to sit for a carrier to be counted
/// present. Six decibels: no discriminator-tier FSK signal decodes at less, so nothing that
/// could have been decoded is gated out, and power smoothed over [`ENVELOPE_TAU_SYMBOLS`] of
/// noise never reaches it.
const CARRIER_RISE: f32 = 4.0;

/// Taps of the analytic front end's image-reject lowpass. 127 at the audio rates these
/// entries run (the ACARS chain this generalises uses the same order) puts the transition
/// band well inside the gap between the wanted tones and their mirror image.
const IMAGE_TAPS: usize = 127;

/// The audio-domain detector a real-input construction runs in place of the quadrature
/// discriminator — per-entry data, never a protocol branch (MODEM-PLAN §3.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RealDetector {
    /// Mix the audio down by `centre_hz`, reject the mirror image a real signal carries, and
    /// quadrature-discriminate — the MSK-in-AM shape (ACARS: 1800 Hz centre). Output is in
    /// mapping-level units, exactly as the complex path's discriminator.
    Discriminator {
        /// The frequency midway between the outermost tones, in Hz at the audio rate.
        centre_hz: f64,
    },
    /// Two sliding tone correlators; their magnitude difference is the detected level — the
    /// classic AFSK detector (Bell 202: 1200/2200 Hz). Two tones detect two levels, so this
    /// detector requires M = 2; the tone that reads +1 and the tone that reads −1 are stated
    /// explicitly, and the mapping table says which *symbol* each is — the assignment is
    /// data, not a sign convention.
    ToneFilterbank {
        /// Tone whose presence reads as level +1, in Hz.
        plus_hz: f64,
        /// Tone whose presence reads as level −1, in Hz.
        minus_hz: f64,
    },
}

/// Everything between the input samples and the detected real-valued level signal.
enum FrontEnd {
    /// Complex IQ in: quadrature FM discriminator, scaled so a symbol at mapping level L
    /// reads L (`FmDemod` at rate = sps, deviation = h/2 — the per-sample phase step of
    /// level L is π·h·L/sps, so ±h/2 "cycles per sample-rate unit" is ±1 level).
    Quadrature(FmDemod),
    /// Real audio in: subcarrier shift, image reject, then the same discriminator (deviation
    /// per level unit is h·baud/2 Hz).
    Analytic {
        mixer: Nco,
        image: Decimator,
        demod: FmDemod,
        mixed: Vec<Complex<f32>>,
        baseband: Vec<Complex<f32>>,
    },
    /// Real audio in: tone-pair correlator difference, already in ±1 level units.
    Filterbank {
        plus: ToneCorrelator,
        minus: ToneCorrelator,
    },
}

/// M-ary CPM/CPFSK demodulator producing normalised soft symbols; see the module docs. Slicing
/// and per-bit soft output are the mapping table's job
/// ([`Mapping::slice`](super::Mapping::slice), [`Mapping::soft_bits`](super::Mapping::soft_bits));
/// the known-symbol hook ([`KnownSymbols`](super::KnownSymbols)) corrects these symbols where a
/// protocol has sync patterns to anchor on.
pub struct CpmDemod {
    front: FrontEnd,
    matched: RealDecimator,
    sync: SymbolSync,
    /// The mapping's outer |level| — what the peak estimates normalise to.
    level_max: f32,
    centre: f32,
    /// Level the decision scale is set by, learned only from a keyed channel, in mapping-level
    /// units (a transmitter at the nominal deviation puts its outer symbols at ±`level_max`).
    peak: f32,
    /// The same estimate for a channel with nothing on it, learned only while there is not.
    /// A discriminator fed noise swings further than any symbol a transmitter sends, so
    /// scaling dead time by the *signal's* level would slice every sample of it to an outer
    /// symbol — a stream of two indices where there should be M, which a sync pattern then
    /// matches by chance far more often than noise ever may.
    idle_peak: f32,
    centre_alpha: f32,
    peak_decay: f32,
    peak_hold_decay: f32,
    /// Lower edge of the outer decision region as a fraction of the peak estimate:
    /// `(max_level − spacing/2) / max_level`. Zero for M = 2, where both levels are outer and
    /// every keyed symbol tracks the peak.
    outer_region: f32,
    envelope: f32,
    floor: f32,
    envelope_alpha: f32,
    floor_alpha: f32,
    /// Samples left of the window the floor is measured over, during which the gate is held
    /// shut because nothing is yet known about the channel.
    settling: usize,
    settle_samples: usize,
    /// Consecutive input samples whose power was above the gate's floor, saturating at
    /// `support`.
    keyed: usize,
    /// Samples of filtering between the input and a recovered symbol: the matched filter plus
    /// whatever support the front end adds (a correlator window, the image filter). The
    /// chain's output only stops carrying a burst edge once this whole span has a carrier
    /// under it, so the gate opens that late and closes that early — eroding the keyed
    /// interval by the group delay at each end.
    support: usize,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
    /// Whether each sample of `filtered` has a carrier under it, and whether it has had one
    /// for the whole of `support`. The first says which of the two level estimates describes
    /// it; only the second may be *learned* from, because within a span of a keying edge the
    /// filtered output is part burst and part dead channel. A burst's first symbols still
    /// have to be sliced against the transmitter's level — they are the head of the payload —
    /// so the two questions cannot share one answer.
    carrier_run: Vec<bool>,
    settled_run: Vec<bool>,
    /// The timing recovery works on complex baseband; a detector produces real samples, and
    /// for those Gardner's expression carries the imaginary part along as zero.
    centred: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
    /// The same two questions, per symbol of `retimed`.
    retimed_carrier: Vec<bool>,
    retimed_settled: Vec<bool>,
}

impl CpmDemod {
    /// Complex-baseband construction: quadrature discriminator front end.
    ///
    /// `receive_filter` is the entry's frequency-pulse-matched receive filter — the RRC half
    /// of a root pair, the Gaussian premod shape, rect for NRZ keying — as
    /// [`pulse::Norm::Area`](crate::pulse::Norm) taps: the level estimates rely on the
    /// filter's unit DC gain. `timing_bw` is the `SymbolSync` loop bandwidth in cycles per
    /// symbol; [`TIMING_BW_BURST`] and [`TIMING_BW_CONTINUOUS`] are the two measured
    /// operating points and the choice is part of the entry's data.
    ///
    /// # Panics
    /// If `receive_filter` is empty or not unit-area, or `timing_bw` is outside
    /// `SymbolSync`'s (0, 1).
    #[must_use]
    pub fn new(params: &CpmParams, receive_filter: &[f32], timing_bw: f64) -> Self {
        let front = FrontEnd::Quadrature(FmDemod::new(params.sps(), params.h() / 2.0));
        Self::build(params, receive_filter, timing_bw, front, 0)
    }

    /// Real-valued (audio-domain) construction: same chain, the discriminator front end
    /// replaced per `detector`. `sample_rate` is the audio rate in Hz; the entry's baud is
    /// `sample_rate / params.sps()`, and the detector's tone geometry is stated in Hz at that
    /// rate. Feed it with [`Self::process_real`].
    ///
    /// # Panics
    /// As [`Self::new`]; additionally if `sample_rate` is not positive, a tone sits outside
    /// (0, `sample_rate`/2), the filterbank tones coincide, or a filterbank is requested for
    /// M ≠ 2 (two tones detect two levels; M-tone orthogonal FSK is `orthogonal/`'s
    /// filterbank, not this detector).
    #[must_use]
    pub fn real(
        params: &CpmParams,
        receive_filter: &[f32],
        timing_bw: f64,
        sample_rate: f64,
        detector: RealDetector,
    ) -> Self {
        assert!(
            sample_rate.is_finite() && sample_rate > 0.0,
            "sample rate must be positive"
        );
        let baud = sample_rate / params.sps();
        let (front, extra) = match detector {
            RealDetector::Discriminator { centre_hz } => {
                assert!(
                    centre_hz > 0.0 && centre_hz < sample_rate / 2.0,
                    "subcarrier centre must lie inside the Nyquist band"
                );
                // The wanted band ends near the outer deviation and the mirror image begins
                // at 2·centre − deviation; the −6 dB point midway between them is the centre
                // frequency itself.
                let taps = design_lowpass(IMAGE_TAPS, centre_hz / sample_rate);
                (
                    FrontEnd::Analytic {
                        mixer: Nco::new(-centre_hz as f32, sample_rate as f32),
                        image: Decimator::new(&taps, 1),
                        demod: FmDemod::new(sample_rate, params.h() * baud / 2.0),
                        mixed: Vec::new(),
                        baseband: Vec::new(),
                    },
                    IMAGE_TAPS,
                )
            }
            RealDetector::ToneFilterbank { plus_hz, minus_hz } => {
                assert_eq!(
                    params.mapping().m(),
                    2,
                    "a two-tone filterbank detects two levels"
                );
                assert!(plus_hz != minus_hz, "filterbank tones must be distinct");
                // A window of rate/|Δf| samples spaces the sliding-DFT bins by exactly the
                // tone split, so each correlator sits on the other tone's null.
                let window = (sample_rate / (plus_hz - minus_hz).abs()).round() as usize;
                (
                    FrontEnd::Filterbank {
                        plus: ToneCorrelator::new(sample_rate, plus_hz, window),
                        minus: ToneCorrelator::new(sample_rate, minus_hz, window),
                    },
                    window,
                )
            }
        };
        Self::build(params, receive_filter, timing_bw, front, extra)
    }

    fn build(
        params: &CpmParams,
        receive_filter: &[f32],
        timing_bw: f64,
        front: FrontEnd,
        front_support: usize,
    ) -> Self {
        assert!(!receive_filter.is_empty(), "receive filter must have taps");
        let area: f64 = receive_filter.iter().map(|&t| f64::from(t)).sum();
        assert!(
            (area - 1.0).abs() < 1e-3,
            "receive filter must be unit-area (pulse::Norm::Area), got Σ = {area}"
        );
        let sps = params.sps();
        let level_max = params.mapping().max_level();
        let settle = (FLOOR_SETTLE * FLOOR_TAU_SYMBOLS * sps) as usize;
        let mean_sq = params
            .mapping()
            .levels()
            .iter()
            .map(|&l| l * l)
            .sum::<f32>()
            / params.mapping().m() as f32;
        let half_spacing = params.mapping().min_spacing() / 2.0;
        let centre_symbols = (CENTRE_POWER_SYMBOLS * mean_sq / (half_spacing * half_spacing))
            .max(CENTRE_SYMBOLS_FLOOR);
        Self {
            front,
            matched: RealDecimator::new(receive_filter, 1),
            sync: SymbolSync::new(sps, timing_bw),
            level_max,
            centre: 0.0,
            peak: level_max,
            idle_peak: level_max,
            centre_alpha: 1.0 / (centre_symbols * sps as f32),
            peak_decay: 1.0 - 1.0 / PEAK_SYMBOLS,
            peak_hold_decay: 1.0 - 1.0 / PEAK_HOLD_SYMBOLS,
            outer_region: (level_max - params.mapping().min_spacing() / 2.0).max(0.0) / level_max,
            envelope: 0.0,
            floor: 0.0,
            envelope_alpha: one_pole_coeff(sps, ENVELOPE_TAU_SYMBOLS),
            floor_alpha: one_pole_coeff(sps, FLOOR_TAU_SYMBOLS),
            settling: settle,
            settle_samples: settle,
            keyed: 0,
            support: receive_filter.len() + front_support,
            demod_buf: Vec::new(),
            filtered: Vec::new(),
            carrier_run: Vec::new(),
            settled_run: Vec::new(),
            centred: Vec::new(),
            retimed: Vec::new(),
            retimed_carrier: Vec::new(),
            retimed_settled: Vec::new(),
        }
    }

    /// Demodulate a block of complex baseband, appending one soft symbol per recovered symbol
    /// period to `out`. Timing and level state carry across calls, so any block split gives
    /// the same symbols.
    ///
    /// # Panics
    /// If this demodulator was constructed with [`Self::real`] — the wrong input domain is a
    /// construction-site bug, caught on the first call.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.carrier_run.clear();
        self.settled_run.clear();
        for sample in iq {
            self.gate_sample(sample.norm_sqr());
        }
        let FrontEnd::Quadrature(demod) = &mut self.front else {
            panic!("constructed for real input; call process_real");
        };
        demod.process(iq, &mut self.demod_buf);
        self.finish(out);
    }

    /// As [`Self::process`], for real audio-domain samples through the constructed
    /// [`RealDetector`]. The chain downstream of the detector — and therefore the output —
    /// is identical.
    ///
    /// # Panics
    /// If this demodulator was constructed with [`Self::new`].
    pub fn process_real(&mut self, audio: &[f32], out: &mut Vec<f32>) {
        self.carrier_run.clear();
        self.settled_run.clear();
        for &sample in audio {
            self.gate_sample(sample * sample);
        }
        match &mut self.front {
            FrontEnd::Quadrature(_) => panic!("constructed for IQ input; call process"),
            FrontEnd::Analytic {
                mixer,
                image,
                demod,
                mixed,
                baseband,
            } => {
                mixed.clear();
                mixed.extend(
                    audio
                        .iter()
                        .map(|&s| Complex::new(s, 0.0) * mixer.next_sample()),
                );
                image.process(mixed, baseband);
                demod.process(baseband, &mut self.demod_buf);
            }
            FrontEnd::Filterbank { plus, minus } => {
                self.demod_buf.clear();
                self.demod_buf
                    .extend(audio.iter().map(|&s| plus.push(s) - minus.push(s)));
            }
        }
        self.finish(out);
    }

    /// Whether one input sample has a carrier under it, judged against a noise floor measured
    /// from the channel's own quiet.
    ///
    /// The floor may not be a recent *maximum*: an idle channel's loudest noise is its own
    /// noise, so nothing would ever read as quiet, and the loops would go on learning through
    /// the seconds before a call as readily as through the dead time inside one.
    ///
    /// A channel first sampled mid-transmission measures its floor on that carrier and reads
    /// keyed off until the carrier drops. That costs the estimates their chance to learn,
    /// which is what a cold start costs anyway — it cannot cost the decoder the signal.
    fn gate_sample(&mut self, power: f32) {
        self.envelope += self.envelope_alpha * (power - self.envelope);
        self.settling = self.settling.saturating_sub(1);
        let keyed = self.settling == 0 && self.envelope > self.floor * CARRIER_RISE;
        if !keyed {
            self.floor += self.floor_alpha * (self.envelope - self.floor);
        }
        // The filtered output only stops carrying a burst's keying edge once the whole
        // filtering span has a carrier under it.
        self.keyed = if keyed {
            (self.keyed + 1).min(self.support)
        } else {
            0
        };
        self.carrier_run.push(keyed);
        self.settled_run.push(self.keyed == self.support);
    }

    /// The shared tail of both input domains: matched filter → centre removal → timing →
    /// level normalisation, `demod_buf` in, soft symbols out.
    fn finish(&mut self, out: &mut Vec<f32>) {
        self.matched.process(&self.demod_buf, &mut self.filtered);
        self.centred.clear();
        for (&sample, &settled) in self.filtered.iter().zip(&self.settled_run) {
            if settled {
                // Per sample rather than per symbol: the timing detector is fed the centred
                // signal, so the estimate has to advance with the samples it is subtracted
                // from, whatever size the blocks arrive in.
                self.centre += self.centre_alpha * (sample - self.centre);
            }
            self.centred.push(Complex::new(sample - self.centre, 0.0));
        }

        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
        // One `SymbolSync` call per run of constant gate state, so each recovered symbol can
        // be attributed to one. Splitting a block this way cannot change the symbols — the
        // timing state carries across calls — and a mode that never keys off makes one call
        // per block as before.
        //
        // The signal itself is passed through either way. The gate decides only what the
        // loops are allowed to learn from, never what the decoder above gets to see: a gate
        // that misjudged a channel would otherwise be able to silence a signal that was
        // decoding.
        let mut start = 0;
        while start < self.centred.len() {
            let (carrier, settled) = (self.carrier_run[start], self.settled_run[start]);
            let mut end = start + 1;
            while end < self.centred.len()
                && self.carrier_run[end] == carrier
                && self.settled_run[end] == settled
            {
                end += 1;
            }
            let run = &self.centred[start..end];
            if settled {
                self.sync.process(run, &mut self.retimed);
            } else {
                self.sync.process_held(run, &mut self.retimed);
            }
            self.retimed_carrier.resize(self.retimed.len(), carrier);
            self.retimed_settled.resize(self.retimed.len(), settled);
            start = end;
        }

        let (mut peak, mut idle) = (self.peak, self.idle_peak);
        let carriers = self.retimed_carrier.iter().zip(&self.retimed_settled);
        for (symbol, (&carrier, &settled)) in self.retimed.iter().zip(carriers) {
            let value = symbol.re;
            // Each span is scaled by the level of what is actually on it, and neither
            // estimate learns from the other's span. Within a filtering span of a keying
            // edge the symbol belongs to the burst and is scaled by the burst's level, but
            // nothing learns from it: that is where the transient lives.
            let magnitude = value.abs();
            if carrier {
                if settled {
                    // Three zones (see PEAK_SYMBOLS): above the estimate the attack limiter
                    // takes over; inside the outer region the symbol *is* the outer level,
                    // so the estimate pulls toward it; below, only the slow safety decay
                    // applies, because an inner symbol says nothing about the eye.
                    if magnitude > peak {
                        peak += PEAK_ATTACK * (magnitude - peak);
                    } else if magnitude > peak * self.outer_region {
                        peak += (magnitude - peak) / PEAK_SYMBOLS;
                    } else {
                        peak *= self.peak_hold_decay;
                    }
                }
            } else {
                // Dead time has no alphabet, so no region: the idle estimate follows the
                // noise itself, exactly as `fsk4` tracked it.
                if magnitude > idle {
                    idle += PEAK_ATTACK * (magnitude - idle);
                } else {
                    idle *= self.peak_decay;
                }
            }
            let level = if carrier { &peak } else { &idle };
            // Guard the divide: a squelched channel produces zeros, and a symbol stream of
            // NaN would poison every decoder above this one.
            let unit = *level / self.level_max;
            out.push(if unit > 1e-6 { value / unit } else { 0.0 });
        }
        (self.peak, self.idle_peak) = (peak, idle);
    }

    /// Forget the timing and level estimates — the channel moved, and what this has learned
    /// describes the transmitter it just left. Filter histories are not flushed (their stale
    /// span washes out within one filter length, exactly as a retune leaves any FIR).
    pub fn reset(&mut self) {
        self.sync.reset();
        if let FrontEnd::Filterbank { plus, minus } = &mut self.front {
            plus.reset();
            minus.reset();
        }
        self.centre = 0.0;
        self.peak = self.level_max;
        self.idle_peak = self.level_max;
        self.envelope = 0.0;
        self.floor = 0.0;
        self.settling = self.settle_samples;
        self.keyed = 0;
        self.demod_buf.clear();
        self.filtered.clear();
        self.carrier_run.clear();
        self.settled_run.clear();
        self.centred.clear();
        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{
        super::{levels::KnownSymbols, modulator::CpmMod, params::Mapping},
        *,
    };
    use crate::{
        ber::rng::Rng,
        pulse::{self, Norm},
    };

    const RATE: f64 = 48_000.0;
    const BAUD: f64 = 4_800.0;
    const SPS: f64 = 10.0;

    /// The ETSI dibit table (TS 102 361-1 §4.2.2): 00 → +1, 01 → +3, 10 → −1, 11 → −3.
    fn dibit_mapping() -> Mapping {
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
    }

    /// A DMR-shaped 4-level entry at the given outer deviation.
    fn four_level(deviation_hz: f64) -> CpmParams {
        CpmParams::from_deviation(
            dibit_mapping(),
            deviation_hz,
            BAUD,
            pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area),
            SPS,
        )
    }

    fn rx_rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area)
    }

    fn symbols(len: usize, seed: u32, m: u8) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as u8) & (m - 1)
            })
            .collect()
    }

    fn transmit(params: &CpmParams, syms: &[u8]) -> Vec<Complex<f32>> {
        let mut m = CpmMod::new(params.clone());
        let mut out = Vec::new();
        m.modulate(syms, &mut out);
        m.flush(&mut out);
        out
    }

    /// A receiver's own noise, 40 dB below a unit-magnitude carrier — what an antenna delivers
    /// when no one is transmitting. Digital silence is not that, and a gate handed it would
    /// measure a noise floor of zero and never close again.
    const NOISE: f32 = 0.01;

    fn noise(seed: u64, len: usize) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        (0..len)
            .map(|_| {
                let re = (rng.uniform() as f32 * 2.0 - 1.0) * NOISE;
                let im = (rng.uniform() as f32 * 2.0 - 1.0) * NOISE;
                Complex::new(re, im)
            })
            .collect()
    }

    fn real_noise(seed: u64, len: usize) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        (0..len)
            .map(|_| (rng.uniform() as f32 * 2.0 - 1.0) * NOISE)
            .collect()
    }

    /// A demodulator that has already heard the channel quiet, which is how a receiver meets
    /// every transmission it was tuned to before the transmitter keyed up. The quiet must
    /// outlast the gate's floor-settle window, which scales with sps.
    fn listening(demod: &mut CpmDemod, seed: u64) {
        let len = demod.settle_samples + 4 * SPS as usize * 100;
        let quiet = noise(seed, len);
        let mut discard = Vec::new();
        demod.process(&quiet, &mut discard);
    }

    /// Errors between `got` and `sent` once the chain delay is taken out, skipping the
    /// lead-in the clock and the level estimate need. The alignment is searched rather than
    /// assumed: it is a property of the filter spans, not of the mode.
    fn symbol_errors(got: &[u8], sent: &[u8], skip: usize) -> (usize, usize) {
        let (delay, errors) = (0..48)
            .map(|delay| {
                let errors = got
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .filter(|&(i, s)| sent.get(i.wrapping_sub(delay)).is_none_or(|w| w != s))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        assert!((1..40).contains(&delay), "implausible alignment {delay}");
        (errors, got.len() - skip)
    }

    /// The whole point of the front end: symbols in, the same symbols out, at a deviation the
    /// demodulator was never told about — an under-deviated transmitter is a signal to
    /// decode, not to reject.
    #[test]
    fn recovers_four_level_symbols_at_an_unexpected_deviation() {
        for deviation in [1_944.0, 1_400.0, 2_600.0] {
            let sent = symbols(400, 17, 4);
            let iq = transmit(&four_level(deviation), &sent);
            let nominal = four_level(1_944.0);
            let mut demod = CpmDemod::new(&nominal, &rx_rrc(), TIMING_BW_BURST);
            listening(&mut demod, 0x1157);
            let mut soft = Vec::new();
            demod.process(&iq, &mut soft);
            let got: Vec<u8> = soft.iter().map(|&s| nominal.mapping().slice(s)).collect();
            let (errors, total) = symbol_errors(&got, &sent, 100);
            assert!(total > 280, "only {total} symbols at {deviation} Hz");
            assert_eq!(errors, 0, "symbol errors at {deviation} Hz deviation");
        }
    }

    /// A receiver is never exactly on frequency. The centre estimate has to absorb the offset
    /// a mistuned dial or a drifting transmitter puts on the discriminator.
    #[test]
    fn tracks_a_carrier_offset() {
        let sent = symbols(900, 5, 4);
        let params = four_level(1_944.0);
        let mut iq = transmit(&params, &sent);
        // 400 Hz off — a fifth of the outer deviation, which un-centred would slice the
        // +1 level as +3 whenever it drifted high.
        for (k, s) in iq.iter_mut().enumerate() {
            *s *= Complex::from_polar(1.0, (TAU * 400.0 * k as f64 / RATE) as f32);
        }
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        // The centre estimate averages over hundreds of symbols, so the offset is only fully
        // absorbed in the tail — which is what a decoder hunting a sync pattern needs.
        let (errors, _) = symbol_errors(&got, &sent, soft.len() - 200);
        assert_eq!(errors, 0);
    }

    /// The host hands a channel whatever the device gave it. Every piece of state — filter
    /// history, timing accumulator, centre and peak estimates — has to advance with the
    /// samples rather than with the calls, or the same signal would decode differently
    /// depending on the radio's buffer size.
    #[test]
    fn block_splits_do_not_change_the_symbols() {
        let sent = symbols(300, 41, 4);
        let params = four_level(1_944.0);
        let iq = transmit(&params, &sent);
        let mut whole = Vec::new();
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        demod.process(&iq, &mut whole);

        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut ragged = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 7].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            demod.process(&iq[pos..end], &mut ragged);
            pos = end;
        }
        assert_eq!(whole, ragged);
    }

    /// The TDMA case, which is what the carrier gate exists for. A DMR-shaped radio radiates
    /// 132 symbols in every 288 and the receiver hears its own noise for the rest; the clock,
    /// centre and level have to arrive at each burst holding what the *transmitter* taught
    /// them, not what the dead channel did.
    #[test]
    fn a_keyed_transmitter_does_not_lose_its_clock_in_the_dead_time() {
        const ON: usize = 132;
        const FRAME: usize = 288;
        let params = four_level(1_944.0);
        let sent = symbols(2_880, 23, 4);
        let keyed: Vec<Option<u8>> = sent
            .iter()
            .enumerate()
            .map(|(i, &s)| (i % FRAME < ON).then_some(s))
            .collect();
        let mut iq = CpmMod::new(params.clone()).keyed(&keyed);
        let floor = noise(0xbeef, iq.len());
        for (s, n) in iq.iter_mut().zip(floor) {
            // The receiver's noise is on the channel whether the transmitter is keyed or
            // not; it is what the gate has to recognise the dead time by.
            *s += n;
        }
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);

        // One symbol per symbol period of input, gaps included: the decoders above count the
        // dead time out in symbols to find the next burst in their slot, so a clock that ran
        // fast or slow through it would put them on the wrong bits.
        let ideal = iq.len() / SPS as usize;
        assert!(
            (soft.len() as i64 - ideal as i64).abs() <= 2,
            "recovered {} symbols, ideal {ideal}",
            soft.len()
        );

        // Only the keyed symbols carry anything; the last burst is checked because it is the
        // one that has been through every gap.
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (delay, _) = (0..48)
            .map(|delay| {
                let errors = (300usize..400)
                    .filter(|&i| sent.get(i.wrapping_sub(delay)).is_none_or(|w| *w != got[i]))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        let last = sent.len() - FRAME + delay;
        let bad: Vec<usize> = (last..last + ON)
            .filter(|&i| sent.get(i - delay).is_none_or(|w| *w != got[i]))
            .map(|i| i - last)
            .collect();
        assert!(
            bad.is_empty(),
            "symbol errors at {bad:?} in the last of {} bursts",
            sent.len() / FRAME
        );
    }

    /// The committed phase-0 finding, beaten and held: on continuous random 4FSK the old
    /// chain floors near 1e-2 past ~2000 symbols (dmr_baseline.rs module docs). The engine at
    /// its continuous operating point must hold lock through 20k symbols with at least an
    /// order of magnitude in hand — measured while choosing [`TIMING_BW_CONTINUOUS`]: this
    /// run's error count was 0 (six independent 20k runs during design: 1 error total).
    #[test]
    fn a_continuous_stream_holds_lock_over_twenty_thousand_symbols() {
        let sent = symbols(20_000, 0x5eed, 4);
        let params = four_level(1_944.0);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_CONTINUOUS);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 500);
        assert!(total > 19_000, "only {total} symbols recovered");
        // 1e-3 as the gate: an order under the old floor, an order over what was measured.
        assert!(
            errors <= total / 1_000,
            "{errors} symbol errors in {total}: the continuous floor is back"
        );
    }

    /// GMSK at BT = 0.5, h = ½ — the D-STAR/Bluetooth-BR shape: Gaussian partial-response
    /// frequency pulse, Gaussian receive filter.
    #[test]
    fn gmsk_loopback_is_clean() {
        let params = CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::gaussian_freq(SPS, 0.5, 3, Norm::Area),
            SPS,
        );
        let sent = symbols(600, 71, 2);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(
            &params,
            &pulse::gaussian(SPS, 0.5, 3, Norm::Area),
            TIMING_BW_BURST,
        );
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 100);
        assert!(total > 400);
        assert_eq!(errors, 0, "GMSK symbol errors");
    }

    /// Plain 2FSK at h = ½ (MSK-index CPFSK): rect pulse, integrate-and-dump receive filter —
    /// the POCSAG/RTTY base shape.
    #[test]
    fn two_level_cpfsk_loopback_is_clean() {
        let params = CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS);
        let sent = symbols(600, 29, 2);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &pulse::rect(SPS, Norm::Area), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 100);
        assert!(total > 400);
        assert_eq!(errors, 0, "2FSK symbol errors");
    }

    /// Eight levels — no protocol behind it, which is the point (§7 phase 3: 8-ary gates the
    /// engine's generality). The level scale rides the known-symbol hook, as §3.4 prescribes
    /// for CPM: blind magnitude-only normalisation was measured to hold the scale within
    /// ×1.09 of true (see [`PEAK_SYMBOLS`]) — ample inside M ≤ 4's 33 % margins, but most of
    /// an 8-level entry's 14 % — so an 8-level entry embeds known symbols exactly as every
    /// burst standard does, and the hook's least-squares gain fit carries the rest. Zero
    /// errors demanded on every hook-corrected payload symbol.
    #[test]
    fn eight_level_loopback_is_clean_on_the_known_symbol_hook() {
        const PATTERN: [u8; 16] = [7, 0, 5, 2, 6, 1, 4, 3, 0, 7, 3, 4, 1, 6, 2, 5];
        const PERIOD: usize = 128;
        const FRAMES: usize = 12;
        let params = CpmParams::from_h(Mapping::natural(8), 0.3, pulse::rect(SPS, Norm::Area), SPS);
        let mut sent = Vec::with_capacity(FRAMES * PERIOD);
        for frame in 0..FRAMES {
            sent.extend_from_slice(&PATTERN);
            sent.extend(symbols(PERIOD - PATTERN.len(), 0x0dd5 ^ frame as u32, 8));
        }
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &pulse::rect(SPS, Norm::Area), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);

        // Locate the stream by raw slicing (the blind scale slices well enough to align),
        // then run the hook the way a protocol does: anchor on each known window, slice the
        // payload behind it through the correction.
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (delay, _) = (0..48usize)
            .map(|d| {
                let errs = (200..1_000usize)
                    .filter(|&i| {
                        sent.get(i.wrapping_sub(d))
                            .is_none_or(|w| got.get(i).is_none_or(|g| g != w))
                    })
                    .count();
                (d, errs)
            })
            .min_by_key(|&(_, e)| e)
            .unwrap();
        let mut hook = KnownSymbols::new(&params, (4 * PERIOD) as u32);
        let mut errors = Vec::new();
        // Frame 0 falls inside the clock's pull-in; every later frame must be perfect.
        for frame in 1..FRAMES {
            let base = frame * PERIOD + delay;
            hook.anchor(&PATTERN, &soft[base..base + PATTERN.len()]);
            for k in PATTERN.len()..PERIOD {
                hook.tick();
                let Some(&s) = soft.get(base + k) else {
                    continue;
                };
                if params.mapping().slice(hook.correct(s)) != sent[frame * PERIOD + k] {
                    errors.push(frame * PERIOD + k);
                }
            }
        }
        assert!(
            errors.is_empty(),
            "8-level symbol errors at {errors:?} through the hook"
        );
    }

    #[test]
    fn a_silent_channel_produces_finite_symbols() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let mut soft = Vec::new();
        demod.process(&vec![Complex::new(0.0, 0.0); 4_800], &mut soft);
        assert!(!soft.is_empty());
        assert!(soft.iter().all(|s| s.is_finite()), "non-finite symbol");
    }

    // --- Real-valued input (§3.5) -------------------------------------------------------

    /// Bell-202-like AFSK on real audio: 1200/2200 Hz about 1700, 1200 baud at 48 kHz. Mark
    /// (bit 1, 1200 Hz) sits *below* the centre, so its level is −1 and the mapping table —
    /// not a sign convention — carries the assignment: index 0 → +1 (2200 Hz), index 1 → −1.
    fn afsk_params() -> CpmParams {
        CpmParams::from_deviation(
            Mapping::new(vec![1.0, -1.0]),
            500.0,
            1_200.0,
            pulse::rect(40.0, Norm::Area),
            40.0,
        )
    }

    /// The AFSK entry's receive filter: half a symbol of rect. The tone correlators already
    /// integrate a 48-sample window — 1.2 symbols, the symbol matched filter and then some —
    /// so the post-detector filter only smooths correlator ripple, and giving it a *full*
    /// symbol stacks over two symbols of integration onto every transition: measured on this
    /// exact loopback, a one-symbol rect here collapses transitions to ±0.05 soft values and
    /// costs 14 errors in 420; half a symbol costs none.
    fn afsk_rx() -> Vec<f32> {
        pulse::rect(20.0, Norm::Area)
    }

    /// The tone-pair audio a Bell-202 transmitter keys: the engine's own baseband CPFSK
    /// shifted onto the 1700 Hz audio subcarrier.
    fn afsk_audio(sent: &[u8]) -> Vec<f32> {
        let baseband = transmit(&afsk_params(), sent);
        let mut carrier = Nco::new(1_700.0, RATE as f32);
        baseband
            .iter()
            .map(|&s| (s * carrier.next_sample()).re)
            .collect()
    }

    /// `receive_filter` is per-detector entry data: the discriminator has no integration of
    /// its own, so it takes the full-symbol matched rect; the filterbank takes [`afsk_rx`].
    fn afsk_roundtrip(detector: RealDetector, receive_filter: &[f32], seed: u32) {
        let params = afsk_params();
        let sent = symbols(500, seed, 2);
        let audio = afsk_audio(&sent);
        let mut demod = CpmDemod::real(&params, receive_filter, TIMING_BW_BURST, RATE, detector);
        // The receiver heard the audio channel quiet first, so the gate's floor is real.
        let quiet = real_noise(0x1157, demod.settle_samples + 19_200);
        let mut discard = Vec::new();
        demod.process_real(&quiet, &mut discard);
        let mut soft = Vec::new();
        demod.process_real(&audio, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 80);
        assert!(total > 350, "only {total} symbols");
        assert_eq!(errors, 0, "AFSK bit errors with {detector:?}");
    }

    #[test]
    fn afsk_decodes_through_the_tone_filterbank() {
        afsk_roundtrip(
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
            &afsk_rx(),
            33,
        );
    }

    /// The same audio through the other detector option — the choice is per-entry data, and
    /// both must hand the identical downstream chain a level signal it decodes.
    #[test]
    fn afsk_decodes_through_the_analytic_discriminator() {
        afsk_roundtrip(
            RealDetector::Discriminator { centre_hz: 1_700.0 },
            &pulse::rect(40.0, Norm::Area),
            34,
        );
    }

    #[test]
    fn real_block_splits_do_not_change_the_symbols() {
        let params = afsk_params();
        let sent = symbols(200, 9, 2);
        let audio = afsk_audio(&sent);
        let detector = RealDetector::ToneFilterbank {
            plus_hz: 2_200.0,
            minus_hz: 1_200.0,
        };
        let filter = afsk_rx();
        let mut whole = Vec::new();
        let mut demod = CpmDemod::real(&params, &filter, TIMING_BW_BURST, RATE, detector);
        demod.process_real(&audio, &mut whole);

        let mut demod = CpmDemod::real(&params, &filter, TIMING_BW_BURST, RATE, detector);
        let mut ragged = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 7].iter().cycle() {
            if pos >= audio.len() {
                break;
            }
            let end = (pos + len).min(audio.len());
            demod.process_real(&audio[pos..end], &mut ragged);
            pos = end;
        }
        assert_eq!(whole, ragged);
    }

    #[test]
    fn a_silent_audio_channel_produces_finite_symbols() {
        let params = afsk_params();
        let mut demod = CpmDemod::real(
            &params,
            &pulse::rect(40.0, Norm::Area),
            TIMING_BW_BURST,
            RATE,
            RealDetector::Discriminator { centre_hz: 1_700.0 },
        );
        let mut soft = Vec::new();
        demod.process_real(&vec![0.0; 48_000], &mut soft);
        assert!(!soft.is_empty());
        assert!(soft.iter().all(|s| s.is_finite()), "non-finite symbol");
    }

    #[test]
    #[should_panic(expected = "call process_real")]
    fn feeding_iq_to_a_real_construction_is_a_caller_bug() {
        let mut demod = CpmDemod::real(
            &afsk_params(),
            &pulse::rect(40.0, Norm::Area),
            TIMING_BW_BURST,
            RATE,
            RealDetector::Discriminator { centre_hz: 1_700.0 },
        );
        demod.process(&[Complex::new(0.0, 0.0); 16], &mut Vec::new());
    }

    #[test]
    #[should_panic(expected = "call process")]
    fn feeding_audio_to_an_iq_construction_is_a_caller_bug() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        demod.process_real(&[0.0; 16], &mut Vec::new());
    }

    #[test]
    #[should_panic(expected = "two levels")]
    fn a_filterbank_needs_a_two_level_mapping() {
        let params = four_level(1_944.0);
        let _ = CpmDemod::real(
            &params,
            &rx_rrc(),
            TIMING_BW_BURST,
            RATE,
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
        );
    }

    // --- §4.2 hot-path gate --------------------------------------------------------------

    /// Two warm-up blocks (streaming stages carry an inter-block remainder, so the second is
    /// the first whose buffers must fit remainder plus block), then one steady-state call
    /// that may allocate nothing.
    #[test]
    fn complex_process_steady_state_allocates_nothing() {
        let params = four_level(1_944.0);
        let iq = transmit(&params, &symbols(1_200, 0x5eed, 4));
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let mut soft = Vec::with_capacity(iq.len());
        demod.process(&iq, &mut soft);
        soft.clear();
        demod.process(&iq, &mut soft);
        soft.clear();
        crate::ber::perf::assert_no_alloc("CpmDemod::process", || demod.process(&iq, &mut soft));
        assert!(!soft.is_empty(), "the measured call recovered no symbols");
    }

    #[test]
    fn real_process_steady_state_allocates_nothing() {
        let params = afsk_params();
        let audio = afsk_audio(&symbols(300, 0x0dd5, 2));
        let mut demod = CpmDemod::real(
            &params,
            &afsk_rx(),
            TIMING_BW_BURST,
            RATE,
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
        );
        let mut soft = Vec::with_capacity(audio.len());
        demod.process_real(&audio, &mut soft);
        soft.clear();
        demod.process_real(&audio, &mut soft);
        soft.clear();
        crate::ber::perf::assert_no_alloc("CpmDemod::process_real", || {
            demod.process_real(&audio, &mut soft);
        });
        assert!(!soft.is_empty(), "the measured call recovered no symbols");
    }
}
