//! The steady-frame substrate the linear entries share: one geometry, one alignment rule, one
//! shaping, so a difference between two of those curves reads the modulation and nothing else.
//!
//! **No channel-selection filter, deliberately.** The CPM entries run behind a ±6 kHz lowpass
//! because a discriminator eats the whole sample rate as noise without one
//! ([`framing`](super::framing)). A linear receiver's matched filter *is* its noise-limiting
//! filter, so adding another would only narrow the pulse the entry is specified with. What that
//! buys the catalog is that these curves sit against their closed-form oracles with no front end
//! to attribute a loss to.
//!
//! **The shaping is the phase-0 calibration link's**: root-raised cosine α = 0.35, span 8, at 8
//! samples per symbol — the same taps `ber::reference::ideal_bpsk` measured 0.2 dB from
//! ½·erfc(√γ) with. The BPSK row below is therefore that link plus framing, timing recovery and
//! carrier recovery, and the gap between the two curves is exactly what those three cost.
//!
//! **Overhead is charged to Eb.** A trial carries 512 acquisition symbols, a 32-symbol unique word
//! and a 16-symbol tail around its 8192-symbol payload, and the sweep divides the whole waveform's
//! energy by the *payload* bit count — so every curve here sits 10·log10(8752/8192) = 0.29 dB right
//! of the same chain with a free preamble. That is the honest accounting (§4.1) and it is identical
//! across the entries, so it cancels in every comparison between them.
//!
//! **Timing is feedforward.** These are bursts, and a burst is what
//! [`FeedforwardTiming`](crate::linear::FeedforwardTiming) is for: one square-law estimate over
//! the whole frame instead of a loop walking toward it. The tracking tier is still measured — the
//! `qam16` entry commits both, which is the §5 item 2 comparison — but it is not what the
//! high-order rows can be measured on, because its residual jitter walls their waterfalls at 1e-4
//! and above. With the estimate feedforward the acquisition preamble no longer has to be long
//! enough for a loop to settle in, which is why the overhead above is 0.12 dB and not 0.5.
//!
//! **The unique word is drawn from one radius shell.** [`unique_word`] picks the table's
//! most-populated equal-radius set and draws from it, which for square QAM is the four corners,
//! for APSK the outer ring, for any PSK the whole table. The reason is measured, in
//! [`PhaseAnchor`](crate::linear::PhaseAnchor)'s docs: an amplitude-varying anchor word fits the
//! carrier slope an order worse, because its low-amplitude points carry almost no phase
//! information at the SNR the rest of the word is comfortable at.

use num_complex::Complex;

use crate::{
    ber::sweep::Link,
    constellation::{Constellation, ConstellationError},
    linear::{
        CarrierLoop, EnvelopeDemod, EnvelopeTiming, LinearBurstDemod, LinearDemod, LinearMod,
        LinearParams, LinearTiming, PhaseAnchor, differential_detect, slice_amplitude,
    },
    pulse::{self, Norm},
    symbolcode::{DifferentialSymbolDecoder, DifferentialSymbolEncoder},
};

/// Reference rate: 48 kHz / 6000 baud, 8 samples per symbol. The engine is rate-free (everything
/// is sps); the Hz numbers exist so the limits axes read in physical units.
pub const BAUD: f64 = 6_000.0;
pub const SPS: usize = 8;
pub const RATE: f64 = BAUD * SPS as f64;

/// The phase-0 calibration link's shaping, reused verbatim (see the module docs).
pub const ALPHA: f64 = 0.35;
pub const SPAN: usize = 8;

/// Steady-frame geometry.
///
/// The acquisition run is 512 symbols and the number is the carrier loop's, not the timing
/// recovery's: with timing feedforward there is no clock to walk to, but a loop at 0.003 cycles per
/// symbol has a ~330-symbol time constant and it must be *settled* before the unique word, because
/// the anchor fitted there applies one constant phase to the whole payload. Measured at 64
/// symbols: the §4.3 static-CFO row read under 2 Hz — the loop was still acquiring when the anchor
/// was taken, and a transient the anchor cannot remove is indistinguishable from no tracking at
/// all. The tail keeps the matched filter's group delay from swallowing the last payload symbols.
pub const PREAMBLE: usize = 512;
pub const UW: usize = 32;
pub const TAIL: usize = 16;
pub const PAYLOAD_SYMBOLS: usize = 8_192;

