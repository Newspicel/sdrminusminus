//! The frequency-allocation database (FEATURES §5): "what is this frequency?".
//!
//! Shaped like [`crate::templates`] and for the same reason — the tables ship with the binary,
//! so a seeded database would need a migration every time an entry is corrected, and a user
//! could delete a row the next release would silently restore.
//!
//! Layering is the whole idea. A region names an ordered stack of [`Layer`]s, least specific
//! first, and [`resolve`] flattens them so that at every frequency the most specific layer that
//! has something to say wins, while what it covers travels with the block for the identify
//! popover. Amateur band plans are *not* in that stack: IARU plans are agreements between
//! operators about a band a regulator has already allocated, so they resolve into a lane of
//! their own and are drawn over the allocation rather than instead of it.
//!
//! Adding a region is one module plus one line in [`LAYERS`] and one in [`REGIONS`]. Nothing in
//! the resolution knows which regulator it is walking.

// Every table row ends in `..Entry::ROW`, including the handful that happen to set all eight
// fields. Uniformity is the point: the tables are read and edited as rows of a table, and a row
// that had to drop the tail because it grew a `notes` line would make adding one a two-line
// edit and a diff nobody can scan. Lint levels inherit down the module tree, so this covers the
// layer modules too.
#![allow(
    clippy::needless_update,
    reason = "the tables are authored as uniform rows; see above"
)]

use std::sync::LazyLock;

use sdrmm_wire::{
    BandAllocation, BandBlock, BandLane, BandLayerInfo, BandLayerKind, BandPlan, BandRegion,
    BandRegionMatch, BandRegionsResponse, BandService, ChannelParams, ItuRegion,
};

mod cept;
mod germany;
mod iaru_r1;
mod uk;
mod us;
mod world;

/// Constructors for the modes a table suggests. `ChannelParams` is not const-constructible, so
/// an entry holds a `fn` and the plan calls it once when the region is first resolved — the same
/// shape `templates.rs` uses for its channels.
mod mode {
    use sdrmm_wire::{
        AdsbParams, AisParams, AmParams, ChannelParams, MorseParams, NavtexParams, NfmParams,
        Sideband, SsbParams, SubghzParams, WfmParams,
    };

    pub(super) fn am() -> ChannelParams {
        ChannelParams::Am(AmParams::default())
    }
    pub(super) fn nfm() -> ChannelParams {
        ChannelParams::Nfm(NfmParams::default())
    }
    pub(super) fn wfm() -> ChannelParams {
        ChannelParams::Wfm(WfmParams::default())
    }
    pub(super) fn adsb() -> ChannelParams {
        ChannelParams::Adsb(AdsbParams::default())
    }
    pub(super) fn ais() -> ChannelParams {
        ChannelParams::Ais(AisParams::default())
    }
    pub(super) fn navtex() -> ChannelParams {
        ChannelParams::Navtex(NavtexParams::default())
    }
    pub(super) fn morse() -> ChannelParams {
        ChannelParams::Morse(MorseParams::default())
    }
    pub(super) fn subghz() -> ChannelParams {
        ChannelParams::Subghz(SubghzParams::default())
    }
    pub(super) fn aprs() -> ChannelParams {
        ChannelParams::Aprs(sdrmm_wire::AprsParams::default())
    }
    /// HF convention: lower sideband below 10 MHz, upper above it. Worth encoding, because
    /// getting it wrong is the single most common way a first HF listen sounds like noise.
    pub(super) fn lsb() -> ChannelParams {
        ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Lsb,
            ..SsbParams::default()
        })
    }
    pub(super) fn usb() -> ChannelParams {
        ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            ..SsbParams::default()
        })
    }
}

/// One row of a static layer table. Ranges are half-open `[start_hz, stop_hz)`, so a band that
/// ends where the next begins produces no zero-width sliver.
pub(crate) struct Entry {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: BandService,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// The mode "tune here" applies. A constructor rather than a value because `ChannelParams`
    /// is not const-constructible — the same reason `templates.rs` holds its channels as `fn`s.
    pub suggested: Option<fn() -> ChannelParams>,
    pub channel_step_hz: Option<f64>,
    pub notes: Option<&'static str>,
}

