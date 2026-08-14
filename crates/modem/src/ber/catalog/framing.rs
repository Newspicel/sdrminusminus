//! The steady-frame substrate the GMSK, MSK and AFSK entries share: one geometry, one
//! alignment rule, one receive front end, so a difference between two of those curves reads
//! the modulation and nothing else.
use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};

use crate::{
    ber::{rng::Rng, sweep::Link},
    cpm::{CpmDemod, CpmMod, CpmParams, TIMING_BW_BURST},
};

/// GMSK/MSK reference rate: 48 kHz / 4800 baud, 10 samples per symbol — the D-STAR figures,
/// and the rate the perf baselines' real-time factors divide by. The engine itself is
/// rate-free (everything is sps); the Hz numbers exist so the limits axes read in physical
/// units.
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;
pub const RATE: f64 = BAUD * SPS;

/// One-sided channel-selection cutoff shared by the GMSK and MSK links. Carson-rule sizing
/// for the wider of the two (MSK/1REC): outer deviation h·baud/2 = 1200 Hz plus one baud of
/// modulation bandwidth. Identical for both entries so the committed BT comparison isolates
/// the frequency pulse, not the noise bandwidth.
pub const NOISE_BW_HZ: f64 = 6_000.0;
pub const FRONT_TAPS: usize = 127;

/// Steady-frame geometry: a clock-acquisition preamble, a unique word the receiver aligns on
/// (never assumed — searched), the payload, and enough trailing filler that the front end's
/// group delay does not swallow the last payload symbols.
pub const PREAMBLE: usize = 96;
pub const TAIL: usize = 24;
pub const STEADY_BITS: usize = 1024;

/// 24-symbol unique word (0x4F9968 MSB-first), chosen by search for aperiodic
/// autocorrelation: worst shifted-overlap sidelobe 3 of 24, counting an alternating preamble
/// as left context. The property is load-bearing — a first draft used 0xB62B62, whose halves
/// repeat, and one payload in 4096 continued the pattern into a *perfect* 12-shifted anchor
/// (measured: a whole mis-sliced trial at 20 dB Eb/N0).
pub const UW24: [u8; 24] = [
    0, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0,
];

/// Trial-bit cap per committed point on this substrate: it bounds the steep high-SNR points,
/// where the error budget alone would run for hours. At [`STEADY_BITS`] per trial this is
/// ~3900 trials, and the 1e-4 points still collect a few hundred errors.
pub const FULL_CAP: u64 = 4_000_000;

/// Receiver noise 40 dB below a unit carrier — what the demodulator hears before a
/// transmission, the `fsk4` tests' `listening` convention. The fixed seed is part of the
/// chain definition: every trial's demodulator meets the channel having heard the same quiet.
pub const WARMUP_SYMBOLS: usize = 500;

/// Alternating clock-acquisition filler: the two levels in turn.
///
/// Correct only where the entry's symbol response is one tap. A *partial-response* pulse puts
/// the alternating pattern in its own spectral null — at GMSK BT = 0.3 the response
/// [0.014, 0.220, 0.532, 0.220, 0.014] sums to 0.119 against an alternating stream, so
/// acquisition arrives 18 dB below the payload and every loop that must converge before the
/// payload converges on the wrong scale. Full-response entries (MSK's rect ⊗ rect triangle
/// has nulls at ±T; AFSK's rect likewise) pay nothing for it. Partial-response entries use
/// [`data_like_symbols`] instead.
#[must_use]
pub fn alternating_symbols(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 2) as u8).collect()
}

/// The acquisition seed every data-like steady chain starts from (`0x9e37_79b9`, the golden-
/// ratio constant): one value, so two chains framed this way differ by their modulation only.
pub const DATA_LIKE_SEED: u32 = 0x9e37_79b9;

/// `len` data-like binary symbols, advancing `state` — one xorshift stream per trial, so a
/// frame's leading and trailing filler are different symbols rather than the same block twice
/// and a trial still reproduces from its own seed alone.
fn data_like_from(state: &mut u32, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            (*state & 1) as u8
        })
        .collect()
}

/// Data-like acquisition filler from a stated seed.
///
/// The rule behind it — an acquisition sequence must look like the data the loops will have to
/// hold — is the same one `catalog::mfsk`'s `preamble` records for the M-ary steady chains,
/// arrived at independently there.
#[must_use]
pub fn data_like_symbols(len: usize, seed: u32) -> Vec<u8> {
    data_like_from(&mut { seed }, len)
}

/// How an entry fills the symbols either side of its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acquisition {
    /// [`alternating_symbols`] — sound only for a one-tap symbol response.
    Alternating,
    /// [`data_like_symbols`] — required of any partial-response entry.
    DataLike,
}