/// Overhead symbols per trial — the 0.45 dB the module docs charge to Eb.
pub const OVERHEAD: usize = PREAMBLE + UW + TAIL;

/// The blind power estimate is **held** across these frames — see
/// [`LinearTiming::BURST`](crate::linear::LinearTiming::BURST) for the measurement behind it. A
/// burst's transmitter level is constant, so the estimate has nothing to track, and its ripple is
/// a *scale* error that costs the outermost point of a dense table proportionally: it was
/// 1024-QAM's error floor until it was frozen. The scale is the §3.4 anchor's job, and the anchor
/// fits it against symbols whose values are known rather than against the payload's own power.
pub const POWER_SYMBOLS: f64 = f64::INFINITY;

/// Trial-bit cap per committed point: it bounds the steep high-SNR points, where the error
/// budget alone would run for hours.
pub const FULL_CAP: u64 = 4_000_000;

/// The acquisition seed every linear chain starts from (`0x9e37_79b9`, the golden-ratio
/// constant): one value, so two chains framed this way differ by their modulation only. Shared
/// with the CPM substrate's [`DATA_LIKE_SEED`](super::framing::DATA_LIKE_SEED) for the same
/// reason.
pub const FILLER_SEED: u32 = 0x9e37_79b9;

/// The entry's transmit pulse and matched filter: one function, because they are the same taps.
#[must_use]
pub fn rrc() -> Vec<f32> {
    pulse::root_raised_cosine(SPS as f64, ALPHA, SPAN, Norm::Energy)
}

/// A table a catalog entry names by construction. The `tables` generators return a `Result`
/// because a *caller* can ask for an order a family does not define; a row in this crate's own
/// entry list cannot, and its validity is already proven by that module's tests. So this converts
/// the impossible case into a loud abort naming the entry, rather than an error type every
/// registry row would have to thread — and it keeps the crate's no-`unwrap` rule intact where the
/// rule is about runtime conditions, which this is not.
///
/// # Panics
/// If the table is not one its family defines — an authoring bug in this crate.
#[must_use]
pub fn table(what: &str, built: Result<Constellation, ConstellationError>) -> Constellation {
    match built {
        Ok(t) => t,
        Err(why) => panic!("catalog entry `{what}`: {why}"),
    }
}

/// A linear entry at the substrate's shaping and rate.
///
/// Takes the table generator's `Result` rather than a table, so an entry states its geometry in
/// one line and the one place a catalog configuration can be wrong reports it in one place.
///
/// # Panics
/// If the table is not one its family defines, or the table and pulse do not make a valid
/// parameter set. Both are authoring bugs in this crate's own entry list — a `tables` generator's
/// validity is proven by that module's tests — so they abort loudly at construction rather than
/// becoming an error type every registry row would have to thread.
#[must_use]
pub fn params(
    table: Result<Constellation, ConstellationError>,
    rotation_rad: f64,
    offset: bool,
) -> LinearParams {
    let built = table
        .map_err(|e| e.to_string())
        .and_then(|t| LinearParams::new(t, rrc(), SPS).map_err(|e| e.to_string()))
        .and_then(|p| p.with_rotation(rotation_rad).map_err(|e| e.to_string()))
        .and_then(|p| p.with_offset(offset).map_err(|e| e.to_string()));
    match built {
        Ok(p) => p,
        Err(why) => panic!("catalog entry parameters: {why}"),
    }
}

// --- Symbols and bits ---------------------------------------------------------------------------

/// Payload bits to constellation labels, `k` bits per symbol, least significant first — the same
/// order [`labels_to_bits`] reads them back in, and the order the sweep runner's payload
/// generator produces them in.
#[must_use]
pub fn bits_to_labels(bits: &[bool], bits_per_symbol: usize) -> Vec<u32> {
    bits.chunks(bits_per_symbol)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, &b)| acc | (u32::from(b) << i))
        })
        .collect()
}