impl Entry {
    /// Base for functional update, so a table row states only what it has to say:
    /// `Entry { start_hz: .., stop_hz: .., service: .., name: "..", ..Entry::ROW }`.
    /// The four fields left blank here are required of every row and checked in tests.
    pub const ROW: Self = Self {
        start_hz: 0.0,
        stop_hz: 0.0,
        service: BandService::Other,
        name: "",
        aliases: &[],
        suggested: None,
        channel_step_hz: None,
        notes: None,
    };
}

/// A published table. `entries` must be sorted by `start_hz` and non-overlapping; both are
/// asserted in tests rather than defended at runtime, because a table is authored, not parsed.
pub(crate) struct Layer {
    pub id: &'static str,
    pub name: &'static str,
    pub authority: &'static str,
    pub source: &'static str,
    pub kind: BandLayerKind,
    pub entries: &'static [Entry],
}

/// Every layer this build ships. Adding an importer is adding a module and a line here.
static LAYERS: &[&Layer] = &[
    &world::GLOBAL,
    &world::ITU_R1,
    &world::ITU_R2,
    &world::ITU_R3,
    &cept::CEPT,
    &germany::BNETZA,
    &uk::OFCOM,
    &us::FCC,
    &iaru_r1::IARU_R1,
];

/// A coarse footprint, in degrees. Used only to guess a default region from a coordinate; the
/// operator's own choice always wins over it.
struct Bbox {
    lat: (f64, f64),
    lon: (f64, f64),
}

impl Bbox {
    fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.lat.0 && lat <= self.lat.1 && lon >= self.lon.0 && lon <= self.lon.1
    }
}

struct RegionDef {
    id: &'static str,
    name: &'static str,
    country: Option<&'static str>,
    itu: ItuRegion,
    /// Layer ids, least specific first.
    layers: &'static [&'static str],
    overlays: &'static [&'static str],
    /// Where this region applies, coarsely. Empty for the bare ITU regions, which are decided
    /// by [`itu_region_of`] instead.
    footprint: &'static [Bbox],
}

/// The selectable regions, most specific first — [`locate`] returns the first footprint hit, so
/// Germany must be tried before CEPT.
static REGIONS: &[RegionDef] = &[
    RegionDef {
        id: "de",
        name: "Germany — BNetzA",
        country: Some("DE"),
        itu: ItuRegion::R1,
        layers: &["world", "itu-r1", "cept", "de"],
        overlays: &["iaru-r1"],
        footprint: &[Bbox {
            lat: (47.2, 55.1),
            lon: (5.8, 15.1),
        }],
    },
    RegionDef {
        id: "gb",
        name: "United Kingdom — Ofcom",
        country: Some("GB"),
        itu: ItuRegion::R1,
        layers: &["world", "itu-r1", "cept", "gb"],
        overlays: &["iaru-r1"],
        footprint: &[Bbox {
            lat: (49.8, 61.0),
            lon: (-8.7, 1.9),
        }],
    },
    RegionDef {
        id: "us",
        name: "United States — FCC",
        country: Some("US"),
        itu: ItuRegion::R2,
        layers: &["world", "itu-r2", "us"],
        overlays: &[],
        footprint: &[
            Bbox {
                lat: (24.5, 49.4),
                lon: (-125.0, -66.9),
            },
            // Alaska and Hawaii, which the contiguous box misses entirely.
            Bbox {
                lat: (51.0, 71.5),
                lon: (-168.0, -130.0),
            },
            Bbox {
                lat: (18.9, 22.3),
                lon: (-160.3, -154.8),
            },
        ],
    },
    RegionDef {
        id: "cept",
        name: "Europe — CEPT",
        country: None,
        itu: ItuRegion::R1,
        layers: &["world", "itu-r1", "cept"],
        overlays: &["iaru-r1"],
        footprint: &[Bbox {
            lat: (34.0, 72.0),
            lon: (-31.5, 40.0),
        }],
    },
    RegionDef {
        id: "itu-r1",
        name: "ITU Region 1",
        country: None,
        itu: ItuRegion::R1,
        layers: &["world", "itu-r1"],
        overlays: &["iaru-r1"],
        footprint: &[],
    },
    RegionDef {
        id: "itu-r2",
        name: "ITU Region 2",
        country: None,
        itu: ItuRegion::R2,
        layers: &["world", "itu-r2"],
        overlays: &[],
        footprint: &[],
    },
    RegionDef {
        id: "itu-r3",
        name: "ITU Region 3",
        country: None,
        itu: ItuRegion::R3,
        layers: &["world", "itu-r3"],
        overlays: &[],
        footprint: &[],
    },
];

