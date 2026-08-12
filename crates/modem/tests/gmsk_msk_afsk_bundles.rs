//! §5 bundles for three discriminator-tier CPM catalog rows (MODEM-PLAN §6 CPM rows 2–4):
//! **GMSK/GFSK** (BT ∈ {0.3, 0.5}, h = ½), **MSK** (the LREC(1) h = ½ case), and
//! **audio-domain AFSK** (Bell-202-like 1200/2200 Hz at 1200 baud, real-valued input through
//! both of the engine's [`RealDetector`] options). Every chain is `cpm::CpmMod` →
//! calibrated `ber::impair` channel → `cpm::CpmDemod` — the library's own modulator drives
//! its own demodulator (§1.2), and no protocol is attached (the bundles gate the *entries*).
//!
//! Committed artifacts live in `baselines/cpm/` (prefixed `gmsk_`/`msk_`/`afsk_`), written by
//! the `--ignored` full-measurement tests in `--release` and guarded by the always-run smoke
//! tests here, exactly the `dmr_baseline.rs` pattern:
//!
//! - `gmsk_bt03_awgn.json`, `gmsk_bt05_awgn.json`, `msk_awgn.json`,
//!   `afsk_filterbank_awgn.json`, `afsk_discriminator_awgn.json` — committed reference BER
//!   curves (§4.1 commit-and-guard: no closed form exists for partial-response CPM through a
//!   discriminator).
//! - `gmsk_limits.json`, `msk_limits.json`, `afsk_limits.json` — §4.3 resistance tables at
//!   each entry's reference configuration, under the *default* criterion (these chains reach
//!   1e-3 cleanly, unlike the phase-0 DMR chain that needed an override). GMSK additionally
//!   carries the burst rows — AIS, its flagship consumer, is a burst mode — measured through
//!   the calibrated [`BurstModel`] with per-burst [`KnownSymbols`] anchoring (§3.4).
//! - `gmsk_perf.json`, `msk_perf.json`, `afsk_perf.json` — §4.2 throughput baselines.
//!
//! **Receiver front end** (part of each committed chain, stated in every curve label): GMSK
//! and MSK run behind the same 127-tap, ±6 kHz channel-selection lowpass at 48 kHz — without
//! one the discriminator eats the full sample rate and the waterfall shifts several dB right
//! (the phase-0 DMR baseline's finding), and keeping it *identical* across the two entries is
//! what makes the GMSK-vs-MSK comparison read the pulse shape alone. AFSK needs none: both
//! audio detectors are inherently band-selective.
//!
//! **Sanity comparisons, measured on the committed curves** (gates in the test bodies):
//! GMSK BT = 0.5 costs **+1.34 dB** over plain MSK at BER 1e-3 — partial response stays
//! cheap at BT = 0.5, though a hard-slicing discriminator pays the eye closure the textbook
//! coherent number does not (see `gmsk_bt05_sits_near_msk_at_1e3`); the two AFSK detectors
//! measured against each other put the tone filterbank **2.1 dB ahead** of the analytic
//! discriminator at 1e-3, making it the entry's tier-1 reference
//! (`afsk_filterbank_is_the_tier_one_reference`).
//!
//! Everything is seeded; committed numbers were measured in `--release` and reproduce
//! bit-for-bit on one host. Curve labels state the overhead accounting: preamble, sync and
//! tail symbols are charged to Eb (per-information-bit accounting, §4.1); TDMA dead time is
//! excluded automatically by the noise model measuring the carved waveform's energy.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Nco, design_lowpass};
use sdrmm_modem::{
    ber::{
        Curve,
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{BurstModel, Cfo, ChannelSpec, ClockError, Drift, TimingOffset},
        limits::{self, Criterion, LimitRow, LimitsTable},
        perf::{self, PerfBaseline},
        rng::Rng,
        sweep::{self, Link},
    },
    cpm::{CpmDemod, CpmMod, CpmParams, KnownSymbols, Mapping, RealDetector, TIMING_BW_BURST},
    pulse::{self, Norm},
};

fn baseline_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/cpm/{name}"))
}

// --- The three entries' reference configurations ---------------------------------------------

/// GMSK/MSK reference rate: 48 kHz / 4800 baud, 10 samples per symbol — the D-STAR figures,
/// and the rate the perf baselines' real-time factors divide by. The engine itself is
/// rate-free (everything is sps); the Hz numbers exist so the limits axes read in physical
/// units.
const BAUD: f64 = 4_800.0;
const SPS: f64 = 10.0;
const RATE: f64 = BAUD * SPS;

/// One-sided channel-selection cutoff shared by the GMSK and MSK links. Carson-rule sizing
/// for the wider of the two (MSK/1REC): outer deviation h·baud/2 = 1200 Hz plus one baud of
/// modulation bandwidth. Identical for both entries so the committed BT comparison isolates
/// the frequency pulse, not the noise bandwidth.
const NOISE_BW_HZ: f64 = 6_000.0;
const FRONT_TAPS: usize = 127;

/// Gaussian pulse spans in symbols (`gaussian_freq`'s total-length convention): BT = 0.5
/// decays within 3 symbols; BT = 0.3's longer tails need 4 — the `pulse::cpm` tests' figures.
fn gmsk_span(bt: f64) -> usize {
    if bt < 0.4 { 4 } else { 3 }
}

/// GMSK at the given BT: Gaussian partial-response frequency pulse, h = ½ (D-STAR/Bluetooth
/// BR at BT 0.5, GSM's 3GPP TS 45.004 figure at BT 0.3).
fn gmsk_params(bt: f64) -> CpmParams {
    CpmParams::from_h(
        Mapping::natural(2),
        0.5,
        pulse::gaussian_freq(SPS, bt, gmsk_span(bt), Norm::Area),
        SPS,
    )
}