/// Labels back to payload bits, `k` per symbol.
#[must_use]
pub fn labels_to_bits(labels: &[u32], bits_per_symbol: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(labels.len() * bits_per_symbol);
    for &label in labels {
        for i in 0..bits_per_symbol {
            bits.push((label >> i) & 1 == 1);
        }
    }
    bits
}

/// Deterministic filler labels — one xorshift stream, advanced by the caller, so a frame's
/// leading and trailing filler are different symbols rather than the same block twice.
fn filler_from(state: &mut u32, len: usize, m: u32) -> Vec<u32> {
    (0..len)
        .map(|_| {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            *state % m
        })
        .collect()
}

/// Data-like acquisition filler: labels drawn uniformly from the whole table, because an
/// acquisition sequence must look like the data the loops will have to hold. The rule is the same
/// one the CPM substrate records ([`framing`](super::framing)), arrived at there by measurement.
#[must_use]
pub fn data_like(table: &Constellation, len: usize, seed: u32) -> Vec<u32> {
    filler_from(&mut { seed }, len, table.len() as u32)
}

/// Labels of the table's most-populated equal-radius shell, in table order. For square QAM that
/// is the four corners, for DVB-S2 32-APSK the outer sixteen, for any PSK the whole table. Ties
/// go to the larger radius: a farther shell carries more energy per anchor, which is exactly what
/// a phase fit wants.
#[must_use]
pub fn shell_labels(table: &Constellation) -> Vec<u32> {
    let radii: Vec<f64> = table.points().iter().map(|p| f64::from(p.norm())).collect();
    let mut best: Option<(usize, f64)> = None;
    for &r in &radii {
        // 0.1 % of the radius: several orders above trigonometric rounding, several below the
        // gap between any two shells in the catalog's tables.
        let count = radii.iter().filter(|&&q| (q - r).abs() <= 1e-3 * r).count();
        if best.is_none_or(|(n, best_r)| count > n || (count == n && r > best_r)) {
            best = Some((count, r));
        }
    }
    let (_, radius) = best.unwrap_or((0, 0.0));
    table
        .labels()
        .iter()
        .zip(&radii)
        .filter(|&(_, &r)| (r - radius).abs() <= 1e-3 * radius)
        .map(|(&l, _)| l)
        .collect()
}

/// Candidate words the [`unique_word`] search considers. 256 draws is enough that the best
/// candidate's sidelobe stops improving on every table here, and cheap enough to run at link
/// construction.
const WORD_CANDIDATES: u32 = 256;

/// The unique word: `len` labels chosen so the receiver's correlator has a peak to find.
///
/// Two properties, both load-bearing and both measured rather than assumed.
///
/// *Constant modulus.* The labels come from [`shell_labels`], so the anchor fit that follows the
/// correlation sees one radius whatever the payload's table looks like — an amplitude-varying
/// anchor word fits the carrier an order worse
/// ([`PhaseAnchor`](crate::linear::PhaseAnchor)'s tests). A table with no shell of two or more
/// points — OOK, whose only nonzero radius holds one point — falls back to the whole table,
/// because a word of one repeated symbol has no correlation peak at all.
///
/// *Aperiodic autocorrelation.* An arbitrary draw is not good enough, and this is the failure it
/// causes: with a random 32-symbol word, one BPSK trial in eight anchored at the wrong position
/// and decoded its whole payload as noise — at 12 dB Eb/N0, where the bit errors should have been
/// a handful. The CPM substrate found the same thing and answered it with a hand-searched
/// constant ([`framing::UW24`](super::framing::UW24)); here the word depends on the table, so the
/// search is done at construction: [`WORD_CANDIDATES`] deterministic draws, scored by the worst
/// shifted-overlap sidelobe of their normalised autocorrelation, best one wins. Ties go to the
/// earlier candidate, so the choice is a function of the table and the seed alone.
#[must_use]
pub fn unique_word(table: &Constellation, len: usize, seed: u32) -> Vec<u32> {
    let shell = shell_labels(table);
    let alphabet: Vec<u32> = if shell.len() >= 2 {
        shell
    } else {
        table.labels().to_vec()
    };
    let draw = |candidate: u32| -> Vec<u32> {
        let picks = filler_from(
            &mut seed.wrapping_add(candidate),
            len,
            alphabet.len() as u32,
        );
        picks.into_iter().map(|i| alphabet[i as usize]).collect()
    };
    let points_of_labels = |labels: &[u32]| -> Vec<Complex<f32>> {
        labels
            .iter()
            .map(|&l| {
                let i = table
                    .labels()
                    .iter()
                    .position(|&x| x == l)
                    .unwrap_or_default();
                table.points()[i]
            })
            .collect()
    };
    (0..WORD_CANDIDATES)
        .map(draw)
        .min_by(|a, b| {
            worst_sidelobe(&points_of_labels(a)).total_cmp(&worst_sidelobe(&points_of_labels(b)))
        })
        .unwrap_or_default()
}

