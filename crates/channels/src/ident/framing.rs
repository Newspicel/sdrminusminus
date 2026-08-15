use num_complex::Complex;
use sdrmm_dsp::{Ddc, hamming_distance};
use sdrmm_modem::cpm::{CpmDemod, TIMING_BW_BURST};
use sdrmm_wire::ProtocolMatch;

use super::detect::Band;
use crate::dv::{INPUT_RATE_HZ, MODE_SIGNATURES, ModeSignature};

const SEARCH_SECONDS: f64 = 0.55;

const SETTLE: usize = 96;

const DEMOTION: f32 = 0.4;

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

fn baseband(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Vec<Complex<f32>>> {
    let wanted = (SEARCH_SECONDS * rate) as usize;
    let tail = &iq[iq.len().saturating_sub(wanted)..];
    let mut ddc = Ddc::new(rate, INPUT_RATE_HZ, band.center_hz).ok()?;
    let mut out = Vec::with_capacity((tail.len() as f64 * INPUT_RATE_HZ / rate) as usize + 1);
    ddc.process(tail, &mut out);
    (out.len() > SETTLE * 2).then_some(out)
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