/// GMSK receive filters, chosen by measurement across six candidates each (rect at 0.5/0.8/
/// 1/1.2/1.5 symbols, the premod Gaussian at the entry's BT, a BT = 0.5 Gaussian, a
/// 0.55-baud lowpass, and the frequency pulse itself):
///
/// - **BT = 0.5: the frequency pulse (rect ⊗ Gaussian) — the matched filter.** Best measured
///   at every point (1e-2 at 10.8 dB vs the premod Gaussian's 12.3; 0.9 dB ahead of it at
///   1e-3), and a clean tail where the premod shape keeps straggler errors to 20 dB.
/// - **BT = 0.3: a BT = 0.5 Gaussian, deliberately *not* matched.** The matched filter's
///   3-symbol ISI closes the inner eye and the tail goes shallow (1e-3 at ~24.5 dB); plain
///   rect keeps a steep tail (1e-3 at ~20 dB) but the unsmoothed ISI feeds the Gardner
///   detector so badly that acquisition fails outright below ~14 dB. The BT = 0.5 smoothing
///   is the measured compromise: acquires from ~12 dB, 1e-3 at ~21 dB. The real fix for
///   BT = 0.3 is the MLSE tier (§7 phase-3 follow-on) — GSM itself never decodes BT = 0.3
///   symbol-by-symbol.
fn gmsk_rx(bt: f64) -> Vec<f32> {
    if bt < 0.4 {
        pulse::gaussian(SPS, 0.5, 3, Norm::Area)
    } else {
        pulse::gaussian_freq(SPS, bt, gmsk_span(bt), Norm::Area)
    }
}

/// MSK as CPM: LREC(1) (rect) frequency pulse at h = ½; integrate-and-dump receive filter.
/// The half-sine amplitude pulse is the same waveform's linear OQPSK reading — that
/// representation belongs to the planned coherent tier, not to this one.
fn msk_params() -> CpmParams {
    CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS)
}

fn msk_rx() -> Vec<f32> {
    pulse::rect(SPS, Norm::Area)
}

/// AFSK reference configuration: Bell-202 tones (mark 1200 Hz = bit 1, space 2200 Hz = bit 0)
/// at 1200 baud, processed at 12 kHz audio rate (10 sps — the rate an APRS-in-NFM audio tap
/// resamples to). Mark sits *below* the 1700 Hz centre, so its level is −1 and the mapping
/// table — not a sign convention — carries the assignment: index 0 → +1 (2200 Hz),
/// index 1 → −1 (1200 Hz).
const AFSK_BAUD: f64 = 1_200.0;
const AFSK_SPS: f64 = 10.0;
const AFSK_RATE: f64 = AFSK_BAUD * AFSK_SPS;
const AFSK_MARK_HZ: f64 = 1_200.0;
const AFSK_SPACE_HZ: f64 = 2_200.0;
const AFSK_CENTRE_HZ: f64 = 1_700.0;

fn afsk_params() -> CpmParams {
    CpmParams::from_deviation(
        Mapping::new(vec![1.0, -1.0]),
        (AFSK_SPACE_HZ - AFSK_MARK_HZ) / 2.0,
        AFSK_BAUD,
        pulse::rect(AFSK_SPS, Norm::Area),
        AFSK_SPS,
    )
}

fn afsk_filterbank() -> RealDetector {
    RealDetector::ToneFilterbank {
        plus_hz: AFSK_SPACE_HZ,
        minus_hz: AFSK_MARK_HZ,
    }
}

fn afsk_discriminator() -> RealDetector {
    RealDetector::Discriminator {
        centre_hz: AFSK_CENTRE_HZ,
    }
}

/// Per-detector receive filter, the demod unit tests' measured judgment restated at 10 sps:
/// the tone correlators already integrate a 1.2-symbol window, so the filterbank takes only a
/// half-symbol smoothing rect; the discriminator has no integration of its own and takes the
/// full-symbol matched rect.
fn afsk_rx(detector: RealDetector) -> Vec<f32> {
    match detector {
        RealDetector::ToneFilterbank { .. } => pulse::rect(AFSK_SPS / 2.0, Norm::Area),
        RealDetector::Discriminator { .. } => pulse::rect(AFSK_SPS, Norm::Area),
    }
}

// --- Shared framing and helpers ---------------------------------------------------------------

/// Steady-frame geometry, shared by every entry: an alternating clock-acquisition preamble, a
/// unique word the receiver aligns on (never assumed — searched, the `dmr_baseline` idiom),
/// the payload, and enough trailing filler that the front end's group delay does not swallow
/// the last payload symbols.
const PREAMBLE: usize = 96;
const TAIL: usize = 24;
const STEADY_BITS: usize = 1024;

/// 24-symbol unique word (0x4F9968 MSB-first), chosen by search for aperiodic
/// autocorrelation: worst shifted-overlap sidelobe 3 of 24, counting the alternating
/// preamble as left context. The property is load-bearing — a first draft used 0xB62B62,
/// whose halves repeat, and one payload in 4096 continued the pattern into a *perfect*
/// 12-shifted anchor (measured: a whole mis-sliced trial at 20 dB Eb/N0).
const UW24: [u8; 24] = [
    0, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0,
];

/// AFSK trials are shorter-framed: the audio detectors lock in tens of symbols (the demod
/// unit tests align within 80), so 64 preamble symbols suffice. The unique word is the same
/// 24-symbol one — a 16-symbol draft put the false-anchor probability at ~2^-16 per payload
/// position, and with ~32 in-window positions per trial the committed curves grew a measured
/// ~1.3e-4 error floor of whole mis-anchored trials at high Eb/N0; 24 symbols and the
/// tighter window below push that below every committed point.
const AFSK_PREAMBLE: usize = 64;
const AFSK_TAIL: usize = 16;
const AFSK_BITS: usize = 1024;

/// AFSK sync search span past the nominal word position: covers the detector group delays
/// (~2 symbols filterbank, ~8 discriminator) and the shift a worst-case sample-clock probe
/// adds by word time (~5 symbols at 50000 ppm), while keeping payload overlap short enough
/// that the word's own sidelobe guard decides every in-window impostor.
const AFSK_SEARCH: usize = 24;

/// Symbol stream of one steady trial: preamble, unique word, payload bits as symbol indices
/// (index = bit for every 2-level mapping here — the mapping table maps index to level), tail.
fn framed_symbols(preamble: usize, uw: &[u8], bits: &[bool], tail: usize) -> Vec<u8> {
    let mut s: Vec<u8> = (0..preamble).map(|i| (i % 2) as u8).collect();
    s.extend_from_slice(uw);
    s.extend(bits.iter().map(|&b| u8::from(b)));
    s.extend((0..tail).map(|i| (i % 2) as u8));
    s
}

fn cpm_wave(params: &CpmParams, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut m = CpmMod::new(params.clone());
    let mut out = Vec::new();
    m.modulate(symbols, &mut out);
    m.flush(&mut out);
    out
}

/// Receiver noise 40 dB below a unit carrier — what the demodulator hears before a
/// transmission, the `fsk4` tests' `listening` convention. The fixed seed is part of the
/// chain definition: every trial's demodulator meets the channel having heard the same quiet.
const WARMUP_SYMBOLS: usize = 500;

fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

fn real_quiet(seed: u64, len: usize) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| ((rng.uniform() * 2.0 - 1.0) * 0.01) as f32)
        .collect()
}

/// The complex-entry receive chain, fresh per trial so every trial reproduces from its own
/// seed alone: channel-selection lowpass → `CpmDemod` at the burst operating point (these
/// entries are burst-capable, and both bandwidths were measured curve-identical on these
/// ~1200-symbol trials — far below the length where the continuous-mode self-noise walk
/// matters).
fn steady_soft(params: &CpmParams, rx: &[f32], wave: &[Complex<f32>]) -> Vec<f32> {
    let front = design_lowpass(FRONT_TAPS, NOISE_BW_HZ / RATE);
    let mut filter = Decimator::new(&front, 1);
    let mut demod = CpmDemod::new(params, rx, TIMING_BW_BURST);
    let mut filtered = Vec::new();
    let mut discard = Vec::new();
    filter.process(&quiet(0x1157, WARMUP_SYMBOLS * SPS as usize), &mut filtered);
    demod.process(&filtered, &mut discard);
    let mut soft = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut soft);
    soft
}

/// The AFSK receive chain: the constructed detector *is* the front end (both options are
/// band-selective), and the audio the demodulator hears is the waveform's real part — see
/// [`afsk_link`] for why the harness carries the analytic signal.
fn afsk_soft(detector: RealDetector, wave: &[Complex<f32>]) -> Vec<f32> {
    let params = afsk_params();
    let mut demod = CpmDemod::real(
        &params,
        &afsk_rx(detector),
        TIMING_BW_BURST,
        AFSK_RATE,
        detector,
    );
    let mut discard = Vec::new();
    demod.process_real(
        &real_quiet(0x1157, WARMUP_SYMBOLS * AFSK_SPS as usize),
        &mut discard,
    );
    let audio: Vec<f32> = wave.iter().map(|s| s.re).collect();
    let mut soft = Vec::new();
    demod.process_real(&audio, &mut soft);
    soft
}

/// The unique word as transmitted levels — what the soft symbols are correlated against.
fn uw_levels(params: &CpmParams, uw: &[u8]) -> Vec<f32> {
    uw.iter().map(|&s| params.mapping().level(s)).collect()
}