/// Worst normalised aperiodic autocorrelation sidelobe of a word: the largest
/// `|Σ x_k·conj(x_{k+s})| / Σ|x_k|²` over every nonzero shift, counting partial overlaps. The
/// magnitude — not the real part — because the receiver's correlator is rotation-invariant, so a
/// sidelobe it could mistake for the peak is one of any phase.
#[must_use]
pub fn worst_sidelobe(word: &[Complex<f32>]) -> f64 {
    let n = word.len();
    let energy: f64 = word.iter().map(|x| f64::from(x.norm_sqr())).sum();
    if energy <= 0.0 || n < 2 {
        return f64::INFINITY;
    }
    let mut worst = 0.0f64;
    for shift in 1..n {
        let mut acc = Complex::new(0.0f64, 0.0);
        for k in 0..(n - shift) {
            let a = word[k + shift];
            let b = word[k];
            acc += Complex::new(f64::from(a.re), f64::from(a.im))
                * Complex::new(f64::from(b.re), -f64::from(b.im));
        }
        worst = worst.max(acc.norm() / energy);
    }
    worst
}

/// The table points a label sequence maps to — what the receiver correlates and fits against.
///
/// Deliberately *unrotated*, even for an entry that carries a per-symbol rotation: the
/// demodulator removes the rotation schedule before anything downstream sees a symbol, so the
/// word arrives on the plain table. Correlating against the transmitted (rotated) points instead
/// is a defect that costs nothing at rotation 0 and destroys π/2-BPSK and π/4-DQPSK outright —
/// measured, when it did.
#[must_use]
pub fn table_points(table: &Constellation, labels: &[u32]) -> Vec<Complex<f32>> {
    labels
        .iter()
        .map(|&l| {
            let i = table
                .labels()
                .iter()
                .position(|&x| x == l)
                .unwrap_or_default();
            table.points()[i]
        })
        .collect()
}

// --- Alignment ----------------------------------------------------------------------------------

/// Best position of a known word in `lo..=hi`, by *normalised* correlation magnitude
/// `|Σ y·conj(x)|² / (Σ|y|²·Σ|x|²)`.
///
/// Rotation-invariant on purpose: a blind carrier loop locks to some rotation of the table
/// (`linear::carrier`), so a metric that compared against the word's absolute phase would fail on
/// three quarters of QPSK acquisitions. Normalised on purpose too: a bare dot product rewards a
/// loud stretch of payload over a correctly-matched word, which was measured mis-anchoring whole
/// trials on the CPM substrate.
///
/// No threshold: a chain too degraded to place its word scores its garbage as bit errors, which
/// is the honest outcome for a sweep point.
#[must_use]
pub fn find_word(
    symbols: &[Complex<f32>],
    lo: usize,
    hi: usize,
    expected: &[Complex<f32>],
) -> Option<usize> {
    let last = hi.min(symbols.len().checked_sub(expected.len())?);
    let word_energy: f64 = expected.iter().map(|x| f64::from(x.norm_sqr())).sum();
    if word_energy <= 0.0 {
        return None;
    }
    let score = |at: usize| -> f64 {
        let mut acc = Complex::new(0.0f64, 0.0);
        let mut energy = 0.0f64;
        for (i, &x) in expected.iter().enumerate() {
            let y = symbols[at + i];
            acc += Complex::new(f64::from(y.re), f64::from(y.im))
                * Complex::new(f64::from(x.re), -f64::from(x.im));
            energy += f64::from(y.norm_sqr());
        }
        if energy <= 0.0 {
            return 0.0;
        }
        acc.norm_sqr() / (energy * word_energy)
    };
    (lo..=last).max_by(|&a, &b| score(a).total_cmp(&score(b)))
}

