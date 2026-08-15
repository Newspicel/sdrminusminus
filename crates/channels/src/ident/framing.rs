//! Stage five of identification: prove it.
//!
//! The land-mobile digital modes are the one family measurement cannot separate. DMR, P25 Phase
//! 1, System Fusion and 12.5 kHz NXDN are all four-level, all 4800 symbols a second, all ±1944 Hz
//! in a 12.5 kHz channel — nothing about the *signal* tells them apart. What does is the frame
//! sync each one opens its bursts with, so the shortlist is settled by demodulating against each
//! candidate's own front end and looking for its own pattern.
//!
//! A hit here outranks every resemblance: the identifier found the protocol's framing, not
//! something shaped like it.

use num_complex::Complex;
use sdrmm_dsp::{Ddc, hamming_distance};
use sdrmm_modem::cpm::{CpmDemod, TIMING_BW_BURST};
use sdrmm_wire::ProtocolMatch;

use super::detect::Band;
use crate::dv::{INPUT_RATE_HZ, MODE_SIGNATURES, ModeSignature};

/// Longest span the search demodulates, in seconds. Sized by the slowest sync cadence in the
/// family — D-STAR repeats its frame sync once per 21-frame superframe, 420 ms — and bounded
/// because this runs on the DSP thread once per report.
const SEARCH_SECONDS: f64 = 0.55;

/// Output samples discarded while the front end's filter chain fills.
const SETTLE: usize = 96;

/// What a candidate keeps of its measured score when a sibling's framing was found instead. Not
/// zero: the waveform still fits, and the operator is entitled to see what else it could be.
const DEMOTION: f32 = 0.4;