/// Best sync position in `lo..=hi` by Euclidean distance of the *soft* symbols to the word's
/// transmitted levels — the searched-alignment idiom, taken soft because a hard-sliced
/// Hamming match throws away exactly the confidence that separates the true position from an
/// ISI-corrupted neighbour, and as a distance rather than a bare correlation because a dot
/// product rewards overshooting symbols and was measured mis-anchoring whole trials. No
/// threshold: a chain too degraded to place its sync scores its garbage as bit errors.
fn find_uw(soft: &[f32], lo: usize, hi: usize, levels: &[f32]) -> Option<usize> {
    let last = hi.min(soft.len().checked_sub(levels.len())?);
    let misfit = |at: usize| -> f32 {
        levels
            .iter()
            .enumerate()
            .map(|(i, &l)| (soft[at + i] - l) * (soft[at + i] - l))
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

/// Payload bits behind a located unique word, missing symbols counting as errors upstream
/// (a short read returns fewer bits and the sweep charges the difference).
fn payload_bits(params: &CpmParams, soft: &[f32], at: usize, uw_len: usize, n: usize) -> Vec<bool> {
    (0..n)
        .map(|k| {
            soft.get(at + uw_len + k)
                .is_some_and(|&s| params.mapping().slice(s) == 1)
        })
        .collect()
}

// --- The steady links (the committed reference chains) ---------------------------------------

/// One steady complex-entry link: bits → framed symbols → `CpmMod` → (channel) → lowpass →
/// `CpmDemod` → slice → align on the unique word → payload bits.
fn steady_link(label: &str, params: CpmParams, rx: Vec<f32>) -> Link {
    let mod_params = params.clone();
    Link {
        label: label.to_string(),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(move |bits| {
            cpm_wave(&mod_params, &framed_symbols(PREAMBLE, &UW24, bits, TAIL))
        }),
        demodulate: Box::new(move |wave| {
            let soft = steady_soft(&params, &rx, wave);
            let levels = uw_levels(&params, &UW24);
            let Some(at) = find_uw(&soft, PREAMBLE, PREAMBLE + 48, &levels) else {
                return Vec::new();
            };
            payload_bits(&params, &soft, at, UW24.len(), STEADY_BITS)
        }),
    }
}

fn gmsk_link(bt: f64) -> Link {
    let rx_name = if bt < 0.4 {
        "gaussian-BT0.5 rx"
    } else {
        "pulse-matched rx"
    };
    steady_link(
        &format!(
            "gmsk BT={bt} h=0.5 uncoded, CpmMod -> +/-6 kHz front lowpass -> CpmDemod \
             ({rx_name}, timing bw 0.015), 48 kHz 4800 baud, 96+24+24 symbol overhead \
             in Eb, release"
        ),
        gmsk_params(bt),
        gmsk_rx(bt),
    )
}

fn msk_link() -> Link {
    steady_link(
        "msk (1REC h=0.5) uncoded, CpmMod -> +/-6 kHz front lowpass -> CpmDemod \
         (integrate-and-dump rx, timing bw 0.015), 48 kHz 4800 baud, 96+24+24 symbol \
         overhead in Eb, release",
        msk_params(),
        msk_rx(),
    )
}

/// The AFSK link. The harness's impairments and Eb/N0 accounting live on complex waveforms,
/// so the link carries the *analytic* audio (baseband CPFSK shifted onto the 1700 Hz
/// subcarrier — positive frequencies only) and the demodulator hears its real part. The
/// projection halves signal power and per-sample noise power together (Re of unit-magnitude
/// analytic carries ½; complex noise of per-component σ² projects to real variance σ²), so
/// the stated Eb/N0 is the ratio at the detector.
fn afsk_link(label: &str, detector: RealDetector) -> Link {
    let params = afsk_params();
    Link {
        label: label.to_string(),
        bits_per_trial: AFSK_BITS,
        modulate: Box::new(move |bits| {
            let baseband = cpm_wave(
                &afsk_params(),
                &framed_symbols(AFSK_PREAMBLE, &UW24, bits, AFSK_TAIL),
            );
            let mut carrier = Nco::new(AFSK_CENTRE_HZ as f32, AFSK_RATE as f32);
            baseband
                .iter()
                .map(|&s| s * carrier.next_sample())
                .collect()
        }),
        demodulate: Box::new(move |wave| {
            let soft = afsk_soft(detector, wave);
            let levels = uw_levels(&params, &UW24);
            let Some(at) = find_uw(&soft, AFSK_PREAMBLE, AFSK_PREAMBLE + AFSK_SEARCH, &levels)
            else {
                return Vec::new();
            };
            payload_bits(&params, &soft, at, UW24.len(), AFSK_BITS)
        }),
    }
}

fn afsk_filterbank_link() -> Link {
    afsk_link(
        "afsk 1200/2200 Hz 1200 baud uncoded, CpmMod on 1700 Hz subcarrier -> CpmDemod::real \
         tone filterbank (half-symbol rx rect, timing bw 0.015), 12 kHz audio, 64+24+16 \
         symbol overhead in Eb, analytic-signal accounting, release",
        afsk_filterbank(),
    )
}

fn afsk_discriminator_link() -> Link {
    afsk_link(
        "afsk 1200/2200 Hz 1200 baud uncoded, CpmMod on 1700 Hz subcarrier -> CpmDemod::real \
         analytic discriminator (full-symbol rx rect, timing bw 0.015), 12 kHz audio, \
         64+24+16 symbol overhead in Eb, analytic-signal accounting, release",
        afsk_discriminator(),
    )
}

// --- The GMSK burst chain (AIS-shaped; feeds the limits table's burst rows) -------------------

/// One parameterisation of the GMSK burst chain: BT = 0.5 content of 24 sync + payload
/// symbols radiated per frame, the rest dead — an AIS-shaped 256-symbol frame by default.
/// The limits axes vary one field each.
#[derive(Clone, Copy)]
struct GmskBurstRecipe {
    payload_symbols: usize,
    off_symbols: usize,
    payload_frames: usize,
    /// `BurstModel` level step applied to alternate bursts; negative attenuates.
    level_step_db: f64,
}

/// Samples of dead air ahead of the first burst, rounded up to whole frames, so the gate's
/// floor estimate (3840-sample settle window at 10 sps) has settled on the channel's true
/// noise before any burst.
const BURST_LEAD_SAMPLES: usize = 12_000;

impl GmskBurstRecipe {
    fn reference(payload_frames: usize) -> Self {
        Self {
            payload_symbols: 128,
            off_symbols: 104,
            payload_frames,
            level_step_db: 0.0,
        }
    }

    fn content(&self) -> usize {
        UW24.len() + self.payload_symbols
    }

    fn frame_symbols(&self) -> usize {
        self.content() + self.off_symbols
    }

    /// The radiated window per frame: the content symbols plus the full frequency-pulse tail,
    /// so keying never robs the receive filter of the pulse shape it is built around. The
    /// one-symbol keying ramps live inside it.
    fn on_samples(&self) -> usize {
        self.content() * SPS as usize + gmsk_params(0.5).freq_pulse().len()
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS as usize)
    }

    fn bits(&self) -> usize {
        self.payload_symbols * self.payload_frames
    }

    /// The full symbol stream: alternating filler everywhere (the exciter keeps shaping
    /// through the dead time; `BurstModel` does the carving, exactly as the phase-0 DMR burst
    /// baseline radiated), content windows of frames 1..=payload_frames overwritten with
    /// sync + payload. Frame 0's radiated content is the clock-acquisition preamble.
    fn symbols(&self, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        let mut symbols: Vec<u8> = (0..frame * self.payload_frames + self.content())
            .map(|i| (i % 2) as u8)
            .collect();
        for p in 0..self.payload_frames {
            let base = frame * (p + 1);
            symbols[base..base + UW24.len()].copy_from_slice(&UW24);
            for k in 0..self.payload_symbols {
                symbols[base + UW24.len() + k] = u8::from(payload[p * self.payload_symbols + k]);
            }
        }
        symbols
    }

    /// The impairment template carrying this recipe's TDMA carving (one-symbol keying ramps,
    /// receiver noise floor 40 dB down in the gaps); the sweep owns AWGN, applied after the
    /// carving so dead time is excluded from Eb automatically.
    fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS as usize;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS as usize,
            self.level_step_db,
            40.0,
        ))
    }

    fn link(&self, label: &str) -> Link {
        let recipe = *self;
        let demod_recipe = *self;
        Link {
            label: label.to_string(),
            bits_per_trial: self.bits(),
            modulate: Box::new(move |bits| {
                let mut wave = vec![
                    Complex::default();
                    recipe.lead_frames() * recipe.frame_symbols() * SPS as usize
                ];
                wave.extend(cpm_wave(&gmsk_params(0.5), &recipe.symbols(bits)));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    /// Per-burst decoding, the way a burst protocol runs the engine: locate each burst's sync
    /// (searched wide on the first, tracked locally after), anchor the §3.4 known-symbol hook
    /// on it, slice the payload through the correction. No warm-up quiet: the dead-air lead
    /// is inside the waveform so the AWGN axis covers it and the gate measures its floor from
    /// the channel it will actually gate.
    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let params = gmsk_params(0.5);
        let front = design_lowpass(FRONT_TAPS, NOISE_BW_HZ / RATE);
        let mut filter = Decimator::new(&front, 1);
        let mut demod = CpmDemod::new(&params, &gmsk_rx(0.5), TIMING_BW_BURST);
        let mut filtered = Vec::new();
        filter.process(wave, &mut filtered);
        let mut soft = Vec::new();
        demod.process(&filtered, &mut soft);
        let levels = uw_levels(&params, &UW24);

        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut hook = KnownSymbols::new(&params, (4 * frame) as u32);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for p in 0..self.payload_frames {
            let expect = lead + frame * (p + 1);
            let (lo, hi) = if p == 0 {
                (expect, expect + 48)
            } else {
                ((expect + delay).saturating_sub(4), expect + delay + 4)
            };
            let at = find_uw(&soft, lo, hi, &levels);
            if let Some(at) = at {
                delay = at.saturating_sub(expect);
                if at + UW24.len() <= soft.len() {
                    hook.anchor(&UW24, &soft[at..at + UW24.len()]);
                }
            }
            for k in 0..self.payload_symbols {
                hook.tick();
                let bit = at
                    .and_then(|at| soft.get(at + UW24.len() + k))
                    .is_some_and(|&s| params.mapping().slice(hook.correct(s)) == 1);
                bits.push(bit);
            }
        }
        bits
    }
}

/// Frames per burst probe: enough payload to amortise the acquisition frame, short enough
/// that a bisection of seeded probes stays fast.
const BURST_FRAMES: usize = 6;

// --- Committed grids, seeds and budgets -------------------------------------------------------

/// Sweep grids covering each chain's waterfall through BER 1e-4, set from the ignored
/// `probe_grids` exploration and pinned by the committed curves. BT = 0.3's grid sits an
/// octave higher and runs shallower — the discriminator tier's ISI penalty at that BT, see
/// [`gmsk_rx`].
const GMSK05_GRID: [f64; 10] = [8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0];
const GMSK03_GRID: [f64; 15] = [
    12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0,
];
const MSK_GRID: [f64; 9] = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
const AFSK_GRID: [f64; 12] = [
    7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];

const GMSK03_SEED: u64 = 0x63a3;
const GMSK05_SEED: u64 = 0x63a5;
const MSK_SEED: u64 = 0x635b;
const AFSK_FB_SEED: u64 = 0xafb1;
const AFSK_DISC_SEED: u64 = 0xafd1;

/// Error budget of the committed curves. Errors arrive in two populations — a steady trickle
/// and rare whole-trial failures when a low-SNR trial mis-anchors — so the budget is set by
/// the heavy tail: 2000 errors keeps a shoulder point's realisation from being one failed
/// trial's, and the cap still gives the 1e-4 points ~400 errors.
const FULL_ERRORS: u64 = 2_000;
const FULL_CAP: u64 = 4_000_000;

/// Axis-probe budget for the limits searches: 100 errors resolves pass/fail around the 1e-2
/// criterion unambiguously (a failing probe collects them in a few thousand bits; a passing
/// probe runs to the cap at ~1e-4 and reads far below the limit).
const PROBE_ERRORS: u64 = 100;
const PROBE_CAP: u64 = 30_000;

// --- Limits tables ----------------------------------------------------------------------------

fn axis_row(
    axis: &str,
    unit: &str,
    max_axis: f64,
    tolerance: f64,
    ber_at: impl Fn(f64) -> f64,
) -> LimitRow {
    limits::measure_axis_row(
        axis,
        unit,
        Criterion::FailureBer,
        max_axis,
        tolerance,
        ber_at,
    )
}

/// One seeded probe at the operating point (common random numbers per axis: the same seed at
/// every axis value, so the search boundary is a property of the axis, not of noise luck).
fn probe(link: &Link, spec: &ChannelSpec, op_db: f64, seed: u64) -> f64 {
    limits::measure_ber(link, spec, op_db, seed, PROBE_ERRORS, PROBE_CAP)
}

/// The four tracking axes every entry measures (§4.3), on the entry's steady link.
fn steady_axis_rows(link: &Link, rate: f64, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 3_000.0, 25.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, rate)),
                op_db,
                seed,
            )
        }),
        axis_row("frequency drift", "Hz/s", 20_000.0, 200.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, rate)),
                op_db,
                seed,
            )
        }),
        axis_row("sample clock", "ppm", 50_000.0, 500.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        axis_row("static timing offset", "samples", 10.0, 0.25, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
                seed,
            )
        }),
    ]
}