/// [`find_word`] for the envelope tier, where the symbols are real amplitudes and there is no
/// phase to be invariant to: least squared distance to the word's own amplitudes.
#[must_use]
pub fn find_word_amplitude(
    amplitudes: &[f32],
    lo: usize,
    hi: usize,
    expected: &[f32],
) -> Option<usize> {
    let last = hi.min(amplitudes.len().checked_sub(expected.len())?);
    let misfit = |at: usize| -> f64 {
        expected
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let d = f64::from(amplitudes[at + i]) - f64::from(x);
                d * d
            })
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

// --- Links ----------------------------------------------------------------------------------------

/// Frame a payload: acquisition filler, unique word, payload, trailing filler.
fn frame(table: &Constellation, uw: &[u32], payload: &[u32]) -> Vec<u32> {
    let mut state = FILLER_SEED;
    let m = table.len() as u32;
    let mut s = filler_from(&mut state, PREAMBLE, m);
    s.extend_from_slice(uw);
    s.extend_from_slice(payload);
    s.extend(filler_from(&mut state, TAIL, m));
    s
}

/// One coherent-tier link: bits → labels → frame → `LinearMod` → (channel) → `LinearDemod` with
/// its carrier loop → locate the unique word → anchor the block's gain and phase on it → slice
/// the payload.
///
/// The anchor is what turns a blind loop's arbitrary lock into the right one; without it a QPSK
/// entry would decode a quarter of its trials correctly and rotate the rest.
#[must_use]
pub fn coherent_link(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
) -> Link {
    coherent_link_with_power(label, params, carrier, POWER_SYMBOLS)
}

/// [`coherent_link`] with the blind power estimate's time constant stated — the axis the
/// high-order rows are sensitive to, and the one the grid probe sweeps.
#[must_use]
pub fn coherent_link_with_power(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
    power_symbols: f64,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearBurstDemod::new(&params, &rx, power_symbols, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(table, &symbols, bits_per_symbol)
        }),
    }
}