/// What a client with no stored preference gets. Region 1 rather than a country: it is the
/// widest answer that is still useful, and the client offers to narrow it from the browser's
/// location.
pub(crate) const DEFAULT_REGION: &str = "itu-r1";

/// The lane the regulatory stack resolves into. Overlay lanes are named by their layer.
const ALLOCATION_LANE: &str = "allocation";

/// Every region, resolved once. The tables are static, so this is computed on first request and
/// then handed out by clone — a plan is a few hundred blocks, and rebuilding it per request
/// would sweep every layer again for an answer that cannot have changed.
static PLANS: LazyLock<Vec<BandPlan>> = LazyLock::new(|| REGIONS.iter().map(build).collect());

pub(crate) fn regions() -> BandRegionsResponse {
    BandRegionsResponse {
        regions: PLANS.iter().map(|plan| plan.region.clone()).collect(),
        default_region: DEFAULT_REGION.to_string(),
    }
}

/// The resolved plan for `region`, or `None` if no such region ships.
pub(crate) fn plan(region: &str) -> Option<BandPlan> {
    PLANS.iter().find(|plan| plan.region.id == region).cloned()
}

/// Guess a region from a coordinate. Coarse by construction — the footprints are bounding
/// boxes, and the ITU fallback approximates lines A/B/C — so the answer is a starting point the
/// operator confirms, never a silent setting.
pub(crate) fn locate(lat: f64, lon: f64) -> BandRegionMatch {
    if let Some(def) = REGIONS
        .iter()
        .find(|def| def.footprint.iter().any(|area| area.contains(lat, lon)))
    {
        return BandRegionMatch {
            region: def.id.to_string(),
            itu_region: def.itu,
            approximate: false,
        };
    }
    let itu = itu_region_of(lat, lon);
    BandRegionMatch {
        region: match itu {
            ItuRegion::R1 => "itu-r1",
            ItuRegion::R2 => "itu-r2",
            ItuRegion::R3 => "itu-r3",
        }
        .to_string(),
        itu_region: itu,
        approximate: true,
    }
}

/// ITU regions from a coordinate, approximating Radio Regulations lines A/B/C with boxes.
///
/// Known to be wrong at the edges the boxes cannot express: Mongolia and northern China sit
/// inside the Region 3 box but Mongolia is Region 1 (RR 5.2 names it explicitly), and the
/// Atlantic leg of line B is a set of great-circle arcs, not the meridian used here. Every
/// answer from this function is reported as `approximate`.
fn itu_region_of(lat: f64, lon: f64) -> ItuRegion {
    // Greenland is Region 1 despite sitting deep inside the Americas box.
    const GREENLAND: Bbox = Bbox {
        lat: (58.0, 84.0),
        lon: (-74.0, -11.0),
    };
    // Region 3 east of line A: south of the former USSR, whose whole territory is Region 1.
    const ASIA_PACIFIC: Bbox = Bbox {
        lat: (-55.0, 45.0),
        lon: (60.0, 180.0),
    };
    // Iran and the Persian Gulf's eastern shore, which line A puts in Region 3.
    const GULF_EAST: Bbox = Bbox {
        lat: (12.0, 40.0),
        lon: (44.0, 60.0),
    };

    if GREENLAND.contains(lat, lon) {
        ItuRegion::R1
    } else if (-170.0..=-20.0).contains(&lon) {
        ItuRegion::R2
    } else if ASIA_PACIFIC.contains(lat, lon) || GULF_EAST.contains(lat, lon) {
        ItuRegion::R3
    } else {
        ItuRegion::R1
    }
}