/// Re-rank `candidates` by looking for the framing of each one that names a mode with framing to
/// look for. Candidates outside that family are left exactly as they are.
pub(crate) fn confirm(
    candidates: &mut [ProtocolMatch],
    iq: &[Complex<f32>],
    rate: f64,
    band: &Band,
) {
    let searchable: Vec<(usize, &ModeSignature)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let mode = MODE_SIGNATURES
                .iter()
                .find(|mode| candidate.name == mode.name)?;
            Some((index, mode))
        })
        .collect();
    if searchable.is_empty() {
        return;
    }
    let Some(baseband) = baseband(iq, rate, band) else {
        return;
    };

    let mut confirmed_any = false;
    let mut soft = Vec::new();
    for group in group_by_waveform(&searchable) {
        let Some(&(_, reference)) = group.first() else {
            continue;
        };
        // Both timing phases. A symbol-clock recovery loop handed a transmission that is already
        // in progress — which is every transmission an identifier ever sees — acquires from some
        // starting phases and not others, and half a symbol period apart is enough to cover the
        // ones it does not. A decoder can afford to wait for the next burst edge; a search over
        // one observation window cannot.
        let stagger = (reference.params.sps() / 2.0) as usize;
        for offset in [0, stagger] {
            if offset >= baseband.len() {
                continue;
            }
            let mut demod = CpmDemod::new(
                &reference.params,
                &reference.receive_filter,
                TIMING_BW_BURST,
            );
            demod.process(&baseband[offset..], &mut soft);
            if soft.len() <= SETTLE {
                continue;
            }
            for &(index, mode) in &group {
                if hits(&soft[SETTLE..], mode) >= mode.min_hits {
                    candidates[index].confirmed = true;
                    candidates[index].score = 1.0;
                    candidates[index].why = format!("{} frame sync found in the signal", mode.name);
                    confirmed_any = true;
                }
            }
        }
    }

    if confirmed_any {
        for (index, _) in searchable {
            if !candidates[index].confirmed {
                candidates[index].score *= DEMOTION;
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
}

/// The detected band at the digital-voice front-end rate, over the search span only.
fn baseband(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Vec<Complex<f32>>> {
    let wanted = (SEARCH_SECONDS * rate) as usize;
    let tail = &iq[iq.len().saturating_sub(wanted)..];
    let mut ddc = Ddc::new(rate, INPUT_RATE_HZ, band.center_hz).ok()?;
    let mut out = Vec::with_capacity((tail.len() as f64 * INPUT_RATE_HZ / rate) as usize + 1);
    ddc.process(tail, &mut out);
    (out.len() > SETTLE * 2).then_some(out)
}

/// How many times `mode`'s sync appears in a stream of its own soft symbols.
fn hits(soft: &[f32], mode: &ModeSignature) -> u32 {
    let mapping = mode.params.mapping();
    let bits_per_symbol = mapping.bits_per_symbol();
    let (offset, scale) = level_fit(soft, mapping);
    let mask = if mode.sync_bits >= 64 {
        u64::MAX
    } else {
        (1u64 << mode.sync_bits) - 1
    };
    let mut register = 0u64;
    let mut found = 0;
    for (position, &symbol) in soft.iter().enumerate() {
        register =
            register << bits_per_symbol | u64::from(mapping.slice((symbol - offset) * scale));
        // Nothing to compare until the register has actually seen a whole pattern's worth.
        if (position + 1) * bits_per_symbol as usize <= mode.sync_bits as usize {
            continue;
        }
        if mode
            .patterns
            .iter()
            .any(|&pattern| hamming_distance(register & mask, pattern & mask) <= mode.tolerance)
        {
            found += 1;
        }
    }
    found
}

/// The centre and scale that put the soft symbols onto the mode's own level table.
///
/// The decoders anchor their level estimate on a sync they have already matched; a search that
/// has not matched anything yet has no such anchor and must fit blind. Two corrections, both
/// necessary. The centre absorbs the residual tuning error — the band's measured centroid is a
/// spectrum estimate, and every hertz it is out by arrives here as a constant offset on a
/// *frequency* discriminator's output. The scale matches the root-mean-square, so a transmitter
/// running a few percent off its nominal deviation still slices at the right thresholds.
///
/// Both rest on the symbols being roughly balanced over a window this long, which every one of
/// these modes' scrambled or coded payloads is. Without them the outer symbols slice as inner
/// ones and no sync ever matches.
fn level_fit(soft: &[f32], mapping: &sdrmm_modem::cpm::Mapping) -> (f32, f32) {
    let n = soft.len().max(1) as f64;
    let mean = soft.iter().map(|&s| f64::from(s)).sum::<f64>() / n;
    let measured = (soft
        .iter()
        .map(|&s| (f64::from(s) - mean) * (f64::from(s) - mean))
        .sum::<f64>()
        / n)
        .sqrt();
    let levels = mapping.levels();
    let expected = (levels
        .iter()
        .map(|&l| f64::from(l) * f64::from(l))
        .sum::<f64>()
        / levels.len() as f64)
        .sqrt();
    let scale = if measured > f64::MIN_POSITIVE {
        expected / measured
    } else {
        1.0
    };
    (mean as f32, scale as f32)
}

/// Group candidates that share a front end, so one demodulation serves all of them — which is
/// most of the family: DMR, P25, System Fusion and wide NXDN transmit the same waveform.
fn group_by_waveform<'a>(
    searchable: &[(usize, &'a ModeSignature)],
) -> Vec<Vec<(usize, &'a ModeSignature)>> {
    let mut groups: Vec<Vec<(usize, &'a ModeSignature)>> = Vec::new();
    for &entry in searchable {
        match groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|&(_, head): &(usize, &ModeSignature)| same_waveform(head, entry.1))
        }) {
            Some(group) => group.push(entry),
            None => groups.push(vec![entry]),
        }
    }
    groups
}

fn same_waveform(a: &ModeSignature, b: &ModeSignature) -> bool {
    a.baud == b.baud && a.deviation_hz == b.deviation_hz && a.receive_filter == b.receive_filter
}
