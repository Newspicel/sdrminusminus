use sdrmm_wire::bandplan::{BandLayerInfo, BandLayerKind, BandProvision, BandRegion, ItuRegion};

use super::{ruler::*, *};
use sdrmm_wire::{
    bandplan::{BandBlock, BandLane},
    channel::ChannelParams,
};

fn allocation(id: &str, start_hz: f64, stop_hz: f64) -> BandAllocation {
    BandAllocation {
        id: id.to_owned(),
        layer: "world".to_owned(),
        start_hz,
        stop_hz,
        service: BandService::Other,
        name: String::new(),
        official_name: String::new(),
        primary: false,
        reference: None,
        aliases: Vec::new(),
        suggested: None,
        channel_step_hz: None,
        notes: None,
        provisions: Vec::new(),
    }
}

fn suggest(kind: &str) -> Option<ChannelParams> {
    ChannelParams::default_for(kind)
}

struct Built {
    plan: BandPlan,
    allocation_lane: BandLane,
    marine: BandBlock,
}

fn built() -> Built {
    let mut pool: Vec<BandAllocation> = Vec::new();
    let mut block = |start: f64,
                     stop: f64,
                     name: &str,
                     service: BandService,
                     aliases: &[&str],
                     suggested: Option<ChannelParams>| {
        let mut entry = allocation(&format!("{name}:{start}"), start, stop);
        entry.name = name.to_owned();
        entry.service = service;
        entry.aliases = aliases.iter().map(|alias| (*alias).to_owned()).collect();
        entry.suggested = suggested;
        pool.push(entry);
        BandBlock {
            start_hz: start,
            stop_hz: stop,
            of: u32::try_from(pool.len() - 1).expect("small pool"),
            covered: Vec::new(),
        }
    };
    let marine = block(
        156_000_000.0,
        161_962_500.0,
        "Marine VHF",
        BandService::Maritime,
        &["marine vhf", "channel 16"],
        None,
    );
    let ais = block(
        161_962_500.0,
        162_037_500.0,
        "AIS",
        BandService::Maritime,
        &["ais", "ship tracking"],
        None,
    );
    let two_m = block(
        144_000_000.0,
        146_000_000.0,
        "2 m amateur",
        BandService::Amateur,
        &["2 m", "70 cm"],
        suggest("nfm"),
    );
    let aprs = block(
        144_794_000.0,
        144_990_000.0,
        "2 m: APRS",
        BandService::Amateur,
        &["aprs"],
        suggest("aprs"),
    );
    let simplex = block(
        145_206_000.0,
        145_594_000.0,
        "2 m: FM simplex",
        BandService::Amateur,
        &["simplex"],
        None,
    );
    let allocation_lane = BandLane {
        id: "allocation".to_owned(),
        name: "Allocation".to_owned(),
        overlay: false,
        blocks: vec![two_m, marine.clone(), ais],
    };
    let amateur = BandLane {
        id: "iaru-r1".to_owned(),
        name: "Amateur band plan: IARU R1".to_owned(),
        overlay: true,
        blocks: vec![aprs, simplex],
    };
    let plan = BandPlan {
        region: BandRegion {
            id: "de".to_owned(),
            name: "Germany".to_owned(),
            country: None,
            itu_region: ItuRegion::R1,
            layers: vec!["world".to_owned()],
            overlays: Vec::new(),
        },
        layers: vec![BandLayerInfo {
            id: "world".to_owned(),
            name: "ITU world table".to_owned(),
            authority: "ITU".to_owned(),
            source: "RR Article 5".to_owned(),
            kind: BandLayerKind::World,
            rank: 0,
            generator: String::new(),
        }],
        allocations: pool,
        lanes: vec![allocation_lane.clone(), amateur],
        provisions: vec![BandProvision {
            layer: "de".to_owned(),
            id: "5".to_owned(),
            text: "ISM-Anwendungen können Frequenzbereiche mitbenutzen.".to_owned(),
        }],
    };
    Built {
        plan,
        allocation_lane,
        marine,
    }
}

fn names(matches: &[BandMatch]) -> Vec<String> {
    matches
        .iter()
        .map(|found| found.allocation.name.clone())
        .collect()
}

fn suggested_kind(found: &[BandIdentity]) -> Option<String> {
    suggested_at(found).map(|params| params.type_id().to_owned())
}

