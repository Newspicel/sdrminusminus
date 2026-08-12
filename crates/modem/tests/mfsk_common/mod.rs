//! The M-ary CPFSK catalog entry's measured chains (MODEM-PLAN §5, §7 phase 3) — shared
//! between the curve/limits/E2E tests (`mfsk_cpfsk.rs`) and the perf baseline
//! (`mfsk_perf.rs`) so every committed artifact of the entry is taken on the *same* chain.
//!
//! One reference geometry for all three alphabets: 48 kHz, 4800 baud, 10 samples/symbol —
//! the DMR-shaped numbers, so the M = 4 configuration doubles as the migration lane's
//! engine-side reference. Everything that differs between M ∈ {2, 4, 8} is `CpmParams` data
//! plus two receiver choices stated per entry (the §4.1 "metric definitions are explicit"
//! rule):
//!
//! - **A pre-detector channel-selection lowpass.** A discriminator eats its whole input
//!   bandwidth as noise; without selection the curve measures the missing filter, not the
//!   entry (the phase-0 DMR chain's docs put the unfiltered shift near 6 dB). The one-sided
//!   cutoff is set per alphabet at outer deviation + (1+α)·baud/2, rounded up: ±4.8 kHz
//!   (M=2), ±6 kHz (M=4, the 12.5 kHz-channel figure), ±9.6 kHz (M=8).
//! - **The timing bandwidth**, per-entry data with two measured operating points
//!   (`cpm` module docs): the steady chains run `TIMING_BW_CONTINUOUS` — holding the
//!   continuous 2k+-symbol trials the old `Fsk4Demod` floored on is the entry's headline —
//!   and the TDMA chain behind the §4.3 burst rows runs `TIMING_BW_BURST`.
//!
//! Eb accounting: per information bit (payload bits only), with the preamble/sync overhead
//! *energy* charged to Eb exactly as the phase-0 DMR baselines charge theirs — the labels say
//! so. Alignment is searched, never assumed (the `find_pattern` idiom from the DMR baseline),
//! and the M = 8 chain carries its level scale on the §3.4 known-symbol hook, the measured
//! boundary of blind normalisation at 8 levels (see `cpm::demod`'s `PEAK_SYMBOLS` docs).

// Each integration-test binary compiles its own copy and uses a subset.
#![allow(dead_code)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_modem::{
    ber::{
        impair::{BurstModel, ChannelSpec},
        rng::Rng,
        sweep::Link,
    },
    cpm::{
        CpmDemod, CpmMod, CpmParams, KnownSymbols, Mapping, TIMING_BW_BURST, TIMING_BW_CONTINUOUS,
    },
    pulse::{self, Norm},
};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;
const SPAN: usize = 8;
const CHANNEL_TAPS: usize = 127;

/// Acquisition takes the timing loop several time constants at the continuous bandwidth
/// (1/0.003 ≈ 333 symbols), and the tail of the transient bites long after lock looks
/// achieved — measured on the clean M = 4 chain: a 480-symbol preamble (1.4τ) leaves the
/// loop's decaying phase/rate overshoot clustering errors hundreds of symbols *into the
/// payload* (up to ~2e-3 in the worst trials), while 1500 symbols (~4.5τ) leaves a scattered
/// ~1e-5 residual — the engine's own measured continuous floor. The overhead is amortised by
/// the long payload and charged to Eb per the labels (~1 dB).
pub const STEADY_PREAMBLE: usize = 1_500;
/// Trailing filler past the payload: the front end is a channel filter plus a matched filter
/// late (~11 symbols), so the transmitter keeps shaping that long or the tail symbols are
/// never emitted.
pub const STEADY_TAIL: usize = 40;

/// Quiet-listening warm-up ahead of the steady chains, in samples: the gate's floor-settle
/// window (4·96 symbol periods) plus margin, at receiver noise 40 dB down — how a receiver
/// meets a transmission it was tuned to before key-up (the `fsk4`/DMR-baseline convention).
pub const WARMUP_SAMPLES: usize = (4.0 * 96.0 * SPS) as usize + 300 * SPS as usize;

/// One alphabet's measured configuration: the `CpmParams` data plus the two receiver choices
/// documented at module level. Everything the committed artifacts were taken with lives here.
pub struct Entry {
    pub params: CpmParams,
    pub receive_filter: Vec<f32>,
    pub channel_taps: Vec<f32>,
    pub timing_bw: f64,
}

