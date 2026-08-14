use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};

use super::{
    Measurement,
    framing::{
        self, Acquisition, FRONT_TAPS, NOISE_BW_HZ, PREAMBLE, RATE, SPS, STEADY_BITS, TAIL, UW24,
        cpm_wave, find_uw, framed_symbols, steady_link, steady_soft, uw_levels,
    },
};
use crate::{
    ber::{
        impair::{BurstModel, ChannelSpec},
        sweep::Link,
    },
    cpm::{CpmDemod, CpmParams, KnownSymbols, Mapping, MlseDetector, TIMING_BW_BURST},
    pulse::{self, Norm},
    soft::SoftBit,
};

/// Gaussian pulse spans in symbols (`gaussian_freq`'s total-length convention): BT = 0.5
/// decays within 3 symbols; BT = 0.3's longer tails need 4 — the `pulse::cpm` tests' figures.
#[must_use]
pub fn span(bt: f64) -> usize {
    if bt < 0.4 { 4 } else { 3 }
}

/// GMSK at the given BT: Gaussian partial-response frequency pulse, h = ½ (D-STAR/Bluetooth
/// BR at BT 0.5, GSM's 3GPP TS 45.004 figure at BT 0.3).
#[must_use]
pub fn params(bt: f64) -> CpmParams {
    CpmParams::from_h(
        Mapping::natural(2),
        0.5,
        pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area),
        SPS,
    )
}

/// Discriminator-tier receive filters, chosen by measurement across six candidates each (rect
/// at 0.5/0.8/1/1.2/1.5 symbols, the premod Gaussian at the entry's BT, a BT = 0.5 Gaussian, a
/// 0.55-baud lowpass, and the frequency pulse itself):
///
/// - **BT = 0.5: the frequency pulse (rect ⊗ Gaussian) — the matched filter.** Best measured
///   at every point (1e-2 at 10.8 dB vs the premod Gaussian's 12.3; 0.9 dB ahead of it at
///   1e-3), and a clean tail where the premod shape keeps straggler errors to 20 dB.
/// - **BT = 0.3: a BT = 0.5 Gaussian, deliberately *not* matched.** The matched filter's
///   3-symbol ISI closes the inner eye and the tail goes shallow (1e-3 at ~24.5 dB); plain
///   rect keeps a steep tail (1e-3 at ~20 dB) but the unsmoothed ISI feeds the Gardner
///   detector so badly that acquisition fails outright below ~14 dB. The BT = 0.5 smoothing
///   is the measured compromise. The real fix for BT = 0.3 is the MLSE tier — GSM itself
///   never decoded BT = 0.3 symbol-by-symbol.
#[must_use]
pub fn rx(bt: f64) -> Vec<f32> {
    if bt < 0.4 {
        pulse::gaussian(SPS, 0.5, 3, Norm::Area)
    } else {
        pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area)
    }
}

/// The MLSE tier's receive filter is always the entry's own frequency pulse — the matched
/// filter. That is the whole point of the tier: the discriminator row runs an *unmatched*
/// BT = 0.5 Gaussian at BT = 0.3 purely to keep an eye open for a symbol-by-symbol slicer
/// ([`rx`]), paying noise bandwidth for it, and a detector that decides sequences has no
/// reason to make that trade.
#[must_use]
pub fn mlse_rx(bt: f64) -> Vec<f32> {
    pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area)
}

fn rx_name(bt: f64) -> &'static str {
    if bt < 0.4 {
        "gaussian-BT0.5 rx"
    } else {
        "pulse-matched rx"
    }
}

/// One steady GMSK link at the discriminator + slicer tier.
#[must_use]
pub fn link(bt: f64) -> Link {
    framed_link(bt, Acquisition::DataLike)
}

#[must_use]
pub fn alternating_link(bt: f64) -> Link {
    framed_link(bt, Acquisition::Alternating)
}

fn framed_link(bt: f64, acquisition: Acquisition) -> Link {
    let filler = match acquisition {
        Acquisition::DataLike => "data-like",
        Acquisition::Alternating => "alternating",
    };
    steady_link(
        &format!(
            "gmsk BT={bt} h=0.5 uncoded, CpmMod -> +/-6 kHz front lowpass -> CpmDemod \
             ({}, timing bw 0.015), 48 kHz 4800 baud, {filler} 96+24+24 symbol overhead \
             in Eb, release",
            rx_name(bt)
        ),
        acquisition,
        params(bt),
        rx(bt),
    )
}

#[must_use]
pub fn bt03_link() -> Link {
    link(0.3)
}

#[must_use]
pub fn bt05_link() -> Link {
    link(0.5)
}