/// Symbol stream of one steady trial: acquisition filler, unique word, payload bits as symbol
/// indices (index = bit for every 2-level mapping here — the mapping table maps index to
/// level), trailing filler.
#[must_use]
pub fn framed_symbols(
    acquisition: Acquisition,
    preamble: usize,
    uw: &[u8],
    bits: &[bool],
    tail: usize,
) -> Vec<u8> {
    let mut state = DATA_LIKE_SEED;
    let mut fill = |len: usize| match acquisition {
        Acquisition::Alternating => alternating_symbols(len),
        Acquisition::DataLike => data_like_from(&mut state, len),
    };
    let mut s = fill(preamble);
    s.extend_from_slice(uw);
    s.extend(bits.iter().map(|&b| u8::from(b)));
    s.extend(fill(tail));
    s
}

#[must_use]
pub fn cpm_wave(params: &CpmParams, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut m = CpmMod::new(params.clone());
    let mut out = Vec::new();
    m.modulate(symbols, &mut out);
    m.flush(&mut out);
    out
}

#[must_use]
pub fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

#[must_use]
pub fn real_quiet(seed: u64, len: usize) -> Vec<f32> {
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
#[must_use]
pub fn steady_soft(params: &CpmParams, rx: &[f32], wave: &[Complex<f32>]) -> Vec<f32> {
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

/// The unique word as transmitted levels — what the soft symbols are correlated against.
#[must_use]
pub fn uw_levels(params: &CpmParams, uw: &[u8]) -> Vec<f32> {
    uw.iter().map(|&s| params.mapping().level(s)).collect()
}

/// Best sync position in `lo..=hi` by Euclidean distance of the *soft* symbols to the word's
/// transmitted levels — the searched-alignment idiom, taken soft because a hard-sliced
/// Hamming match throws away exactly the confidence that separates the true position from an
/// ISI-corrupted neighbour, and as a distance rather than a bare correlation because a dot
/// product rewards overshooting symbols and was measured mis-anchoring whole trials. No
/// threshold: a chain too degraded to place its sync scores its garbage as bit errors.
#[must_use]
pub fn find_uw(soft: &[f32], lo: usize, hi: usize, levels: &[f32]) -> Option<usize> {
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
#[must_use]
pub fn payload_bits(
    params: &CpmParams,
    soft: &[f32],
    at: usize,
    uw_len: usize,
    n: usize,
) -> Vec<bool> {
    (0..n)
        .map(|k| {
            soft.get(at + uw_len + k)
                .is_some_and(|&s| params.mapping().slice(s) == 1)
        })
        .collect()
}

/// One steady complex-entry link: bits → framed symbols → `CpmMod` → (channel) → lowpass →
/// `CpmDemod` → slice → align on the unique word → payload bits.
#[must_use]
pub fn steady_link(label: &str, acquisition: Acquisition, params: CpmParams, rx: Vec<f32>) -> Link {
    let mod_params = params.clone();
    Link {
        label: label.to_string(),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(move |bits| {
            cpm_wave(
                &mod_params,
                &framed_symbols(acquisition, PREAMBLE, &UW24, bits, TAIL),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The framing's lengths are the Eb accounting: a tier or acquisition change that quietly
    /// moved one would shift every curve on this substrate without any gate noticing.
    #[test]
    fn every_acquisition_frames_the_same_lengths() {
        let bits = [true, false, true, true];
        for acquisition in [Acquisition::Alternating, Acquisition::DataLike] {
            let s = framed_symbols(acquisition, PREAMBLE, &UW24, &bits, TAIL);
            assert_eq!(s.len(), PREAMBLE + UW24.len() + bits.len() + TAIL);
            assert_eq!(&s[PREAMBLE..PREAMBLE + UW24.len()], &UW24);
        }
    }

    /// The property the whole [`Acquisition::DataLike`] choice rests on: through a
    /// partial-response symbol response, alternating filler collapses towards the response's
    /// DC null while data-like filler arrives at payload scale. Measured on the GMSK BT = 0.3
    /// response, which is why that entry cannot be framed with the alternating pattern.
    #[test]
    fn alternating_filler_collapses_through_a_partial_response() {
        let response = [0.014f32, 0.220, 0.532, 0.220, 0.014];
        let level = |s: u8| if s == 1 { 1.0f32 } else { -1.0 };
        let convolve = |symbols: &[u8]| -> f32 {
            symbols
                .windows(response.len())
                .map(|w| {
                    w.iter()
                        .zip(&response)
                        .map(|(&s, &h)| level(s) * h)
                        .sum::<f32>()
                        .abs()
                })
                .fold(0.0, f32::max)
        };
        let alternating = convolve(&alternating_symbols(64));
        let data_like = convolve(&data_like_symbols(64, DATA_LIKE_SEED));
        assert!(
            alternating < 0.2 && data_like > 0.9,
            "alternating {alternating}, data-like {data_like}"
        );
    }
}
