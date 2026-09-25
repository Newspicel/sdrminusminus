use sdrmm_wire::{
    bandplan::{BandAllocation, BandBlock, BandLane, BandPlan},
    channel::ChannelParams,
};

use super::allocation_at;

const MIN_PIECE: f64 = 0.002;

#[derive(Clone, Debug, PartialEq)]
pub struct BandSpan {
    pub block: BandBlock,
    pub allocation: BandAllocation,
    pub left: f64,
    pub width: f64,
    pub starts_inside: bool,
    pub ends_inside: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BandIdentity {
    pub lane_id: String,
    pub lane_name: String,
    pub block: BandBlock,
    pub allocation: BandAllocation,
    pub covered: Vec<BandAllocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveredGroup {
    pub label: String,
    pub names: Vec<String>,
}

#[must_use]
pub fn spans_in(plan: &BandPlan, lane: &BandLane, low_hz: f64, visible_hz: f64) -> Vec<BandSpan> {
    if visible_hz.is_nan() || visible_hz <= 0.0 {
        return Vec::new();
    }
    let high_hz = low_hz + visible_hz;
    lane.blocks
        .iter()
        .filter(|block| block.stop_hz > low_hz && block.start_hz < high_hz)
        .filter_map(|block| {
            let allocation = allocation_at(plan, block.of)?;
            let left = (block.start_hz - low_hz) / visible_hz;
            let right = (block.stop_hz - low_hz) / visible_hz;
            Some(BandSpan {
                block: block.clone(),
                allocation: allocation.clone(),
                left: left.max(0.0),
                width: right.min(1.0) - left.max(0.0),
                starts_inside: left >= 0.0,
                ends_inside: right <= 1.0,
            })
        })
        .collect()
}

#[must_use]
pub fn identify(plan: &BandPlan, hz: f64) -> Vec<BandIdentity> {
    plan.lanes
        .iter()
        .filter_map(|lane| {
            let block = lane
                .blocks
                .iter()
                .find(|block| block.start_hz <= hz && block.stop_hz > hz)?;
            let allocation = allocation_at(plan, block.of)?;
            Some(BandIdentity {
                lane_id: lane.id.clone(),
                lane_name: lane.name.clone(),
                block: block.clone(),
                allocation: allocation.clone(),
                covered: block
                    .covered
                    .iter()
                    .filter_map(|at| allocation_at(plan, *at).cloned())
                    .collect(),
            })
        })
        .collect()
}

#[must_use]
pub fn covered_by_layer(
    covered: &[BandAllocation],
    label_of: impl Fn(&str) -> String,
) -> Vec<CoveredGroup> {
    let mut groups: Vec<CoveredGroup> = Vec::new();
    for allocation in covered {
        let label = label_of(&allocation.layer);
        let at = match groups.iter().position(|group| group.label == label) {
            Some(at) => at,
            None => {
                groups.push(CoveredGroup {
                    label,
                    names: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let names = &mut groups[at].names;
        if !names.contains(&allocation.name) {
            names.push(allocation.name.clone());
        }
    }
    groups
}

#[must_use]
pub fn provision_text<'a>(plan: &'a BandPlan, layer: &str, id: &str) -> Option<&'a str> {
    plan.provisions
        .iter()
        .find(|found| found.layer == layer && found.id == id)
        .map(|found| found.text.as_str())
}

#[must_use]
pub fn suggested_at(found: &[BandIdentity]) -> Option<ChannelParams> {
    found
        .iter()
        .rev()
        .find_map(|entry| entry.allocation.suggested.clone())
}

fn bandwidth(allocation: &BandAllocation) -> f64 {
    allocation.stop_hz - allocation.start_hz
}

fn free_parts(left: f64, right: f64, taken: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut parts = vec![(left, right)];
    for (from, to) in taken {
        parts = parts
            .into_iter()
            .flat_map(|(a, b)| {
                if *to <= a || *from >= b {
                    return vec![(a, b)];
                }
                let mut kept = Vec::new();
                if *from > a {
                    kept.push((a, *from));
                }
                if *to < b {
                    kept.push((*to, b));
                }
                kept
            })
            .collect();
    }
    parts
}

fn merge_neighbours(pieces: Vec<BandSpan>) -> Vec<BandSpan> {
    let mut merged: Vec<BandSpan> = Vec::new();
    for piece in pieces {
        if let Some(last) = merged.last_mut()
            && last.allocation.name == piece.allocation.name
            && last.allocation.service == piece.allocation.service
            && (last.left + last.width - piece.left).abs() < MIN_PIECE
        {
            last.width = piece.left + piece.width - last.left;
            last.ends_inside = piece.ends_inside;
            continue;
        }
        merged.push(piece);
    }
    merged
}

#[must_use]
pub fn flatten_lanes(lanes: &[Vec<BandSpan>]) -> Vec<BandSpan> {
    let mut narrowest_first: Vec<&BandSpan> = lanes.iter().flatten().collect();
    narrowest_first.sort_by(|a, b| bandwidth(&a.allocation).total_cmp(&bandwidth(&b.allocation)));
    let mut taken: Vec<(f64, f64)> = Vec::new();
    let mut pieces: Vec<BandSpan> = Vec::new();
    for span in narrowest_first {
        for (left, right) in free_parts(span.left, span.left + span.width, &taken) {
            if right - left >= MIN_PIECE {
                pieces.push(BandSpan {
                    left,
                    width: right - left,
                    starts_inside: left > 0.0,
                    ..span.clone()
                });
            }
        }
        taken.push((span.left, span.left + span.width));
    }
    pieces.sort_by(|a, b| a.left.total_cmp(&b.left));
    merge_neighbours(pieces)
}