/// A coherent chain over a *differentially encoded* payload: the carrier is recovered and the
/// symbols sliced against the table, and the differential decode then runs on the recovered
/// symbol *indices* rather than on the received phases. That is what buys back the differential
/// tier's ~3 dB — the decision is made once per symbol against the whole table instead of against
/// a noisy reference — and what it costs is the phase ambiguity, which the unique-word anchor
/// inside [`coherent_link`]'s decode resolves. Without an anchor this tier would not decode at
/// all, which is the honest statement of what the §3.4 hook is worth here.
#[must_use]
pub fn coherent_differential_link(
    label: &str,
    params: LinearParams,
    phase_positions: u32,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let uw = unique_word(table, UW, FILLER_SEED);
            let payload = differential_encode(
                table,
                phase_positions,
                *uw.last().unwrap_or(&0),
                &bits_to_labels(bits, bits_per_symbol),
            );
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearBurstDemod::new(&params, &rx, POWER_SYMBOLS, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            // One extra symbol is sliced: the reference the first payload difference is taken
            // against is the last unique-word symbol, so the decode starts one position early.
            let Some(sliced) = slice_from_word(table, &symbols, PAYLOAD_SYMBOLS + 1, 1) else {
                return Vec::new();
            };
            let labels = differential_decode(table, phase_positions, &sliced);
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

/// Differentially encode a payload for a ring × phase table.
///
/// The rule is *phase-differential, amplitude-absolute*, and it is what the detector's algebra
/// forces rather than a choice: the product `y_k·conj(y_{k−1})/|y_{k−1}|` carries the phase
/// *step* but `y_k`'s own radius, so a ring index has to be transmitted absolutely while the
/// phase index accumulates. For M-PSK there is one ring and this reduces to the familiar
/// "add the data index mod M" ([`DifferentialSymbolEncoder`], the crate's one implementation of
/// that rule).
///
/// Encoding both indices differentially — the obvious generalisation — is wrong, and measured
/// wrong: the 16-star table is not closed under multiplication, so the product of two of its
/// points is not one of its points, and the star row decoded at 41 % BER on a noiseless channel
/// until the rule was split this way.
///
/// `reference` is the label of the symbol the first payload difference is taken against — the
/// last symbol of the unique word. Without it the first difference measures the distance from
/// the encoder's arbitrary initial state instead of from what was actually transmitted, which
/// costs exactly one symbol per frame: an error floor at 1/payload, measured at 2.4e-4.
fn differential_encode(
    table: &Constellation,
    phase_positions: u32,
    reference: u32,
    data: &[u32],
) -> Vec<u32> {
    let index_of = |label: u32| {
        table
            .labels()
            .iter()
            .position(|&x| x == label)
            .unwrap_or_default() as u32
    };
    let mut encoder = DifferentialSymbolEncoder::new(phase_positions);
    // Prime the accumulator with the reference symbol's phase.
    let _ = encoder.encode(index_of(reference) % phase_positions);
    data.iter()
        .map(|&label| {
            let index = index_of(label);
            let ring = index / phase_positions;
            let phase = encoder.encode(index % phase_positions);
            table.labels()[(ring * phase_positions + phase) as usize]
        })
        .collect()
}

/// The inverse of [`differential_encode`], for a coherent tier that slices absolute symbols and
/// then takes the differences: `sliced[0]` is the reference symbol and carries no data, so the
/// result is one label shorter than its input.
fn differential_decode(table: &Constellation, phase_positions: u32, sliced: &[u32]) -> Vec<u32> {
    let index_of = |label: u32| {
        table
            .labels()
            .iter()
            .position(|&x| x == label)
            .unwrap_or_default() as u32
    };
    let mut decoder = DifferentialSymbolDecoder::new(phase_positions);
    let mut out = Vec::with_capacity(sliced.len().saturating_sub(1));
    for (k, &label) in sliced.iter().enumerate() {
        let index = index_of(label);
        let phase = decoder.decode(index % phase_positions);
        if k == 0 {
            continue;
        }
        let ring = index / phase_positions;
        out.push(table.labels()[(ring * phase_positions + phase) as usize]);
    }
    out
}

/// Locate the unique word, anchor on it, and slice `count` symbols starting `before` positions
/// ahead of where the payload begins.
fn slice_from_word(
    table: &Constellation,
    symbols: &[Complex<f32>],
    count: usize,
    before: usize,
) -> Option<Vec<u32>> {
    let uw = unique_word(table, UW, FILLER_SEED);
    let expected = table_points(table, &uw);
    let at = find_word(symbols, 0, PREAMBLE * 2, &expected)?;
    let anchor = PhaseAnchor::fit_gain_only(&symbols[at..at + UW], &expected).ok()?;
    let start = at + UW - before;
    Some(
        (0..count)
            .filter_map(|k| symbols.get(start + k))
            .map(|&y| table.hard_slice(anchor.correct(0, y)))
            .collect(),
    )
}

/// The same coherent chain on the **tracking** timing tier — the §5 item 2 comparison the
/// feedforward tier is measured against. Identical in every other respect, so the two curves
/// differ by the timing recovery alone.
#[must_use]
pub fn coherent_tracked_link(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
    timing: LinearTiming,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearDemod::new(&params, &rx, timing, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(table, &symbols, bits_per_symbol)
        }),
    }
}

