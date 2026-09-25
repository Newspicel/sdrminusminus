use super::view::above;
use sdrmm_wire::{
    bandplan::{BandAllocation, BandLane, BandPlan, BandService},
    channel::ChannelParams,
};

#[derive(Clone, Debug, PartialEq)]
pub struct BandSpan {
    pub of: usize,
    pub bandwidth: f64,
    pub left: f64,
    pub width: f64,
    pub starts_inside: bool,
    pub ends_inside: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BandIdentity<'a> {
    pub lane_id: &'a str,
    pub allocation: &'a BandAllocation,
    pub covered: Vec<&'a BandAllocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveredGroup {
    pub label: String,
    pub names: Vec<String>,
}

const MIN_PIECE: f64 = 0.002;

#[must_use]
pub fn spans_in(plan: &BandPlan, lane: &BandLane, low_hz: f64, visible_hz: f64) -> Vec<BandSpan> {
    if !above(visible_hz, 0.0) {
        return Vec::new();
    }
    let high_hz = low_hz + visible_hz;
    lane.blocks
        .iter()
        .filter(|block| block.stop_hz > low_hz && block.start_hz < high_hz)
        .filter_map(|block| {
            let of = block.of as usize;
            let allocation = plan.allocations.get(of)?;
            let left = (block.start_hz - low_hz) / visible_hz;
            let right = (block.stop_hz - low_hz) / visible_hz;
            Some(BandSpan {
                of,
                bandwidth: allocation.stop_hz - allocation.start_hz,
                left: left.max(0.0),
                width: right.min(1.0) - left.max(0.0),
                starts_inside: left >= 0.0,
                ends_inside: right <= 1.0,
            })
        })
        .collect()
}

#[must_use]
pub fn identify(plan: &BandPlan, hz: f64) -> Vec<BandIdentity<'_>> {
    plan.lanes
        .iter()
        .filter_map(|lane| {
            let block = lane
                .blocks
                .iter()
                .find(|block| block.start_hz <= hz && block.stop_hz > hz)?;
            let allocation = plan.allocations.get(block.of as usize)?;
            Some(BandIdentity {
                lane_id: &lane.id,
                allocation,
                covered: block
                    .covered
                    .iter()
                    .filter_map(|at| plan.allocations.get(*at as usize))
                    .collect(),
            })
        })
        .collect()
}