/// Best sync position by Euclidean distance in the MLSE tier's *own* soft-bit domain.
///
/// Neither existing search serves this tier. The soft-symbol [`find_uw`] reads the closed eye
/// the trellis exists to open — at BT = 0.3 through the matched filter those symbols carry
/// barely more word-position information than noise. A hard Hamming match over the decided
/// symbols reads the trellis output but throws away its confidence, and that is the mistake
/// [`find_uw`]'s docs already record for the slicer tier: measured here on the committed grid,
/// hard matching mis-anchored whole trials often enough to put a *rise* in the BT = 0.3 curve
/// between 16 and 17 dB (5.8e-4 → 7.8e-4, each mis-anchored trial contributing ~512 errors at
/// once). The detector's per-bit soft output is exactly the confidence the search was missing.
fn find_uw_soft(bits: &[SoftBit], lo: usize, hi: usize, uw: &[u8]) -> Option<usize> {
    let last = hi.min(bits.len().checked_sub(uw.len())?);
    let misfit = |at: usize| -> f32 {
        uw.iter()
            .enumerate()
            .map(|(i, &s)| {
                let want = if s == 1 { 1.0 } else { -1.0 };
                let got = bits[at + i].0;
                (got - want) * (got - want)
            })
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

/// One steady GMSK link at the sequence-detection tier: the discriminator chain up to its soft
/// symbols, then [`MlseDetector`] over the entry's own response instead of the slicer. The
/// detector is built per trial, like every other piece of the receive chain here, so a trial
/// reproduces from its own seed alone.
#[must_use]
pub fn mlse_link(bt: f64) -> Link {
    let p = params(bt);
    let filter = mlse_rx(bt);
    let mod_params = p.clone();
    Link {
        label: format!(
            "gmsk BT={bt} h=0.5 uncoded, MLSE tier: CpmMod -> +/-6 kHz front lowpass -> \
             CpmDemod (pulse-matched rx, timing bw 0.015) -> MlseDetector over the pulse's own \
             symbol-spaced response, 48 kHz 4800 baud, data-like 96+24+24 symbol overhead in \
             Eb, release"
        ),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(move |bits| {
            cpm_wave(
                &mod_params,
                &framed_symbols(Acquisition::DataLike, PREAMBLE, &UW24, bits, TAIL),
            )
        }),
        demodulate: Box::new(move |wave| {
            let soft = steady_soft(&p, &filter, wave);
            let mut detector = MlseDetector::new(&p, &filter);
            let (mut decided, mut bits) = (Vec::new(), Vec::new());
            detector.process(&soft, &mut decided, &mut bits);
            detector.flush(&mut decided, &mut bits);
            let Some(at) = find_uw_soft(&bits, PREAMBLE, PREAMBLE + 48, &UW24) else {
                return Vec::new();
            };
            (0..STEADY_BITS)
                .map(|k| decided.get(at + UW24.len() + k) == Some(&1))
                .collect()
        }),
    }
}

#[must_use]
pub fn bt03_mlse_link() -> Link {
    mlse_link(0.3)
}

#[must_use]
pub fn bt05_mlse_link() -> Link {
    mlse_link(0.5)
}

/// Samples of dead air ahead of the first burst, rounded up to whole frames, so the gate's
/// floor estimate (3840-sample settle window at 10 sps) has settled on the channel's true
/// noise before any burst.
const BURST_LEAD_SAMPLES: usize = 12_000;

/// Frames per burst probe: enough payload to amortise the acquisition frame, short enough
/// that a bisection of seeded probes stays fast.
pub const BURST_FRAMES: usize = 6;

/// One parameterisation of the GMSK burst chain: BT = 0.5 content of 24 sync + payload
/// symbols radiated per frame, the rest dead — an AIS-shaped 256-symbol frame by default.
/// The limits axes vary one field each.
#[derive(Clone, Copy)]
pub struct BurstRecipe {
    pub payload_symbols: usize,
    pub off_symbols: usize,
    pub payload_frames: usize,
    /// `BurstModel` level step applied to alternate bursts; negative attenuates.
    pub level_step_db: f64,
}

impl BurstRecipe {
    #[must_use]
    pub fn reference(payload_frames: usize) -> Self {
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
        self.content() * SPS as usize + params(0.5).freq_pulse().len()
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS as usize)
    }

    fn bits(&self) -> usize {
        self.payload_symbols * self.payload_frames
    }

    /// The full symbol stream: data-like filler everywhere (the exciter keeps shaping through
    /// the dead time; `BurstModel` does the carving), content windows of frames
    /// 1..=payload_frames overwritten with sync + payload. Frame 0's radiated content is the
    /// clock-acquisition preamble.
    fn symbols(&self, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        let mut symbols = framing::data_like_symbols(
            frame * self.payload_frames + self.content(),
            framing::DATA_LIKE_SEED,
        );
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
    #[must_use]
    pub fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS as usize;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS as usize,
            self.level_step_db,
            26.0,
        ))
    }

    #[must_use]
    pub fn link(&self, label: &str) -> Link {
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
                wave.extend(cpm_wave(&params(0.5), &recipe.symbols(bits)));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let p = params(0.5);
        let front = design_lowpass(FRONT_TAPS, NOISE_BW_HZ / RATE);
        let mut filter = Decimator::new(&front, 1);
        let mut demod = CpmDemod::new(&p, &rx(0.5), TIMING_BW_BURST);
        let mut filtered = Vec::new();
        filter.process(wave, &mut filtered);
        let mut soft = Vec::new();
        demod.process(&filtered, &mut soft);
        let levels = uw_levels(&p, &UW24);

        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut hook = KnownSymbols::new(&p, (4 * frame) as u32);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for k in 0..self.payload_frames {
            let expect = lead + frame * (k + 1);
            let (lo, hi) = if k == 0 {
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
            for i in 0..self.payload_symbols {
                hook.tick();
                let bit = at
                    .and_then(|at| soft.get(at + UW24.len() + i))
                    .is_some_and(|&s| p.mapping().slice(hook.correct(s)) == 1);
                bits.push(bit);
            }
        }
        bits
    }
}

