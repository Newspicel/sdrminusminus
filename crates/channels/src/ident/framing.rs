use num_complex::Complex;
use sdrmm_dsp::{Ddc, hamming_distance};
use sdrmm_modem::cpm::{CpmDemod, TIMING_BW_BURST};
use sdrmm_wire::{Modulation, ProtocolMatch};

use super::detect::Band;
use crate::dv::{INPUT_RATE_HZ, MODE_SIGNATURES, ModeSignature};

const SEARCH_SECONDS: f64 = 0.55;

const SETTLE: usize = 96;

const DEMOTION: f32 = 0.4;

const CHANCE_MARGIN: f64 = 3.0;

const PROBE_CHANCE_MARGIN: f64 = 10.0;

const PROBE_MIN_HITS: u32 = 3;

const SHORTEST_FRAME_SYMBOLS: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rigour {
    Candidate,
    Probe,
}

pub(crate) struct Probe {
    pub(crate) matches: Vec<ProtocolMatch>,
    pub(crate) modulation: Modulation,
}

pub(crate) fn probe(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Probe> {
    let mut candidates: Vec<ProtocolMatch> = MODE_SIGNATURES
        .iter()
        .map(|mode| ProtocolMatch {
            name: mode.name.to_owned(),
            type_id: Some(mode.type_id.to_owned()),
            score: 0.0,
            confirmed: false,
            why: String::new(),
        })
        .collect();
    confirm_with(&mut candidates, iq, rate, band, Rigour::Probe);
    candidates.retain(|candidate| candidate.confirmed);
    let first = candidates.first()?;
    let levels = MODE_SIGNATURES
        .iter()
        .find(|mode| mode.name == first.name)
        .map_or(4, |mode| mode.params.mapping().m());
    Some(Probe {
        matches: candidates,
        modulation: if levels == 4 {
            Modulation::Fsk4
        } else {
            Modulation::Fsk2
        },
    })
}

pub(crate) fn confirm(
    candidates: &mut [ProtocolMatch],
    iq: &[Complex<f32>],
    rate: f64,
    band: &Band,
) {
    confirm_with(candidates, iq, rate, band, Rigour::Candidate);
}

fn confirm_with(
    candidates: &mut [ProtocolMatch],
    iq: &[Complex<f32>],
    rate: f64,
    band: &Band,
    rigour: Rigour,
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
                if plausible(
                    mode,
                    hits(&soft[SETTLE..], mode),
                    soft.len() - SETTLE,
                    rigour,
                ) {
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
        candidates.sort_by(|a, b| {
            b.confirmed
                .cmp(&a.confirmed)
                .then(b.score.total_cmp(&a.score))
        });
    }
}

fn baseband(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Vec<Complex<f32>>> {
    let wanted = (SEARCH_SECONDS * rate) as usize;
    let tail = &iq[iq.len().saturating_sub(wanted)..];
    let mut ddc = Ddc::new(rate, INPUT_RATE_HZ, band.center_hz).ok()?;
    let mut out = Vec::with_capacity((tail.len() as f64 * INPUT_RATE_HZ / rate) as usize + 1);
    ddc.process(tail, &mut out);
    (out.len() > SETTLE * 2).then_some(out)
}

fn plausible(mode: &ModeSignature, hits: u32, positions: usize, rigour: Rigour) -> bool {
    let (least, margin) = match rigour {
        Rigour::Candidate => (mode.min_hits, CHANCE_MARGIN),
        Rigour::Probe => (mode.min_hits.max(PROBE_MIN_HITS), PROBE_CHANCE_MARGIN),
    };
    let densest = (positions / SHORTEST_FRAME_SYMBOLS) as u32 + 1;
    hits >= least
        && hits <= densest
        && f64::from(hits) >= margin * chance_hits(mode, positions) + 1.0
}

fn chance_hits(mode: &ModeSignature, positions: usize) -> f64 {
    let bits = f64::from(mode.sync_bits);
    let mut within = 0.0;
    let mut choose = 1.0;
    for k in 0..=mode.tolerance {
        if k > 0 {
            choose *= (bits - f64::from(k) + 1.0) / f64::from(k);
        }
        within += choose;
    }
    within / 2f64.powf(bits) * positions as f64 * mode.patterns.len() as f64
}

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