/// M = 2 CPFSK at h = ½ (MSK-index 2FSK): rect pulse, integrate-and-dump receive filter —
/// the POCSAG/RTTY base shape, referenced against noncoherent orthogonal 2-FSK theory plus
/// the documented discriminator offset.
pub fn mfsk2() -> Entry {
    Entry {
        params: CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS),
        receive_filter: pulse::rect(SPS, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 4_800.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

/// The ETSI dibit table (TS 102 361-1 §4.2.2): 00 → +1, 01 → +3, 10 → −1, 11 → −3 — supplied
/// as caller data, the axis the DMR probe exists to stress.
pub fn dibit_mapping() -> Mapping {
    Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
}

/// M = 4, DMR-like: ETSI dibit table, ±1944 Hz outer deviation (h = 0.27), RRC α = 0.2 —
/// the reference configuration behind the limits table and the perf baseline.
pub fn mfsk4() -> Entry {
    Entry {
        params: CpmParams::from_deviation(
            dibit_mapping(),
            1_944.0,
            BAUD,
            pulse::root_raised_cosine(SPS, 0.2, SPAN, Norm::Area),
            SPS,
        ),
        receive_filter: pulse::root_raised_cosine(SPS, 0.2, SPAN, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 6_000.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

/// The same M = 4 configuration at the burst timing bandwidth — the chain behind the §4.3
/// burst-survival rows, where TDMA gating wants the wide loop (`TIMING_BW_BURST` docs).
pub fn mfsk4_burst() -> Entry {
    Entry {
        timing_bw: TIMING_BW_BURST,
        ..mfsk4()
    }
}

/// M = 8 CPFSK at h = 0.3, natural odd-integer levels ±1..±7, rect pulse. No protocol behind
/// it — 8-ary gates the engine's generality (§7 phase 3) — and its level scale rides the
/// known-symbol hook, as §3.4 prescribes past blind normalisation's measured M ≤ 4 boundary.
pub fn mfsk8() -> Entry {
    Entry {
        params: CpmParams::from_h(Mapping::natural(8), 0.3, pulse::rect(SPS, Norm::Area), SPS),
        receive_filter: pulse::rect(SPS, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 9_600.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

// --- Sync patterns ---------------------------------------------------------------------------

/// The DMR BS voice sync's 24 dibits (ETSI TS 102 361-1 §9.1.1), oldest first — the M = 4
/// chains align and anchor on the same word the protocol will.
pub fn sync4() -> Vec<u8> {
    let bits: u64 = 0x755F_D7DF_75F7;
    (0..24)
        .rev()
        .map(|i| (bits >> (2 * i)) as u8 & 0b11)
        .collect()
}

/// 24-symbol binary sync for the M = 2 chain: balanced, aperiodic, no run past four.
pub const SYNC2: [u8; 24] = [
    1, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 1,
];

/// 16-symbol known pattern for the M = 8 chain — every level visited twice, so the hook's
/// least-squares fit is conditioned on the whole alphabet.
pub const SYNC8: [u8; 16] = [7, 0, 5, 2, 6, 1, 4, 3, 0, 7, 3, 4, 1, 6, 2, 5];

/// M = 8 framing: a known pattern every 128 symbols, hook-corrected payload between — the
/// shape every burst standard embeds, here as pure entry data.
pub const M8_FRAME: usize = 128;
pub const M8_PAYLOAD: usize = M8_FRAME - SYNC8.len();
pub const M8_FRAMES: usize = 48;

// --- Shared helpers --------------------------------------------------------------------------

/// Steady-chain acquisition preamble: *data-like* fixed pseudo-random symbols, not the
/// classic alternating outer levels. Measured on the M = 4 chain: an alternating preamble
/// parks the Gardner loop at that pattern's own ISI equilibrium, and at the continuous
/// bandwidth (0.003 cy/sym) the re-convergence onto random data takes ~300 symbols — errors
/// recur at fixed payload positions ~350–720 and floor the clean curve at ~1.3e-4. Random
/// symbols acquire from cold just as the engine's 20k-symbol test proves, and park the loop
/// where the payload keeps it: measured at the 480-symbol length, going data-like halved the
/// floor on its own; [`STEADY_PREAMBLE`]'s length removes the rest.
pub fn preamble(entry: &Entry, len: usize) -> Vec<u8> {
    let m = entry.params.mapping().m() as u32;
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % m) as u8
        })
        .collect()
}

/// Maximally-transitioning filler: the outermost positive and negative levels alternated,
/// whatever indices the mapping table gave them — the tail shaping and the TDMA dead-slot
/// filler (where the burst chain's per-burst sync re-anchoring makes the loop equilibrium
/// argument above moot).
pub fn alternating(entry: &Entry, len: usize) -> Vec<u8> {
    let levels = entry.params.mapping().levels();
    let hi = (0..levels.len())
        .max_by(|&a, &b| levels[a].total_cmp(&levels[b]))
        .unwrap_or(0);
    let lo = (0..levels.len())
        .min_by(|&a, &b| levels[a].total_cmp(&levels[b]))
        .unwrap_or(0);
    (0..len)
        .map(|i| if i % 2 == 0 { hi as u8 } else { lo as u8 })
        .collect()
}

/// Payload bits to symbol indices, MSB first per symbol — the bit order `Mapping::soft_bits`
/// emits and the DMR dibit convention reads.
pub fn bits_to_symbols(bits: &[bool], bits_per_symbol: usize) -> Vec<u8> {
    bits.chunks(bits_per_symbol)
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
        .collect()
}

pub fn push_symbol_bits(symbol: u8, bits_per_symbol: usize, out: &mut Vec<bool>) {
    for k in (0..bits_per_symbol).rev() {
        out.push(symbol >> k & 1 == 1);
    }
}

/// Receiver noise at 40 dB below a unit carrier — what the demodulator hears before the
/// transmission, exactly the `fsk4`/DMR-baseline `listening` convention.
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

/// The receive front end under measurement: channel-selection lowpass into `CpmDemod`, fresh
/// per trial so every trial reproduces from its own seed alone. `warm_up` is the steady
/// chains' quiet listening; the burst chain instead carries dead air inside the waveform so
/// the AWGN axis covers it and the gate measures the floor it will actually gate on.
pub fn recovered_soft(entry: &Entry, wave: &[Complex<f32>], warm_up: bool) -> Vec<f32> {
    let mut filter = Decimator::new(&entry.channel_taps, 1);
    let mut demod = CpmDemod::new(&entry.params, &entry.receive_filter, entry.timing_bw);
    let mut filtered = Vec::new();
    if warm_up {
        let mut discard = Vec::new();
        filter.process(&quiet(0x1157, WARMUP_SAMPLES), &mut filtered);
        demod.process(&filtered, &mut discard);
    }
    let mut soft = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut soft);
    soft
}

fn pattern_distance(sliced: &[u8], at: usize, pattern: &[u8]) -> usize {
    pattern
        .iter()
        .enumerate()
        .filter(|&(i, &s)| sliced[at + i] != s)
        .count()
}

/// Best sync position in `lo..=hi` by symbol Hamming distance — the searched-alignment idiom.
/// No threshold: a chain too degraded to place its sync scores its garbage as bit errors.
pub fn find_pattern(sliced: &[u8], lo: usize, hi: usize, pattern: &[u8]) -> Option<usize> {
    let last = hi.min(sliced.len().checked_sub(pattern.len())?);
    (lo..=last).min_by_key(|&at| pattern_distance(sliced, at, pattern))
}

pub fn modulate(entry: &Entry, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = CpmMod::new(entry.params.clone());
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    modulator.flush(&mut out);
    out
}

pub fn baseline_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/cpm/{name}"))
}

// --- Steady (continuous) links ---------------------------------------------------------------

/// Payload symbols per steady trial: long enough that the continuous-mode claim is exercised
/// (well past the ~2000 symbols where the phase-0 chain's wander floor set in), short enough
/// that one trial stays a breath.
pub const STEADY_PAYLOAD: usize = 6_144;

/// The continuous chain for M = 2 or M = 4 as one payload-to-payload [`Link`]:
/// preamble + sync + payload + tail through [`CpmMod`], searched sync alignment, payload
/// sliced straight off the mapping table. `payload_symbols` is [`STEADY_PAYLOAD`] for the
/// committed curves; the level-1 E2E runs the same chain with short payloads, because its
/// property is perfection and the entry's honest continuous residual (~1e-5, the engine's
/// own measured floor) bounds how many bits perfection can fairly be demanded over.
pub fn steady_link(
    make_entry: fn() -> Entry,
    sync: Vec<u8>,
    label: &str,
    payload_symbols: usize,
) -> Link {
    let entry = make_entry();
    let bits_per_symbol = entry.params.mapping().bits_per_symbol() as usize;
    let tx_sync = sync.clone();
    let demod_entry = make_entry();
    Link {
        label: label.to_string(),
        bits_per_trial: payload_symbols * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut symbols = preamble(&entry, STEADY_PREAMBLE);
            symbols.extend_from_slice(&tx_sync);
            symbols.extend(bits_to_symbols(bits, bits_per_symbol));
            symbols.extend(alternating(&entry, STEADY_TAIL));
            modulate(&entry, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            let soft = recovered_soft(&demod_entry, wave, true);
            let mapping = demod_entry.params.mapping();
            let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
            let Some(at) = find_pattern(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 72, &sync)
            else {
                return Vec::new();
            };
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = sliced.get(at + sync.len() + k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

pub fn mfsk2_link_sized(payload_symbols: usize) -> Link {
    steady_link(
        mfsk2,
        SYNC2.to_vec(),
        "mfsk2 CPFSK h=0.5 rect, +-4.8 kHz select -> CpmDemod bw 0.003, \
         1500-sym preamble + 24-sym sync overhead in Eb, release",
        payload_symbols,
    )
}

pub fn mfsk2_link() -> Link {
    mfsk2_link_sized(STEADY_PAYLOAD)
}

pub fn mfsk4_link_sized(payload_symbols: usize) -> Link {
    steady_link(
        mfsk4,
        sync4(),
        "mfsk4 CPFSK ETSI dibits h=0.27 RRC a=0.2, +-6 kHz select -> CpmDemod bw 0.003, \
         1500-sym preamble + 24-sym sync overhead in Eb, release",
        payload_symbols,
    )
}

pub fn mfsk4_link() -> Link {
    mfsk4_link_sized(STEADY_PAYLOAD)
}

/// The M = 8 chain: framed known patterns, payload sliced through the §3.4 hook's per-frame
/// least-squares correction — the designed level reference at 8 levels. The first frame's
/// sync is searched wide; later frames only locally, tracking residual slip.
pub fn mfsk8_link() -> Link {
    mfsk8_link_sized(M8_FRAMES)
}

pub fn mfsk8_link_sized(frames: usize) -> Link {
    let entry = mfsk8();
    let demod_entry = mfsk8();
    Link {
        label: "mfsk8 CPFSK h=0.3 rect natural map, +-9.6 kHz select -> CpmDemod bw 0.003 \
                + KnownSymbols per 128-sym frame, preamble + 16/128 sync overhead in Eb, \
                release"
            .to_string(),
        bits_per_trial: frames * M8_PAYLOAD * 3,
        modulate: Box::new(move |bits| {
            let mut symbols = preamble(&entry, STEADY_PREAMBLE);
            let payload = bits_to_symbols(bits, 3);
            for frame in payload.chunks(M8_PAYLOAD) {
                symbols.extend_from_slice(&SYNC8);
                symbols.extend_from_slice(frame);
            }
            symbols.extend(alternating(&entry, STEADY_TAIL));
            modulate(&entry, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            let soft = recovered_soft(&demod_entry, wave, true);
            let mapping = demod_entry.params.mapping();
            let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
            let mut bits = Vec::with_capacity(frames * M8_PAYLOAD * 3);
            let Some(at0) = find_pattern(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 72, &SYNC8)
            else {
                return Vec::new();
            };
            let mut hook = KnownSymbols::new(&demod_entry.params, (4 * M8_FRAME) as u32);
            for frame in 0..frames {
                let expect = at0 + frame * M8_FRAME;
                let at = find_pattern(&sliced, expect.saturating_sub(3), expect + 3, &SYNC8)
                    .unwrap_or(expect);
                if let Some(window) = soft.get(at..at + SYNC8.len()) {
                    hook.anchor(&SYNC8, window);
                }
                for k in SYNC8.len()..M8_FRAME {
                    hook.tick();
                    let symbol = soft
                        .get(at + k)
                        .map_or(0, |&s| mapping.slice(hook.correct(s)));
                    push_symbol_bits(symbol, 3, &mut bits);
                }
            }
            bits
        }),
    }
}

// --- Burst (TDMA) chain for the M = 4 limits rows --------------------------------------------

/// Samples of dead air ahead of the first burst, so the gate's floor estimate (settle window
/// 3840 samples at 10 sps) has measured the channel's true noise before any burst.
pub const BURST_LEAD_SAMPLES: usize = 12_000;

/// Frames per burst trial in the limits probes: enough payload to amortise the acquisition
/// frame, cheap enough that a bisection stays fast.
pub const BURST_FRAMES: usize = 6;

/// One parameterisation of the DMR-shaped TDMA chain (24-sym sync + 108 payload symbols of
/// every 288), carved by the calibrated [`BurstModel`]; the §4.3 burst axes vary one field
/// each, reshaping transmitter and accounting together.
#[derive(Clone, Copy)]
pub struct BurstRecipe {
    pub payload_symbols: usize,
    pub off_symbols: usize,
    pub payload_frames: usize,
    /// `BurstModel` level step applied to alternate bursts; negative attenuates.
    pub level_step_db: f64,
}

impl BurstRecipe {
    pub fn reference(payload_frames: usize) -> Self {
        Self {
            payload_symbols: 108,
            off_symbols: 156,
            payload_frames,
            level_step_db: 0.0,
        }
    }

    pub fn content(&self) -> usize {
        sync4().len() + self.payload_symbols
    }

    pub fn frame_symbols(&self) -> usize {
        self.content() + self.off_symbols
    }

    /// The radiated window per frame, in samples: the content symbols plus the pulse tails
    /// either side, so keying never robs the matched filter of the tails it is built around
    /// (the phase-0 recipe's own figure). The one-symbol ramps live inside it.
    fn on_samples(&self) -> usize {
        self.content() * SPS as usize + 150
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS as usize)
    }

    pub fn bits(&self) -> usize {
        2 * self.payload_symbols * self.payload_frames
    }

    /// Frame 0 is the acquisition preamble; frames 1..=payload_frames carry sync + payload.
    /// Dead slots hold filler the `BurstModel` carves away, so the exciter's phase runs
    /// continuously — the phase-0 recipe's shape, with one measured change: the acquisition
    /// frame and filler are *data-like* ([`preamble`]) rather than alternating. Alternating
    /// content parks the timing loop at its own ISI equilibrium here exactly as on the
    /// steady chain — measured at 30 dB: 6.2e-3 BER concentrated in the first payload
    /// bursts, an order down once the filler is data-like.
    fn symbols(&self, entry: &Entry, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        // One trailing dead slot past the last burst: the front end is a channel filter
        // plus a matched filter late (~11 symbols), and a waveform ending at the last
        // content symbol never emits it — measured at 30 dB as 4–8 errors per trial, all in
        // the final burst's last bits (the phase-0 recipe shares the truncation; its looser
        // 2e-2 floor bound masked it).
        let mut symbols = preamble(entry, frame * (self.payload_frames + 1));
        let sync = sync4();
        let dibits = bits_to_symbols(payload, 2);
        for p in 0..self.payload_frames {
            let base = frame * (p + 1);
            symbols[base..base + sync.len()].copy_from_slice(&sync);
            let src = &dibits[p * self.payload_symbols..(p + 1) * self.payload_symbols];
            symbols[base + sync.len()..base + self.content()].copy_from_slice(src);
        }
        symbols
    }

    /// The impairment template carrying this recipe's TDMA carving; the sweep owns AWGN.
    pub fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS as usize;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS as usize,
            self.level_step_db,
            40.0,
        ))
    }

    pub fn link(&self, label: &str) -> Link {
        let recipe = *self;
        let demod_recipe = *self;
        let entry = mfsk4_burst();
        Link {
            label: label.to_string(),
            bits_per_trial: self.bits(),
            modulate: Box::new(move |bits| {
                let mut wave = vec![
                    Complex::default();
                    recipe.lead_frames() * recipe.frame_symbols() * SPS as usize
                ];
                wave.extend(modulate(&entry, &recipe.symbols(&entry, bits)));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    /// Per-burst re-anchoring, as the decoders themselves run: position against slip via a
    /// local sync search, levels via the known-symbol hook — a burst's own sync knows the
    /// levels better than any loop can.
    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let entry = mfsk4_burst();
        let soft = recovered_soft(&entry, wave, false);
        let mapping = entry.params.mapping();
        let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
        let sync = sync4();
        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut hook = KnownSymbols::new(&entry.params, 4_800);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for p in 0..self.payload_frames {
            let expect = lead + frame * (p + 1);
            // The first burst's sync is searched wide (front-end delay unknown); later bursts
            // only locally, tracking whatever slip the dead time cost the clock.
            let (lo, hi) = if p == 0 {
                (expect, expect + 72)
            } else {
                ((expect + delay).saturating_sub(4), expect + delay + 4)
            };
            let at = find_pattern(&sliced, lo, hi, &sync);
            if let Some(at) = at {
                delay = at.saturating_sub(expect);
                if let Some(window) = soft.get(at..at + sync.len()) {
                    hook.anchor(&sync, window);
                }
            }
            for k in 0..self.payload_symbols {
                hook.tick();
                let symbol = at
                    .and_then(|at| soft.get(at + sync.len() + k))
                    .map_or(0, |&s| mapping.slice(hook.correct(s)));
                push_symbol_bits(symbol, 2, &mut bits);
            }
        }
        bits
    }
}