#[test]
fn groups_what_a_block_covers_by_authority_and_names_each_once() {
    let covered = |id: &str, layer: &str, name: &str| {
        let mut entry = allocation(id, 0.0, 1.0);
        entry.layer = layer.to_owned();
        entry.name = name.to_owned();
        entry
    };
    let authority = |layer: &str| {
        if layer == "cept" {
            String::from("CEPT / ECO")
        } else {
            String::from("ITU")
        }
    };
    let groups = covered_by_layer(
        &[
            covered("a", "cept", "1800 MHz mobile broadband"),
            covered("b", "cept", "1800 MHz mobile broadband"),
            covered("c", "world", "FIXED"),
            covered("d", "cept", "FIXED"),
            covered("e", "itu-r1", "1800 MHz mobile broadband"),
        ],
        authority,
    );
    assert_eq!(
        groups,
        [
            CoveredGroup {
                label: "CEPT / ECO".to_owned(),
                names: vec!["1800 MHz mobile broadband".to_owned(), "FIXED".to_owned()],
            },
            CoveredGroup {
                label: "ITU".to_owned(),
                names: vec!["FIXED".to_owned(), "1800 MHz mobile broadband".to_owned()],
            },
        ]
    );
    assert!(covered_by_layer(&[], authority).is_empty());
}

#[test]
fn reads_the_text_a_layer_cites() {
    let Built { plan, .. } = built();
    assert_eq!(
        provision_text(&plan, "de", "5"),
        Some("ISM-Anwendungen können Frequenzbereiche mitbenutzen.")
    );
    assert_eq!(provision_text(&plan, "de", "D338"), None);
    assert_eq!(provision_text(&plan, "world", "5"), None);
}

#[test]
fn clips_a_block_that_runs_off_both_edges() {
    let Built {
        plan,
        allocation_lane,
        ..
    } = built();
    let spans = spans_in(&plan, &allocation_lane, 144_500_000.0, 1_000_000.0);
    assert_eq!(spans[0].left, 0.0);
    assert_eq!(spans[0].width, 1.0);
    assert!(!spans[0].starts_inside);
    assert!(!spans[0].ends_inside);
}

#[test]
fn places_a_band_that_sits_wholly_inside_the_window() {
    let Built {
        plan,
        allocation_lane,
        ..
    } = built();
    let spans = spans_in(&plan, &allocation_lane, 161_900_000.0, 200_000.0);
    let ais = spans
        .iter()
        .find(|span| span.allocation.name == "AIS")
        .expect("AIS");
    assert!((ais.left - 0.3125).abs() < 1e-10);
    assert!((ais.width - 0.375).abs() < 1e-10);
    assert!(ais.starts_inside && ais.ends_inside);
}

#[test]
fn drops_what_the_window_does_not_reach_and_refuses_an_empty_window() {
    let Built {
        plan,
        allocation_lane,
        ..
    } = built();
    assert!(spans_in(&plan, &allocation_lane, 100_000_000.0, 1_000_000.0).is_empty());
    assert!(spans_in(&plan, &allocation_lane, 143_000_000.0, 1_000_000.0).is_empty());
    assert!(spans_in(&plan, &allocation_lane, 144_000_000.0, 0.0).is_empty());
    assert!(spans_in(&plan, &allocation_lane, 144_000_000.0, -1.0).is_empty());
}

#[test]
fn identifies_once_per_lane_in_lane_order() {
    let Built { plan, .. } = built();
    let found = identify(&plan, 145_500_000.0);
    let lanes: Vec<&str> = found.iter().map(|entry| entry.lane_id.as_str()).collect();
    assert_eq!(lanes, ["allocation", "iaru-r1"]);
    assert_eq!(found[0].allocation.name, "2 m amateur");
    assert_eq!(found[1].allocation.name, "2 m: FM simplex");
    assert_eq!(identify(&plan, 156_800_000.0).len(), 1);
    assert!(identify(&plan, 1.0).is_empty());
}

#[test]
fn treats_a_block_as_half_open() {
    let Built { plan, .. } = built();
    assert_eq!(identify(&plan, 161_962_500.0)[0].allocation.name, "AIS");
    assert_eq!(
        identify(&plan, 161_962_499.0)[0].allocation.name,
        "Marine VHF"
    );
}

#[test]
fn suggests_the_most_specific_lanes_mode() {
    let Built { plan, .. } = built();
    assert_eq!(
        suggested_kind(&identify(&plan, 144_800_000.0)).as_deref(),
        Some("aprs")
    );
    assert_eq!(
        suggested_kind(&identify(&plan, 145_500_000.0)).as_deref(),
        Some("nfm")
    );
    assert_eq!(suggested_kind(&identify(&plan, 1.0)), None);
    assert_eq!(suggested_kind(&identify(&plan, 156_800_000.0)), None);
}