/// The GMSK burst rows (§4.3 burst survival; AIS is burst). Each axis varies the recipe
/// itself, so every probe rebuilds the link — the searched value reshapes the transmitter
/// and the accounting together, not just an impairment knob.
fn gmsk_burst_axis_rows(op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("dead time", "symbols", 1_024.0, 16.0, |off| {
            let mut recipe = GmskBurstRecipe::reference(BURST_FRAMES);
            recipe.off_symbols = (off.round() as usize).max(16);
            let link = recipe.link("dead-time probe");
            probe(&link, &recipe.channel(), op_db, seed)
        }),
        // "Minimum burst length" spelled so higher stays better for the comparator: payload
        // symbols removable from the 128-symbol burst; min burst = 24-symbol sync + rest.
        axis_row(
            "burst shortening",
            "payload symbols removed (of 128)",
            112.0,
            2.0,
            |removed| {
                let mut recipe = GmskBurstRecipe::reference(BURST_FRAMES);
                recipe.payload_symbols = 128 - (removed.round() as usize).min(112);
                let link = recipe.link("burst-length probe");
                probe(&link, &recipe.channel(), op_db, seed)
            },
        ),
        // Attenuation of alternate bursts: the decay-limited direction of the level tracker,
        // recovered (or not) within each burst's own sync via the known-symbol hook.
        axis_row(
            "level step",
            "dB attenuation of alternate bursts",
            12.0,
            0.25,
            |db| {
                let mut recipe = GmskBurstRecipe::reference(BURST_FRAMES);
                recipe.level_step_db = -db;
                let link = recipe.link("level-step probe");
                probe(&link, &recipe.channel(), op_db, seed)
            },
        ),
    ]
}

/// Operating point off a committed (or freshly measured) sensitivity curve: the §4.3 default,
/// sensitivity(1e-3) + 3 dB.
fn operating_point(curve: &Curve) -> f64 {
    limits::ebn0_at_ber(curve, 1e-3).expect("the grid must bracket BER 1e-3") + 3.0
}

// --- Always-run harness canaries --------------------------------------------------------------

/// A chain defect (alignment, sign, level scale) is loud before any statistics: near-clean
/// channels, one trial each, essentially error-free.
#[test]
fn every_chain_round_trips_near_clean_at_high_ebn0() {
    for (link, template, name) in [
        (gmsk_link(0.3), ChannelSpec::default(), "gmsk bt=0.3"),
        (gmsk_link(0.5), ChannelSpec::default(), "gmsk bt=0.5"),
        (msk_link(), ChannelSpec::default(), "msk"),
        (afsk_filterbank_link(), ChannelSpec::default(), "afsk fb"),
        (
            afsk_discriminator_link(),
            ChannelSpec::default(),
            "afsk disc",
        ),
        (
            GmskBurstRecipe::reference(BURST_FRAMES).link("gmsk burst"),
            GmskBurstRecipe::reference(BURST_FRAMES).channel(),
            "gmsk burst",
        ),
    ] {
        let ber = limits::measure_ber(&link, &template, 30.0, 0x0c1e, 1, 1);
        assert!(ber < 3e-3, "{name} floor {ber} at 30 dB Eb/N0");
    }
}

// --- Always-run smoke guards of the committed curves ------------------------------------------