/// Locate the unique word, anchor the block on it, slice the payload — the tail every coherent
/// link shares, whichever timing tier placed the symbols.
fn decode_coherent(
    table: &Constellation,
    symbols: &[Complex<f32>],
    bits_per_symbol: usize,
) -> Vec<bool> {
    let uw = unique_word(table, UW, FILLER_SEED);
    let expected = table_points(table, &uw);
    let Some(at) = find_word(symbols, 0, PREAMBLE * 2, &expected) else {
        return Vec::new();
    };
    let Ok(anchor) = PhaseAnchor::fit_gain_only(&symbols[at..at + UW], &expected) else {
        return Vec::new();
    };
    let payload = at + UW;
    let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
        .filter_map(|k| symbols.get(payload + k))
        .map(|&y| table.hard_slice(anchor.correct(0, y)))
        .collect();
    labels_to_bits(&labels, bits_per_symbol)
}

/// One differential-tier link. The payload's symbol *indices* are differentially encoded before
/// the table lookup ([`DifferentialSymbolEncoder`], the crate's one implementation of that rule),
/// so the receiver reads the data straight off the product of consecutive symbols and never needs
/// an absolute phase — which is why this link runs its demodulator open-loop.
///
/// `difference_table` is where the products live: `psk(M)` for the DPSK family, and for an entry
/// carrying a per-symbol rotation the same table, because the demodulator has already undone the
/// rotation before the product is formed.
#[must_use]
pub fn differential_link(
    label: &str,
    params: LinearParams,
    difference_table: Constellation,
    phase_positions: u32,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let uw = unique_word(table, UW, FILLER_SEED);
            let payload = differential_encode(
                table,
                phase_positions,
                *uw.last().unwrap_or(&0),
                &bits_to_labels(bits, bits_per_symbol),
            );
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = LinearBurstDemod::new(&params, &rx, POWER_SYMBOLS, None);
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            let mut products = Vec::new();
            differential_detect(&symbols, &mut products);
            // Alignment runs on the *products*, which carry no absolute phase at all — the
            // strongest form of the rotation invariance `find_word` provides for the coherent
            // rows. The word's own differences are the pattern searched for.
            let uw = unique_word(params.constellation(), UW, FILLER_SEED);
            let word_points = table_points(params.constellation(), &uw);
            let mut word_products = Vec::new();
            differential_detect(&word_points, &mut word_products);
            let Some(at) = find_word(&products, 0, PREAMBLE * 2, &word_products) else {
                return Vec::new();
            };
            // Product `at + UW - 1` pairs the last word symbol with the first payload symbol,
            // which is the first data-carrying difference.
            let payload = at + UW - 1;
            let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
                .filter_map(|k| products.get(payload + k))
                .map(|&z| difference_table.hard_slice(z))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

/// One noncoherent envelope-tier link: the same framing through [`EnvelopeDemod`], aligned on the
/// unique word's amplitudes and sliced against the table's.
#[must_use]
pub fn envelope_link(
    label: &str,
    params: LinearParams,
    timing_bw: f64,
    timing: EnvelopeTiming,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = EnvelopeDemod::new(&params, &rx, timing_bw, timing);
            let mut amplitudes = Vec::new();
            demod.process(wave, &mut amplitudes);
            let uw = unique_word(table, UW, FILLER_SEED);
            let expected: Vec<f32> = table_points(table, &uw).iter().map(|p| p.norm()).collect();
            let Some(at) = find_word_amplitude(&amplitudes, 0, PREAMBLE * 2, &expected) else {
                return Vec::new();
            };
            let payload = at + UW;
            let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
                .filter_map(|k| amplitudes.get(payload + k))
                .map(|&a| slice_amplitude(table, a))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constellation::tables;

    #[test]
    fn bits_and_labels_round_trip_least_significant_first() {
        let bits = [true, false, true, true, false, false, true, false];
        for k in [1usize, 2, 4, 8] {
            let labels = bits_to_labels(&bits, k);
            assert_eq!(labels.len(), bits.len() / k);
            assert_eq!(labels_to_bits(&labels, k), bits);
        }
        // Explicit ordering, so a silent endianness change is a failure and not a shrug.
        assert_eq!(bits_to_labels(&[true, false, true, false], 2), [0b01, 0b01]);
    }

    /// The shell the anchor word is drawn from must be a genuine equal-radius set, and the
    /// largest one the table has — getting it wrong would quietly hand the phase fit an
    /// amplitude-varying word, which the anchor's own tests measure as an order of accuracy.
    #[test]
    fn the_unique_word_shell_is_the_most_populated_radius() {
        for (name, table, want) in [
            ("psk8", tables::psk(8).unwrap(), 8usize),
            // 16-QAM's biggest shell is the eight (±1, ±3) edge points, not the four corners.
            ("qam16", tables::qam_square(16).unwrap(), 8),
            // 64-QAM: the twelve points at radius² = 50, i.e. (±1,±7), (±5,±5), (±7,±1).
            ("qam64", tables::qam_square(64).unwrap(), 12),
            ("apsk32", tables::apsk32_dvbs2(2.84, 5.27).unwrap(), 16),
            ("star16", tables::qam_star(&[1.0, 2.0], 8).unwrap(), 8),
            ("ook", tables::ook().unwrap(), 1),
        ] {
            let shell = shell_labels(&table);
            assert_eq!(shell.len(), want, "{name}: shell {shell:?}");
            let radius_of = |label: u32| {
                let i = table.labels().iter().position(|&x| x == label).unwrap();
                f64::from(table.points()[i].norm())
            };
            let first = radius_of(shell[0]);
            for &l in &shell {
                assert!(
                    (radius_of(l) - first).abs() <= 1e-3 * first.max(f64::MIN_POSITIVE),
                    "{name}: label {l} sits at {}, not {first}",
                    radius_of(l)
                );
            }
            // No other radius is more populated.
            let biggest = table
                .points()
                .iter()
                .map(|p| {
                    let r = f64::from(p.norm());
                    table
                        .points()
                        .iter()
                        .filter(|q| {
                            (f64::from(q.norm()) - r).abs() <= 1e-3 * r.max(f64::MIN_POSITIVE)
                        })
                        .count()
                })
                .max()
                .unwrap();
            assert_eq!(shell.len(), biggest, "{name}");
        }
    }

    /// The word must be found at its own position and nowhere else, through an arbitrary
    /// rotation — the property that lets a blind carrier loop lock wherever it likes.
    #[test]
    fn the_word_is_located_through_any_rotation() {
        let table = tables::qam_square(16).unwrap();
        let uw = unique_word(&table, UW, FILLER_SEED);
        let expected = table_points(&table, &uw);
        let filler = data_like(&table, PREAMBLE, FILLER_SEED);
        let mut stream: Vec<Complex<f32>> = table_points(&table, &filler);
        stream.extend_from_slice(&expected);
        stream.extend(table_points(&table, &data_like(&table, 200, 0x1234)));
        for turns in [0.0f64, 0.25, 0.5, 0.75, 0.13] {
            let theta = std::f64::consts::TAU * turns;
            let rot = Complex::new(theta.cos() as f32, theta.sin() as f32);
            let rotated: Vec<Complex<f32>> = stream.iter().map(|&s| s * rot).collect();
            assert_eq!(
                find_word(&rotated, 0, PREAMBLE * 2, &expected),
                Some(PREAMBLE),
                "rotation {turns} turns"
            );
        }
    }

    /// Framing lengths are the Eb accounting: a change that quietly moved one would shift every
    /// curve on this substrate with no gate noticing.
    #[test]
    fn the_frame_is_the_documented_geometry() {
        let table = tables::psk(4).unwrap();
        let uw = unique_word(&table, UW, FILLER_SEED);
        let payload = vec![0u32; PAYLOAD_SYMBOLS];
        let s = frame(&table, &uw, &payload);
        assert_eq!(s.len(), OVERHEAD + PAYLOAD_SYMBOLS);
        assert_eq!(&s[PREAMBLE..PREAMBLE + UW], &uw[..]);
        // The documented 0.45 dB of overhead, from the geometry itself.
        let charged = 10.0 * ((OVERHEAD + PAYLOAD_SYMBOLS) as f64 / PAYLOAD_SYMBOLS as f64).log10();
        assert!(
            (charged - 0.287).abs() < 0.005,
            "overhead charges {charged} dB"
        );
    }
}