fn build(def: &RegionDef) -> BandPlan {
    let mut layers = Vec::new();
    let mut rank = 0u8;
    let info_of = |id: &str, rank: u8| -> Option<BandLayerInfo> {
        layer(id).map(|layer| BandLayerInfo {
            id: layer.id.to_string(),
            name: layer.name.to_string(),
            authority: layer.authority.to_string(),
            source: layer.source.to_string(),
            kind: layer.kind,
            rank,
        })
    };

    let mut stack = Vec::new();
    for id in def.layers {
        if let Some(found) = layer(id) {
            if let Some(info) = info_of(id, rank) {
                layers.push(info);
            }
            stack.push(found);
            rank += 1;
        }
    }

    let mut lanes = vec![BandLane {
        id: ALLOCATION_LANE.to_string(),
        name: "Allocation".to_string(),
        overlay: false,
        blocks: resolve(&stack),
    }];

    for id in def.overlays {
        let Some(found) = layer(id) else { continue };
        if let Some(info) = info_of(id, rank) {
            layers.push(info);
        }
        rank += 1;
        lanes.push(BandLane {
            id: found.id.to_string(),
            name: found.name.to_string(),
            overlay: true,
            blocks: resolve(&[found]),
        });
    }

    BandPlan {
        region: BandRegion {
            id: def.id.to_string(),
            name: def.name.to_string(),
            country: def.country.map(str::to_string),
            itu_region: def.itu,
            layers: def.layers.iter().map(|id| (*id).to_string()).collect(),
            overlays: def.overlays.iter().map(|id| (*id).to_string()).collect(),
        },
        layers,
        lanes,
    }
}

fn layer(id: &str) -> Option<&'static Layer> {
    LAYERS.iter().copied().find(|layer| layer.id == id)
}