/// Sweep grids covering each chain's waterfall through BER 1e-4, set from the ignored
/// `probe_grids` exploration and pinned by the committed curves. BT = 0.3's grid sits an
/// octave higher and runs shallower than BT = 0.5's — the discriminator tier's ISI penalty at
/// that BT, see [`rx`].
///
/// BT = 0.3 starts at its shoulder rather than below it, the same rule [`BT05_MLSE_GRID`]
/// records: measured at 12–13 dB this chain is past acquisition threshold and the points stop
/// ordering (5.96e-2 at 12 dB against 6.98e-2 at 13, far outside the budget's counting noise),
/// which is a statement about whether the receiver locks, not about the entry's BER. The
/// alternating-framed generation put its disorder at 14→15 instead, inside the waterfall
/// proper — that curve's non-monotonicity is the framing's, and this one's absence of it is
/// what the rename bought.
pub const BT03_GRID: &[f64] = &[
    14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0,
];
pub const BT05_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0];

/// The MLSE tier's grids. Both sit lower than their discriminator counterparts — that gap *is*
/// the tier's gain — and BT = 0.3's runs two points past its 1e-4 crossing because the entry's
/// tail below that is shallow: partial response leaves a population of low-distance trellis
/// error events, and the committed curve has to show where it flattens rather than stop at the
/// last steep point (`probe_mlse_error_positions`).
pub const BT03_MLSE_GRID: &[f64] = &[
    8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];
/// BT = 0.5's grid starts at its shoulder rather than below it: measured at 6–8 dB the chain is
/// past acquisition threshold and the points stop ordering (1.4e-1 at 8 dB against 1.1e-1 at
/// 7), which is a statement about whether the receiver locks, not about the entry's BER.
pub const BT05_MLSE_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];

pub const BT03_SEED: u64 = 0x63a3;
pub const BT05_SEED: u64 = 0x63a5;
pub const BT03_MLSE_SEED: u64 = 0x63a3_11e5;
pub const BT05_MLSE_SEED: u64 = 0x63a5_11e5;

/// The committed artifacts of the entry's four measured curves.
pub const BT03_AWGN: &str = "cpm/gmsk_bt03_datalike_awgn";
pub const BT05_AWGN: &str = "cpm/gmsk_bt05_datalike_awgn";
pub const BT03_MLSE_AWGN: &str = "cpm/gmsk_bt03_mlse_awgn";
pub const BT05_MLSE_AWGN: &str = "cpm/gmsk_bt05_mlse_awgn";

pub const BT03_AWGN_ALTERNATING: &str = "cpm/gmsk_bt03_awgn";
pub const BT05_AWGN_ALTERNATING: &str = "cpm/gmsk_bt05_awgn";

pub const LIMITS: &str = "cpm/gmsk_datalike_limits";
pub const PERF: &str = "cpm/gmsk_perf";
pub const MLSE_PERF: &str = "cpm/gmsk_mlse_perf";
pub const MLSE_LIMITS: &str = "cpm/gmsk_mlse_limits";
pub const LIMITS_ALTERNATING: &str = "cpm/gmsk_limits";

/// The entry's measurements, in the order `cargo xtask ber gmsk` runs them. The historical
/// alternating-framed curves are deliberately absent: they are never re-measured as an
/// artifact, only compared against (see [`alternating_link`]).
pub const MEASUREMENTS: &[Measurement] = &[
    Measurement::committed(
        BT03_AWGN,
        bt03_link,
        BT03_GRID,
        BT03_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT05_AWGN,
        bt05_link,
        BT05_GRID,
        BT05_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT03_MLSE_AWGN,
        bt03_mlse_link,
        BT03_MLSE_GRID,
        BT03_MLSE_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT05_MLSE_AWGN,
        bt05_mlse_link,
        BT05_MLSE_GRID,
        BT05_MLSE_SEED,
        framing::FULL_CAP,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The two tiers must differ by their detector alone: same pulse, same framing, same
    /// lengths. A tier gain measured across a framing change would not be a tier gain.
    #[test]
    fn both_tiers_transmit_the_same_waveform() {
        let bits: Vec<bool> = (0..STEADY_BITS).map(|i| i % 3 == 0).collect();
        for bt in [0.3, 0.5] {
            assert_eq!(
                (link(bt).modulate)(&bits),
                (mlse_link(bt).modulate)(&bits),
                "BT={bt}: the tiers no longer share a transmitter"
            );
        }
    }
}