fn smoke_curve(link: &Link, grid: &[f64], seed: u64, name: &str) {
    let committed = sweep::load_json(&baseline_path(name)).unwrap();
    let measured = sweep::sweep_ber(
        link,
        &ChannelSpec::default(),
        &grid[..3],
        seed,
        FULL_ERRORS,
        FULL_CAP,
    );
    let worst = sweep::worst_penalty_db_vs_curve(&measured, &committed, grid[0], grid[2]);
    assert!(worst.abs() < 0.5, "{name} drift vs committed: {worst} dB");
}

/// Smoke tier of the committed curves: the first three grid points re-measured with the
/// committed budgets. A sweep point's realisation is named by (seed, grid index), so a grid
/// prefix reproduces the committed points exactly — bit-identical on one host — and the
/// 0.5 dB slack only absorbs cross-platform float drift.
#[test]
fn gmsk_curves_match_committed_baselines() {
    smoke_curve(
        &gmsk_link(0.3),
        &GMSK03_GRID,
        GMSK03_SEED,
        "gmsk_bt03_awgn.json",
    );
    smoke_curve(
        &gmsk_link(0.5),
        &GMSK05_GRID,
        GMSK05_SEED,
        "gmsk_bt05_awgn.json",
    );
}

#[test]
fn msk_curve_matches_committed_baseline() {
    smoke_curve(&msk_link(), &MSK_GRID, MSK_SEED, "msk_awgn.json");
}

#[test]
fn afsk_curves_match_committed_baselines() {
    smoke_curve(
        &afsk_filterbank_link(),
        &AFSK_GRID,
        AFSK_FB_SEED,
        "afsk_filterbank_awgn.json",
    );
    smoke_curve(
        &afsk_discriminator_link(),
        &AFSK_GRID,
        AFSK_DISC_SEED,
        "afsk_discriminator_awgn.json",
    );
}

// --- The task-stated sanity comparisons -------------------------------------------------------

/// Partial response costs little at BT = 0.5: the committed GMSK BT = 0.5 curve sits near
/// plain MSK at BER 1e-3 (same h, same front lowpass, same framing — the comparison reads
/// the frequency pulse alone). **Measured: +1.34 dB.** That is past the coherent-tier
/// textbook fraction-of-a-dB because a hard-slicing discriminator pays BT = 0.5's eye
/// closure in full where a matched coherent receiver would not — the gate bounds the
/// committed number with room only for counting noise, so the distance cannot quietly grow.
#[test]
fn gmsk_bt05_sits_near_msk_at_1e3() {
    let gmsk = sweep::load_json(&baseline_path("gmsk_bt05_awgn.json")).unwrap();
    let msk = sweep::load_json(&baseline_path("msk_awgn.json")).unwrap();
    let penalty = sweep::penalty_db_vs_curve(&gmsk, &msk, 1e-3);
    println!("GMSK BT=0.5 vs MSK at BER 1e-3: {penalty:+.3} dB");
    assert!(
        (0.0..1.6).contains(&penalty),
        "GMSK BT=0.5 is {penalty} dB from MSK at 1e-3 (committed: +1.34 dB)"
    );
}

/// The two AFSK detector options against each other, on their committed curves: the tone
/// filterbank is the tier-1 reference — **measured 2.1 dB ahead** of the analytic
/// discriminator at BER 1e-3 (the two correlators integrate exactly the tone split the
/// discriminator's click noise smears below the FM threshold). The gate only demands it not
/// fall behind, so a detector improvement on either side cannot fail it.
#[test]
fn afsk_filterbank_is_the_tier_one_reference() {
    let fb = sweep::load_json(&baseline_path("afsk_filterbank_awgn.json")).unwrap();
    let disc = sweep::load_json(&baseline_path("afsk_discriminator_awgn.json")).unwrap();
    let penalty = sweep::penalty_db_vs_curve(&fb, &disc, 1e-3);
    println!("AFSK filterbank vs discriminator at BER 1e-3: {penalty:+.3} dB");
    assert!(
        penalty < 0.25,
        "the filterbank fell {penalty} dB behind the discriminator; the tier-1 choice no \
         longer holds"
    );
}

// --- Always-run smoke guards of the limits tables ---------------------------------------------

/// One-sided row comparison with the committed table: moving better is never a failure; a
/// vanished row or a changed unit/criterion is.
fn compare_rows(measured: &[LimitRow], committed: &LimitsTable, name: &str) {
    let mut faults = Vec::new();
    for row in &committed.rows {
        let Some(m) = measured.iter().find(|m| m.axis == row.axis) else {
            faults.push(format!("row '{}' vanished", row.axis));
            continue;
        };
        assert_eq!(
            m.criterion, row.criterion,
            "criterion changed on '{}'",
            row.axis
        );
        assert_eq!(m.unit, row.unit, "unit changed on '{}'", row.axis);
        let worse_by = row.threshold - m.threshold;
        if m.threshold.is_nan() || worse_by > 0.2 * row.threshold.abs() {
            faults.push(format!(
                "row '{}': committed {} -> measured {} {}",
                row.axis, row.threshold, m.threshold, m.unit
            ));
        }
    }
    assert!(faults.is_empty(), "{name} limits regressions: {faults:#?}");
}

/// The limits smoke reads the operating point off the committed curve (parameter-identical to
/// the table's own sensitivity sweep) so it does not pay for a resweep; the curve smoke tests
/// above guard that number.
#[test]
fn gmsk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path("gmsk_limits.json")).unwrap();
    let curve = sweep::load_json(&baseline_path("gmsk_bt05_awgn.json")).unwrap();
    let op_db = operating_point(&curve);
    let link = gmsk_link(0.5);
    let mut measured = steady_axis_rows(&link, RATE, op_db, GMSK05_SEED ^ 0xbe5);
    measured.extend(gmsk_burst_axis_rows(op_db, GMSK05_SEED ^ 0xbe5));
    compare_rows(&measured, &committed, "gmsk");
}

#[test]
fn msk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path("msk_limits.json")).unwrap();
    let curve = sweep::load_json(&baseline_path("msk_awgn.json")).unwrap();
    let op_db = operating_point(&curve);
    let link = msk_link();
    let measured = steady_axis_rows(&link, RATE, op_db, MSK_SEED ^ 0xbe5);
    compare_rows(&measured, &committed, "msk");
}