#[test]
fn searches_by_words_and_frequencies() {
    let Built { plan, marine, .. } = built();
    assert_eq!(
        names(&search_plan(&plan, "show me marine VHF", 40))[0],
        "Marine VHF"
    );
    assert!(names(&search_plan(&plan, "70 cm ham", 40)).contains(&"2 m amateur".to_owned()));
    let hits = names(&search_plan(&plan, "145.5", 40));
    assert_eq!(hits[..2], ["2 m: FM simplex", "2 m amateur"]);

    let mut split = plan.clone();
    split.lanes = vec![BandLane {
        id: "allocation".to_owned(),
        name: "Allocation".to_owned(),
        overlay: false,
        blocks: vec![
            BandBlock {
                stop_hz: 158_000_000.0,
                ..marine.clone()
            },
            BandBlock {
                start_hz: 158_000_000.0,
                ..marine
            },
        ],
    }];
    assert_eq!(search_plan(&split, "marine", 40).len(), 1);

    assert!(search_plan(&plan, "", 40).is_empty());
    assert!(search_plan(&plan, "a", 40).is_empty());
    assert!(search_plan(&plan, "zzzz", 40).is_empty());
    assert!(search_plan(&plan, "m", 1).is_empty());
    assert_eq!(search_plan(&plan, "amateur", 1).len(), 1);
}

#[test]
fn parses_a_frequency_in_any_unit() {
    assert_eq!(parse_frequency("145.5"), Some(145_500_000.0));
    assert_eq!(parse_frequency("1090"), Some(1_090_000_000.0));
    assert_eq!(parse_frequency("433 MHz"), Some(433_000_000.0));
    assert_eq!(parse_frequency("77.5khz"), Some(77_500.0));
    assert!(parse_frequency("1.09 GHz").is_some_and(|hz| (hz - 1.09e9).abs() < 1e-3));
    assert_eq!(parse_frequency("9000 hz"), Some(9_000.0));
    assert_eq!(parse_frequency("145,500"), Some(145_500_000.0));
    assert_eq!(parse_frequency("20 m"), None);
    assert_eq!(parse_frequency("marine vhf"), None);
    assert_eq!(parse_frequency("145.5 marine"), None);
    assert_eq!(parse_frequency(""), None);
}

#[test]
fn spells_an_initialism_as_one() {
    assert_eq!(service_label(BandService::Ism), "ISM");
    assert_eq!(service_label(BandService::Aeronautical), "Aeronautical");
}

#[test]
fn tunes_the_middle_or_the_channel_grid() {
    assert_eq!(
        band_tune_hz(&allocation("fm", 87_500_000.0, 108_000_000.0)),
        97_750_000.0
    );
    let mut marine = allocation("marine", 156_000_000.0, 156_062_500.0);
    marine.channel_step_hz = Some(25_000.0);
    assert_eq!(band_tune_hz(&marine), 156_025_000.0);
    let mut band = allocation("b", 100.0, 200.0);
    band.channel_step_hz = Some(0.0);
    assert_eq!(band_tune_hz(&band), 150.0);
}

fn span(left: f64, width: f64, name: &str, hz: f64) -> BandSpan {
    let mut entry = allocation(name, 0.0, hz);
    entry.name = name.to_owned();
    BandSpan {
        block: BandBlock {
            start_hz: 0.0,
            stop_hz: hz,
            of: 0,
            covered: Vec::new(),
        },
        allocation: entry,
        left,
        width,
        starts_inside: left > 0.0,
        ends_inside: left + width < 1.0,
    }
}

fn pieces(spans: &[BandSpan]) -> Vec<(String, f64, f64)> {
    let round = |value: f64| (value * 1000.0).round() / 1000.0;
    spans
        .iter()
        .map(|piece| {
            (
                piece.allocation.name.clone(),
                round(piece.left),
                round(piece.width),
            )
        })
        .collect()
}

#[test]
fn flattens_lanes_so_the_narrowest_band_wins() {
    let flat = flatten_lanes(&[
        vec![span(0.0, 0.8, "ADS-B", 1e6), span(0.8, 0.2, "aero", 5e7)],
        vec![span(0.0, 1.0, "ISM", 1e8)],
    ]);
    assert_eq!(
        pieces(&flat),
        [
            ("ADS-B".to_owned(), 0.0, 0.8),
            ("aero".to_owned(), 0.8, 0.2)
        ]
    );

    let around = flatten_lanes(&[
        vec![span(0.4, 0.2, "narrow", 1e5)],
        vec![span(0.0, 1.0, "wide", 1e8)],
    ]);
    assert_eq!(
        pieces(&around),
        [
            ("wide".to_owned(), 0.0, 0.4),
            ("narrow".to_owned(), 0.4, 0.2),
            ("wide".to_owned(), 0.6, 0.4)
        ]
    );
    assert!(around[2].starts_inside);

    let joined = flatten_lanes(&[
        vec![span(0.0, 0.5, "FM", 1e6)],
        vec![span(0.5, 0.5, "FM", 2e6)],
    ]);
    assert_eq!(pieces(&joined), [("FM".to_owned(), 0.0, 1.0)]);
}