#[must_use]
pub fn covered_by_layer(
    covered: &[&BandAllocation],
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
        if !groups[at].names.contains(&allocation.name) {
            groups[at].names.push(allocation.name.clone());
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
pub fn suggested_at(found: &[BandIdentity<'_>]) -> Option<ChannelParams> {
    found
        .iter()
        .rev()
        .find_map(|entry| entry.allocation.suggested.clone())
}

#[must_use]
pub fn service_name(service: BandService) -> &'static str {
    match service {
        BandService::Amateur => "amateur",
        BandService::Broadcast => "broadcast",
        BandService::Aeronautical => "aeronautical",
        BandService::Maritime => "maritime",
        BandService::Mobile => "mobile",
        BandService::Satellite => "satellite",
        BandService::Navigation => "navigation",
        BandService::Science => "science",
        BandService::Ism => "ism",
        BandService::Other => "other",
    }
}

#[must_use]
pub fn service_label(service: BandService) -> String {
    if service == BandService::Ism {
        return String::from("ISM");
    }
    let name = service_name(service);
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[must_use]
pub fn flatten_lanes(plan: &BandPlan, lanes: &[Vec<BandSpan>]) -> Vec<BandSpan> {
    let mut narrowest_first: Vec<&BandSpan> = lanes.iter().flatten().collect();
    narrowest_first.sort_by(|a, b| a.bandwidth.total_cmp(&b.bandwidth));
    let mut taken: Vec<(f64, f64)> = Vec::new();
    let mut pieces = Vec::new();
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
    merge_neighbours(plan, pieces)
}

fn same_band(plan: &BandPlan, a: usize, b: usize) -> bool {
    match (plan.allocations.get(a), plan.allocations.get(b)) {
        (Some(a), Some(b)) => a.name == b.name && a.service == b.service,
        _ => false,
    }
}

fn merge_neighbours(plan: &BandPlan, pieces: Vec<BandSpan>) -> Vec<BandSpan> {
    let mut merged: Vec<BandSpan> = Vec::new();
    for piece in pieces {
        match merged.last_mut() {
            Some(last)
                if same_band(plan, last.of, piece.of)
                    && (last.left + last.width - piece.left).abs() < MIN_PIECE =>
            {
                last.width = piece.left + piece.width - last.left;
                last.ends_inside = piece.ends_inside;
            }
            _ => merged.push(piece),
        }
    }
    merged
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

#[cfg(test)]
pub(super) mod tests {
    use sdrmm_wire::bandplan::{
        BandBlock, BandLayerInfo, BandLayerKind, BandProvision, BandRegion, ItuRegion,
    };

    use super::*;

    pub fn allocation(id: &str, name: &str, start_hz: f64, stop_hz: f64) -> BandAllocation {
        BandAllocation {
            id: id.to_owned(),
            layer: String::from("world"),
            start_hz,
            stop_hz,
            service: BandService::Other,
            name: name.to_owned(),
            official_name: String::new(),
            primary: true,
            reference: None,
            aliases: Vec::new(),
            suggested: None,
            channel_step_hz: None,
            notes: None,
            provisions: Vec::new(),
        }
    }

    fn params(type_id: &str) -> Option<ChannelParams> {
        serde_json::from_value(serde_json::json!({ "type": type_id, "settings": {} })).ok()
    }

    fn block(of: u32, start_hz: f64, stop_hz: f64) -> BandBlock {
        BandBlock {
            start_hz,
            stop_hz,
            of,
            covered: Vec::new(),
        }
    }

    fn lane(id: &str, blocks: Vec<BandBlock>) -> BandLane {
        BandLane {
            id: id.to_owned(),
            name: id.to_owned(),
            overlay: false,
            blocks,
        }
    }

    pub fn plan() -> BandPlan {
        let mut two_m = allocation("2m", "2 m amateur", 144e6, 146e6);
        two_m.service = BandService::Amateur;
        two_m.suggested = params("nfm");
        let mut aprs = allocation("aprs", "2 m: APRS", 144.794e6, 144.99e6);
        aprs.suggested = params("aprs");
        let mut marine = allocation("marine", "Marine VHF", 156e6, 161.9625e6);
        marine.service = BandService::Maritime;
        let allocations = vec![
            two_m,
            marine,
            allocation("ais", "AIS", 161.9625e6, 162.0375e6),
            aprs,
            allocation("simplex", "2 m: FM simplex", 145.206e6, 145.594e6),
        ];
        BandPlan {
            region: BandRegion {
                id: String::from("de"),
                name: String::from("Germany"),
                country: None,
                itu_region: ItuRegion::R1,
                layers: vec![String::from("world")],
                overlays: Vec::new(),
            },
            layers: vec![BandLayerInfo {
                id: String::from("world"),
                name: String::from("ITU world table"),
                authority: String::from("ITU"),
                source: String::from("RR Article 5"),
                kind: BandLayerKind::World,
                rank: 0,
                generator: String::new(),
            }],
            allocations,
            lanes: vec![
                lane(
                    "allocation",
                    vec![
                        block(0, 144e6, 146e6),
                        block(1, 156e6, 161.9625e6),
                        block(2, 161.9625e6, 162.0375e6),
                    ],
                ),
                lane(
                    "iaru-r1",
                    vec![
                        block(3, 144.794e6, 144.99e6),
                        block(4, 145.206e6, 145.594e6),
                    ],
                ),
            ],
            provisions: vec![BandProvision {
                layer: String::from("de"),
                id: String::from("5"),
                text: String::from("ISM shares these bands."),
            }],
        }
    }

    #[test]
    fn covered_bands_group_by_authority_and_name_each_once() {
        let named = |id: &str, layer: &str, name: &str| {
            let mut found = allocation(id, name, 0.0, 1.0);
            found.layer = layer.to_owned();
            found
        };
        let listed = [
            named("a", "cept", "1800 MHz mobile broadband"),
            named("b", "cept", "1800 MHz mobile broadband"),
            named("c", "world", "FIXED"),
            named("d", "cept", "FIXED"),
            named("e", "itu-r1", "1800 MHz mobile broadband"),
        ];
        let refs: Vec<&BandAllocation> = listed.iter().collect();
        let authority =
            |layer: &str| String::from(if layer == "cept" { "CEPT / ECO" } else { "ITU" });
        let groups = covered_by_layer(&refs, authority);
        assert_eq!(groups[0].label, "CEPT / ECO");
        assert_eq!(groups[0].names, ["1800 MHz mobile broadband", "FIXED"]);
        assert_eq!(groups[1].label, "ITU");
        assert_eq!(groups[1].names, ["FIXED", "1800 MHz mobile broadband"]);
        assert!(covered_by_layer(&[], authority).is_empty());
    }

    #[test]
    fn a_provision_is_read_by_its_layer_and_id() {
        let plan = plan();
        assert_eq!(
            provision_text(&plan, "de", "5"),
            Some("ISM shares these bands.")
        );
        assert_eq!(provision_text(&plan, "de", "D338"), None);
        assert_eq!(provision_text(&plan, "world", "5"), None);
    }

    #[test]
    fn a_span_is_clipped_to_the_window_and_says_which_edges_are_real() {
        let plan = plan();
        let clipped = spans_in(&plan, &plan.lanes[0], 144.5e6, 1e6);
        assert_eq!(clipped[0].left, 0.0);
        assert_eq!(clipped[0].width, 1.0);
        assert!(!clipped[0].starts_inside && !clipped[0].ends_inside);
        let spans = spans_in(&plan, &plan.lanes[0], 161.9e6, 200_000.0);
        let ais = spans.iter().find(|span| span.of == 2).expect("ais");
        assert!((ais.left - 0.3125).abs() < 1e-9);
        assert!((ais.width - 0.375).abs() < 1e-9);
        assert!(ais.starts_inside && ais.ends_inside);
    }

    #[test]
    fn nothing_is_spanned_outside_the_window_or_in_an_empty_one() {
        let plan = plan();
        assert!(spans_in(&plan, &plan.lanes[0], 100e6, 1e6).is_empty());
        assert!(spans_in(&plan, &plan.lanes[0], 143e6, 1e6).is_empty());
        assert!(spans_in(&plan, &plan.lanes[0], 144e6, 0.0).is_empty());
        assert!(spans_in(&plan, &plan.lanes[0], 144e6, -1.0).is_empty());
    }

    #[test]
    fn a_frequency_is_identified_once_per_lane_that_covers_it() {
        let plan = plan();
        let found = identify(&plan, 145.5e6);
        let lanes: Vec<&str> = found.iter().map(|entry| entry.lane_id).collect();
        assert_eq!(lanes, ["allocation", "iaru-r1"]);
        assert_eq!(found[0].allocation.name, "2 m amateur");
        assert_eq!(found[1].allocation.name, "2 m: FM simplex");
        assert_eq!(identify(&plan, 156.8e6).len(), 1);
        assert_eq!(identify(&plan, 161.9625e6)[0].allocation.name, "AIS");
        assert_eq!(
            identify(&plan, 161_962_499.0)[0].allocation.name,
            "Marine VHF"
        );
        assert!(identify(&plan, 1.0).is_empty());
    }

    #[test]
    fn the_most_specific_lane_suggests_the_mode() {
        let plan = plan();
        let type_of = |hz: f64| suggested_at(&identify(&plan, hz)).map(|found| found.type_id());
        assert_eq!(type_of(144.8e6), Some("aprs"));
        assert_eq!(type_of(145.5e6), Some("nfm"));
        assert_eq!(type_of(1.0), None);
        assert_eq!(type_of(156.8e6), None);
    }

    #[test]
    fn an_initialism_is_spelled_as_one() {
        assert_eq!(service_label(BandService::Ism), "ISM");
        assert_eq!(service_label(BandService::Maritime), "Maritime");
    }

    fn span(of: usize, left: f64, width: f64, bandwidth: f64) -> BandSpan {
        BandSpan {
            of,
            bandwidth,
            left,
            width,
            starts_inside: left > 0.0,
            ends_inside: left + width < 1.0,
        }
    }

    fn flat_plan(names: &[&str]) -> BandPlan {
        let mut plan = plan();
        plan.allocations = names
            .iter()
            .map(|name| allocation(name, name, 0.0, 1.0))
            .collect();
        plan
    }

    fn pieces(plan: &BandPlan, flat: &[BandSpan]) -> Vec<(String, f64, f64)> {
        flat.iter()
            .map(|piece| {
                (
                    plan.allocations[piece.of].name.clone(),
                    (piece.left * 1000.0).round() / 1000.0,
                    (piece.width * 1000.0).round() / 1000.0,
                )
            })
            .collect()
    }

    #[test]
    fn the_narrowest_band_wins_one_row() {
        let plan = flat_plan(&["ADS-B", "aero", "ISM"]);
        let flat = flatten_lanes(
            &plan,
            &[
                vec![span(0, 0.0, 0.8, 1e6), span(1, 0.8, 0.2, 5e7)],
                vec![span(2, 0.0, 1.0, 1e8)],
            ],
        );
        assert_eq!(
            pieces(&plan, &flat),
            [
                ("ADS-B".to_owned(), 0.0, 0.8),
                ("aero".to_owned(), 0.8, 0.2)
            ]
        );
    }

    #[test]
    fn a_wider_band_shows_around_a_narrow_one() {
        let plan = flat_plan(&["narrow", "wide"]);
        let flat = flatten_lanes(
            &plan,
            &[vec![span(0, 0.4, 0.2, 1e5)], vec![span(1, 0.0, 1.0, 1e8)]],
        );
        assert_eq!(
            pieces(&plan, &flat),
            [
                ("wide".to_owned(), 0.0, 0.4),
                ("narrow".to_owned(), 0.4, 0.2),
                ("wide".to_owned(), 0.6, 0.4)
            ]
        );
        assert!(flat[2].starts_inside);
    }

    #[test]
    fn neighbouring_blocks_of_one_band_join() {
        let plan = flat_plan(&["FM", "FM"]);
        let flat = flatten_lanes(
            &plan,
            &[vec![span(0, 0.0, 0.5, 1e6)], vec![span(1, 0.5, 0.5, 2e6)]],
        );
        assert_eq!(pieces(&plan, &flat), [("FM".to_owned(), 0.0, 1.0)]);
    }
}