#[test]
fn afsk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path("afsk_limits.json")).unwrap();
    let curve = sweep::load_json(&baseline_path("afsk_filterbank_awgn.json")).unwrap();
    let op_db = operating_point(&curve);
    let link = afsk_filterbank_link();
    let measured = afsk_axis_rows(&link, op_db, AFSK_FB_SEED ^ 0xbe5);
    compare_rows(&measured, &committed, "afsk");
}

/// AFSK's tracking axes at its own rate and brackets (the tones are 1000 Hz apart, so the
/// CFO axis lives an order of magnitude below the RF entries').
fn afsk_axis_rows(link: &Link, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 500.0, 5.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, AFSK_RATE)),
                op_db,
                seed,
            )
        }),
        axis_row("frequency drift", "Hz/s", 2_000.0, 25.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, AFSK_RATE)),
                op_db,
                seed,
            )
        }),
        axis_row("sample clock", "ppm", 50_000.0, 500.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        axis_row("static timing offset", "samples", 10.0, 0.25, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
                seed,
            )
        }),
    ]
}

// --- Level-1 E2E (§4.4): payload in equals payload out at a stated margin ---------------------

/// +6 dB over the committed 1e-3 sensitivity: residual BER is off the bottom of the measured
/// waterfall (≲1e-7), so a handful of payloads carries ≪1 expected errors and the fixed seed
/// makes the outcome a fact of the entry.
const E2E_MARGIN_DB: f64 = 6.0;
const E2E_PAYLOADS: usize = 6;