/// Flatten a layer stack into non-overlapping blocks, most-specific-wins.
///
/// A sweep rather than a per-layer overwrite: the edges of every entry in every layer become
/// candidate boundaries, and each resulting interval is decided by asking the layers, from most
/// to least specific, which of them covers its midpoint. That produces the covered stack for
/// free, and it is the only formulation that stays correct when a national entry straddles two
/// ITU entries — an overwrite would have to split the loser, which is the same sweep written
/// less directly.
fn resolve(stack: &[&'static Layer]) -> Vec<BandBlock> {
    let mut edges: Vec<f64> = stack
        .iter()
        .flat_map(|layer| layer.entries.iter())
        .flat_map(|entry| [entry.start_hz, entry.stop_hz])
        .collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup();

    let mut blocks: Vec<BandBlock> = Vec::new();
    for pair in edges.windows(2) {
        let (start, stop) = (pair[0], pair[1]);
        // `dedup` already removed equal edges; this catches the pair a NaN in a table would
        // leave behind, which would otherwise become a block spanning nothing at all.
        if stop <= start {
            continue;
        }
        let mid = start + (stop - start) / 2.0;
        // Most specific first, which is the order the popover reads in.
        let mut hits: Vec<BandAllocation> = stack
            .iter()
            .rev()
            .filter_map(|layer| entry_at(layer, mid).map(|entry| allocation(layer, entry)))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let winner = hits.remove(0);

        // Two adjacent intervals that agree on every layer are one block: a boundary that only
        // exists because some *other* part of the spectrum has an edge there is not a boundary.
        if let Some(last) = blocks.last_mut()
            && last.stop_hz == start
            && last.allocation.id == winner.id
            && last.covered.len() == hits.len()
            && last.covered.iter().zip(&hits).all(|(a, b)| a.id == b.id)
        {
            last.stop_hz = stop;
            continue;
        }
        blocks.push(BandBlock {
            start_hz: start,
            stop_hz: stop,
            allocation: winner,
            covered: hits,
        });
    }
    blocks
}

/// The entry covering `hz`, by binary search — layers are sorted and non-overlapping.
fn entry_at(layer: &'static Layer, hz: f64) -> Option<&'static Entry> {
    let at = layer.entries.partition_point(|entry| entry.start_hz <= hz);
    layer
        .entries
        .get(at.checked_sub(1)?)
        .filter(|entry| hz < entry.stop_hz)
}

fn allocation(layer: &Layer, entry: &Entry) -> BandAllocation {
    BandAllocation {
        id: format!("{}:{:.0}", layer.id, entry.start_hz),
        layer: layer.id.to_string(),
        start_hz: entry.start_hz,
        stop_hz: entry.stop_hz,
        service: entry.service,
        name: entry.name.to_string(),
        aliases: entry.aliases.iter().map(|a| (*a).to_string()).collect(),
        suggested: entry.suggested.map(|make| make()),
        channel_step_hz: entry.channel_step_hz,
        notes: entry.notes.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binary search in `entry_at` and the sweep's block ordering both assume it, and an
    /// unsorted table would silently answer the wrong band rather than fail.
    #[test]
    fn every_layer_is_sorted_and_disjoint() {
        for layer in LAYERS {
            for entry in layer.entries {
                assert!(!entry.name.is_empty(), "{}: an unnamed row", layer.id);
            }
            for pair in layer.entries.windows(2) {
                assert!(
                    pair[0].stop_hz > pair[0].start_hz,
                    "{}: {} is empty or inverted",
                    layer.id,
                    pair[0].name
                );
                assert!(
                    pair[1].start_hz >= pair[0].stop_hz,
                    "{}: {} overlaps {}",
                    layer.id,
                    pair[1].name,
                    pair[0].name
                );
            }
            if let Some(last) = layer.entries.last() {
                assert!(last.stop_hz > last.start_hz, "{}: {}", layer.id, last.name);
            }
        }
    }

    /// Most-specific-wins only refines if the specific layers are *more* specific. A national
    /// table that covers a whole ITU band with one coarse row would erase every detail the
    /// world layer carries there, and the loss would show up as a blank ruler nobody could
    /// explain. Twenty times wider is the line between "refines" and "erases".
    #[test]
    fn a_higher_layer_never_swallows_a_much_narrower_lower_one() {
        for def in REGIONS {
            let stack: Vec<&Layer> = def.layers.iter().filter_map(|id| layer(id)).collect();
            for (above, upper) in stack.iter().enumerate().skip(1) {
                for lower in &stack[..above] {
                    for wide in upper.entries {
                        for narrow in lower.entries {
                            let covered =
                                narrow.start_hz >= wide.start_hz && narrow.stop_hz <= wide.stop_hz;
                            let ratio =
                                (wide.stop_hz - wide.start_hz) / (narrow.stop_hz - narrow.start_hz);
                            assert!(
                                !(covered && ratio > 20.0),
                                "{}'s {} ({:.0} Hz) swallows {}'s {} ({:.0} Hz)",
                                upper.id,
                                wide.name,
                                wide.stop_hz - wide.start_hz,
                                lower.id,
                                narrow.name,
                                narrow.stop_hz - narrow.start_hz,
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_region_names_layers_that_exist() {
        for def in REGIONS {
            for id in def.layers.iter().chain(def.overlays) {
                assert!(layer(id).is_some(), "{}: no layer {id}", def.id);
            }
        }
        assert!(plan(DEFAULT_REGION).is_some());
    }

    /// A suggested mode is applied to a channel verbatim, so a type this build does not ship
    /// would be a 400 the operator sees only on click.
    #[test]
    fn every_suggested_mode_is_a_channel_type_this_build_ships() {
        let types: Vec<String> = sdrmm_engine::Engine::new(None)
            .channel_types()
            .into_iter()
            .map(|descriptor| descriptor.type_id)
            .collect();
        for layer in LAYERS {
            for entry in layer.entries {
                if let Some(make) = entry.suggested {
                    let id = make().type_id();
                    assert!(
                        types.iter().any(|known| known == id),
                        "{}: {} suggests unknown type {id}",
                        layer.id,
                        entry.name
                    );
                }
            }
        }
    }

    #[test]
    fn resolved_lanes_are_sorted_and_disjoint() {
        for plan in PLANS.iter() {
            for lane in &plan.lanes {
                for pair in lane.blocks.windows(2) {
                    assert!(
                        pair[1].start_hz >= pair[0].stop_hz,
                        "{}/{}: blocks overlap at {}",
                        plan.region.id,
                        lane.id,
                        pair[0].stop_hz
                    );
                }
                for block in &lane.blocks {
                    assert!(block.stop_hz > block.start_hz);
                    assert!(block.allocation.start_hz <= block.start_hz);
                    assert!(block.allocation.stop_hz >= block.stop_hz);
                }
            }
        }
    }

    /// Every block's layer must be one the plan describes, or the popover has no authority to
    /// name for it.
    #[test]
    fn every_block_names_a_layer_the_plan_carries() {
        for plan in PLANS.iter() {
            for lane in &plan.lanes {
                for block in &lane.blocks {
                    for allocation in std::iter::once(&block.allocation).chain(&block.covered) {
                        assert!(
                            plan.layers.iter().any(|info| info.id == allocation.layer),
                            "{}: block cites unknown layer {}",
                            plan.region.id,
                            allocation.layer
                        );
                    }
                }
            }
        }
    }

    /// The property the whole feature rests on: inside a national sub-band the national entry
    /// wins, and the ITU entry it refines is still readable underneath it.
    #[test]
    fn the_national_layer_wins_and_keeps_what_it_covers() {
        let plan = plan("de").expect("germany ships");
        let lane = &plan.lanes[0];
        // PMR446 is a CEPT sub-band inside the ITU land-mobile allocation.
        let block = lane
            .blocks
            .iter()
            .find(|block| block.start_hz <= 446_100_000.0 && block.stop_hz > 446_100_000.0)
            .expect("446.1 MHz is allocated");
        assert_eq!(block.allocation.layer, "cept");
        assert!(block.allocation.name.contains("PMR446"));
        assert!(
            block.covered.iter().any(|under| under.layer == "world"),
            "the ITU allocation under PMR446 is lost"
        );
    }

    /// The same frequency answers differently per region — the reason regions exist at all.
    #[test]
    fn regions_disagree_where_the_tables_do() {
        let of = |region: &str, hz: f64| -> String {
            let plan = plan(region).expect("region ships");
            plan.lanes[0]
                .blocks
                .iter()
                .find(|block| block.start_hz <= hz && block.stop_hz > hz)
                .map(|block| block.allocation.name.clone())
                .unwrap_or_default()
        };
        // 902–928 MHz is the Region 2 ISM band and a Region 1 GSM uplink.
        assert!(of("us", 915_000_000.0).contains("ISM"));
        assert!(!of("de", 915_000_000.0).contains("ISM"));
    }

    /// The amateur plan is an overlay, not an override: the allocation lane must still say
    /// "amateur" underneath it.
    #[test]
    fn the_amateur_overlay_is_its_own_lane() {
        let plan = plan("de").expect("germany ships");
        let overlay = plan
            .lanes
            .iter()
            .find(|lane| lane.overlay)
            .expect("germany carries the IARU R1 overlay");
        assert_eq!(overlay.id, "iaru-r1");
        assert!(
            overlay
                .blocks
                .iter()
                .any(|block| block.start_hz >= 144_000_000.0 && block.stop_hz <= 146_000_000.0)
        );
        let allocation = &plan.lanes[0];
        let two_metres = allocation
            .blocks
            .iter()
            .find(|block| block.start_hz <= 145_000_000.0 && block.stop_hz > 145_000_000.0)
            .expect("2 m is allocated");
        assert_eq!(two_metres.allocation.service, BandService::Amateur);
    }

    #[test]
    fn locate_prefers_a_national_footprint_over_the_itu_fallback() {
        let berlin = locate(52.52, 13.40);
        assert_eq!(berlin.region, "de");
        assert!(!berlin.approximate);

        let london = locate(51.5, -0.13);
        assert_eq!(london.region, "gb");

        let denver = locate(39.74, -104.99);
        assert_eq!(denver.region, "us");

        // Inside CEPT but outside every national table this build ships.
        let warsaw = locate(52.23, 21.01);
        assert_eq!(warsaw.region, "cept");
        assert!(!warsaw.approximate);
    }

    #[test]
    fn locate_falls_back_to_the_itu_region_and_says_it_is_coarse() {
        let tokyo = locate(35.68, 139.69);
        assert_eq!(tokyo.region, "itu-r3");
        assert!(tokyo.approximate);

        let nairobi = locate(-1.29, 36.82);
        assert_eq!(nairobi.region, "itu-r1");
        assert!(nairobi.approximate);

        // Brazil is Region 2 without being the United States.
        let saopaulo = locate(-23.55, -46.63);
        assert_eq!(saopaulo.region, "itu-r2");

        // Greenland sits inside the Americas box and is still Region 1.
        assert_eq!(locate(72.0, -40.0).itu_region, ItuRegion::R1);
    }

    #[test]
    fn unknown_regions_are_none() {
        assert!(plan("nope").is_none());
        assert!(plan("").is_none());
    }

    /// A boundary shared by two layers must not leave a zero-width block behind, and a gap in
    /// every layer must stay a gap rather than becoming a block with no allocation.
    #[test]
    fn the_sweep_emits_neither_slivers_nor_empty_blocks() {
        for plan in PLANS.iter() {
            for lane in &plan.lanes {
                for block in &lane.blocks {
                    assert!(
                        block.stop_hz - block.start_hz > 0.0,
                        "{}: zero-width block at {}",
                        plan.region.id,
                        block.start_hz
                    );
                }
            }
        }
    }
}