fn e2e(mut link: Link, curve_name: &str, seed: u64) {
    let committed = sweep::load_json(&baseline_path(curve_name)).unwrap();
    let sensitivity = limits::ebn0_at_ber(&committed, 1e-3).unwrap();
    let payloads = Payloads::new(seed, E2E_PAYLOADS, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, E2E_MARGIN_DB);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn gmsk_loops_back_clean_at_margin() {
    e2e(gmsk_link(0.5), "gmsk_bt05_awgn.json", 0x0e2e_63a5);
    e2e(gmsk_link(0.3), "gmsk_bt03_awgn.json", 0x0e2e_63a3);
}

#[test]
fn msk_loops_back_clean_at_margin() {
    e2e(msk_link(), "msk_awgn.json", 0x0e2e_635b);
}

#[test]
fn afsk_loops_back_clean_at_margin_through_both_detectors() {
    e2e(
        afsk_filterbank_link(),
        "afsk_filterbank_awgn.json",
        0x0e2e_afb1,
    );
    e2e(
        afsk_discriminator_link(),
        "afsk_discriminator_awgn.json",
        0x0e2e_afd1,
    );
}

// --- §4.2 perf baselines (same #[ignore] writer pattern as ber::perf) -------------------------

/// Warmed-up throughput of one entry's steady-state `process` path, per the ber::perf
/// convention: two warm-up calls so the buffers hold their steady capacity, then the
/// measured iterations.
/// 2-level bench symbols from the shared dibit generator — the same stream `benches/perf.rs`
/// modulates, so the criterion bench and the committed number measure the same work.
fn bench_bits(len: usize, seed: u32) -> Vec<u8> {
    perf::test_dibits(len, seed)
        .into_iter()
        .map(|d| d & 1)
        .collect()
}

fn measured_gmsk_perf() -> Vec<PerfBaseline> {
    let params = gmsk_params(0.5);
    let iq = cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut demod = CpmDemod::new(&params, &gmsk_rx(0.5), TIMING_BW_BURST);
    let mut soft = Vec::with_capacity(iq.len());
    demod.process(&iq, &mut soft);
    soft.clear();
    demod.process(&iq, &mut soft);
    let msps = perf::measure_throughput(300, iq.len() as u64, || {
        soft.clear();
        demod.process(&iq, &mut soft);
    });
    vec![PerfBaseline {
        bench: "gmsk_bt05_demod".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / RATE,
        config: "GMSK BT=0.5 h=0.5, 10 sps, pulse-matched rx, timing bw 0.015".into(),
        host: perf::host_id(),
    }]
}

fn measured_msk_perf() -> Vec<PerfBaseline> {
    let params = msk_params();
    let iq = cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut demod = CpmDemod::new(&params, &msk_rx(), TIMING_BW_BURST);
    let mut soft = Vec::with_capacity(iq.len());
    demod.process(&iq, &mut soft);
    soft.clear();
    demod.process(&iq, &mut soft);
    let msps = perf::measure_throughput(300, iq.len() as u64, || {
        soft.clear();
        demod.process(&iq, &mut soft);
    });
    vec![PerfBaseline {
        bench: "msk_demod".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / RATE,
        config: "MSK (1REC h=0.5), 10 sps, integrate-and-dump rx, timing bw 0.015".into(),
        host: perf::host_id(),
    }]
}

fn measured_afsk_perf() -> Vec<PerfBaseline> {
    let params = afsk_params();
    let baseband = cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut carrier = Nco::new(AFSK_CENTRE_HZ as f32, AFSK_RATE as f32);
    let audio: Vec<f32> = baseband
        .iter()
        .map(|&s| (s * carrier.next_sample()).re)
        .collect();
    let detector = afsk_filterbank();
    let mut demod = CpmDemod::real(
        &params,
        &afsk_rx(detector),
        TIMING_BW_BURST,
        AFSK_RATE,
        detector,
    );
    let mut soft = Vec::with_capacity(audio.len());
    demod.process_real(&audio, &mut soft);
    soft.clear();
    demod.process_real(&audio, &mut soft);
    let msps = perf::measure_throughput(300, audio.len() as u64, || {
        soft.clear();
        demod.process_real(&audio, &mut soft);
    });
    vec![PerfBaseline {
        bench: "afsk_filterbank_12k".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / AFSK_RATE,
        config: "AFSK 1200/2200 Hz tone filterbank, 12 kHz, 10 sps, half-symbol rx rect".into(),
        host: perf::host_id(),
    }]
}

fn write_perf(name: &str, measured: &[PerfBaseline]) {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = baseline_path(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    perf::save_baselines(&path, measured).unwrap();
}

fn compare_perf(name: &str, measured: &[PerfBaseline]) {
    let committed = perf::load_baselines(&baseline_path(name)).unwrap();
    if committed.iter().any(|b| b.host != perf::host_id()) {
        eprintln!(
            "skipping the perf gate: baseline host is not {}",
            perf::host_id()
        );
        return;
    }
    match perf::compare_perf(measured, &committed, perf::REGRESSION_FRACTION) {
        Ok(changes) => {
            for c in changes {
                eprintln!(
                    "{}: {:+.1}% vs baseline ({:.1} -> {:.1} Msamples/s)",
                    c.bench,
                    100.0 * c.change_fraction,
                    c.committed_msamples_per_s,
                    c.measured_msamples_per_s
                );
            }
        }
        Err(regressions) => panic!(
            "{name} throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * perf::REGRESSION_FRACTION
        ),
    }
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_gmsk_perf_baseline() {
    write_perf("gmsk_perf.json", &measured_gmsk_perf());
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_msk_perf_baseline() {
    write_perf("msk_perf.json", &measured_msk_perf());
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_afsk_perf_baseline() {
    write_perf("afsk_perf.json", &measured_afsk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_gmsk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf("gmsk_perf.json", &measured_gmsk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_msk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf("msk_perf.json", &measured_msk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_afsk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf("afsk_perf.json", &measured_afsk_perf());
}

// --- Full re-measurement (nightly; regenerates the committed artifacts) -----------------------

/// Writes the curve when its artifact is missing; asserts point-by-point reproduction when it
/// exists (same seeds and budgets make each point a reproduction of the committed one; the
/// ratio allowance absorbs cross-host float drift, nothing else). A superseding chain gets a
/// NEW artifact name — committed files are never regenerated in place (MODEM-PLAN §8).
fn remeasure_curve(link: &Link, template: &ChannelSpec, grid: &[f64], seed: u64, name: &str) {
    let curve = sweep::sweep_ber(link, template, grid, seed, FULL_ERRORS, FULL_CAP);
    for p in &curve.points {
        println!(
            "{:>5.1} dB  {:>7} / {:<9} BER {:.3e}",
            p.ebn0_db,
            p.errors,
            p.trials,
            p.rate()
        );
    }
    let path = baseline_path(name);
    if path.exists() {
        let committed: Curve = sweep::load_json(&path).unwrap();
        assert_eq!(
            curve.points.len(),
            committed.points.len(),
            "{name}: grid changed"
        );
        for (m, c) in curve.points.iter().zip(&committed.points) {
            assert!((m.ebn0_db - c.ebn0_db).abs() < 1e-9, "{name}: grid changed");
            let ratio = (m.rate().max(1e-12) / c.rate().max(1e-12)).log10().abs();
            assert!(
                ratio < 0.1,
                "{name} at {} dB: committed BER {:.3e}, measured {:.3e}",
                c.ebn0_db,
                c.rate(),
                m.rate()
            );
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        sweep::save_json(&curve, &path).unwrap();
        println!("baseline created at {}", path.display());
    }
}

/// Run in release: `cargo test -p sdrmm-modem --release --test gmsk_msk_afsk_bundles -- --ignored`.
#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_gmsk_curves_full() {
    remeasure_curve(
        &gmsk_link(0.3),
        &ChannelSpec::default(),
        &GMSK03_GRID,
        GMSK03_SEED,
        "gmsk_bt03_awgn.json",
    );
    remeasure_curve(
        &gmsk_link(0.5),
        &ChannelSpec::default(),
        &GMSK05_GRID,
        GMSK05_SEED,
        "gmsk_bt05_awgn.json",
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_msk_curve_full() {
    remeasure_curve(
        &msk_link(),
        &ChannelSpec::default(),
        &MSK_GRID,
        MSK_SEED,
        "msk_awgn.json",
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_afsk_curves_full() {
    remeasure_curve(
        &afsk_filterbank_link(),
        &ChannelSpec::default(),
        &AFSK_GRID,
        AFSK_FB_SEED,
        "afsk_filterbank_awgn.json",
    );
    remeasure_curve(
        &afsk_discriminator_link(),
        &ChannelSpec::default(),
        &AFSK_GRID,
        AFSK_DISC_SEED,
        "afsk_discriminator_awgn.json",
    );
}

fn write_or_check_limits(table: &LimitsTable, name: &str) {
    println!(
        "{name}: sensitivity 1e-2 {:?}  1e-3 {:?}  1e-4 {:?}",
        table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
    );
    for row in &table.rows {
        println!("{:<24} {:>12.4} {}", row.axis, row.threshold, row.unit);
    }
    let path = baseline_path(name);
    if path.exists() {
        let committed = limits::load_json(&path).unwrap();
        if let Err(faults) = limits::compare_tables(table, &committed, 0.2) {
            panic!("{name} limits regressions: {faults:#?}");
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(table, &path).unwrap();
        println!("baseline created at {}", path.display());
    }
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_gmsk_limits_full() {
    let link = gmsk_link(0.5);
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        &GMSK05_GRID,
        GMSK05_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("gmsk-bt05-discriminator", GMSK05_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&link, RATE, op_db, GMSK05_SEED ^ 0xbe5);
    table
        .rows
        .extend(gmsk_burst_axis_rows(op_db, GMSK05_SEED ^ 0xbe5));
    write_or_check_limits(&table, "gmsk_limits.json");
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_msk_limits_full() {
    let link = msk_link();
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        &MSK_GRID,
        MSK_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("msk-discriminator", MSK_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&link, RATE, op_db, MSK_SEED ^ 0xbe5);
    write_or_check_limits(&table, "msk_limits.json");
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_afsk_limits_full() {
    let link = afsk_filterbank_link();
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        &AFSK_GRID,
        AFSK_FB_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("afsk-tone-filterbank", AFSK_FB_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = afsk_axis_rows(&link, op_db, AFSK_FB_SEED ^ 0xbe5);
    write_or_check_limits(&table, "afsk_limits.json");
}

// --- Exploration (never asserted; chooses the sweep grids) ------------------------------------

#[test]
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (link, name) in [
        (gmsk_link(0.3), "gmsk bt=0.3"),
        (gmsk_link(0.5), "gmsk bt=0.5"),
        (msk_link(), "msk"),
        (afsk_filterbank_link(), "afsk filterbank"),
        (afsk_discriminator_link(), "afsk discriminator"),
    ] {
        let grid: Vec<f64> = (3..=15).map(|d| f64::from(d) * 2.0).collect();
        let curve = sweep::sweep_ber(&link, &ChannelSpec::default(), &grid, 0x9999, 100, 200_000);
        println!("--- {name}");
        for p in &curve.points {
            println!(
                "{:>5.1} dB  BER {:.3e}  ({}/{})",
                p.ebn0_db,
                p.rate(),
                p.errors,
                p.trials
            );
        }
    }
}
