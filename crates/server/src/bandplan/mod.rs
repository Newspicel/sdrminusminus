//! The frequency-allocation database (FEATURES §5): "what is this frequency?".
//!
//! **The tables are data, not code.** Each layer is a JSON document in `data/bandplan/`, and most
//! of them are *generated* — `cargo xtask bandplan` fetches the regulator's own publication and
//! parses it, so the answer this gives is the one the source document gives, with the row it came
//! from recorded on it. Hand-typing a regulator's table is how a band plan becomes quietly wrong
//! and stays that way; the importers are re-runnable and their output is reviewed as a diff.
//!
//! The documents are `include_str!`d, so they ship inside the binary exactly as the old static
//! tables did: no runtime file I/O, no seeded database that a migration has to correct and a user
//! can delete. What changed is who writes them.
//!
//! Layering is the whole idea. A region names an ordered stack of [`Layer`]s, least specific
//! first, and [`resolve`] flattens them so that at every frequency the most specific layer that
//! has something to say wins, while everything it covers travels with the block for the identify
//! popover. Amateur band plans are *not* in that stack: IARU plans are agreements between
//! operators about a band a regulator has already allocated, so they resolve into a lane of
//! their own and are drawn over the allocation rather than instead of it.
//!
//! What a regulator publishes is an allocation, not operator knowledge — BNetzA says
//! "MOBILER SEEFUNKDIENST", not "Marine VHF, and channel 16 is the distress channel". The gap is
//! [`ANNOTATIONS`]: a small curated overlay, the only hand-written data left, which adds the
//! friendly name, the aliases the explorer searches, and the mode "tune here" applies.
//!
//! Adding a region is a JSON document plus one line in [`REGIONS`]. Nothing in the resolution
//! knows which regulator it is walking.

use std::sync::LazyLock;

use sdrmm_wire::{
    BandAllocation, BandBlock, BandLane, BandLayerInfo, BandLayerKind, BandPlan, BandRegion,
    BandRegionMatch, BandRegionsResponse, BandService, ChannelParams, ItuRegion,
};
use serde::Deserialize;

/// One row of a layer table. Ranges are half-open `[start_hz, stop_hz)`, so a band that ends
/// where the next begins produces no zero-width sliver.
///
/// Rows within a layer **may overlap**: a regulator gives one range to several services at once,
/// and flattening that into one row per range would be inventing an answer the source does not
/// give. [`resolve`] handles it; nothing here has to.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Entry {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: BandService,
    /// Exactly what the source calls it.
    pub name: String,
    /// The name an operator would use, where it differs from the official one. Contributed by
    /// [`ANNOTATIONS`], never by an importer.
    #[serde(default)]
    pub friendly: Option<String>,
    /// Primary rather than secondary. Both the ITU and BNetzA tables encode this as
    /// capitalisation of the service name, so an importer reads it off for free.
    #[serde(default = "primary_by_default")]
    pub primary: bool,
    /// Where the row is in its source document — `27001`, `FREQ_00001`. Also the allocation's
    /// stable id, because a range cannot identify a row in a table that repeats ranges.
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The mode "tune here" applies.
    #[serde(default)]
    pub suggested: Option<ChannelParams>,
    #[serde(default)]
    pub channel_step_hz: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
}

const fn primary_by_default() -> bool {
    true
}

/// How a layer document came to exist, carried into [`BandLayerInfo`] so a reader can tell an
/// importer's output from the hand-written remainder.
#[derive(Clone, Debug, Default, Deserialize)]
#[expect(
    dead_code,
    reason = "the URL, timestamp and digest are written for a human reviewing the generated \
              document and its diff; only `generator` is answered over the wire"
)]
pub(crate) struct Provenance {
    /// `curated`, or the importer that wrote it (`bnetza`, `ofcom`, `fcc`).
    pub generator: String,
    /// Where the source document was fetched from, and when.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// SHA-256 of the source document as parsed. Recorded, not enforced: it is how a reviewer
    /// tells "the regulator changed the table" from "the parser changed its mind".
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A published table. `entries` must be sorted by `start_hz`; they may overlap each other.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Layer {
    pub id: String,
    pub name: String,
    pub authority: String,
    pub source: String,
    pub kind: BandLayerKind,
    #[serde(default)]
    pub provenance: Provenance,
    pub entries: Vec<Entry>,
}

/// The curated overlay that turns a regulator's wording into an operator's (FEATURES §5).
///
/// A regulator publishes allocations; nobody publishes "this is Marine VHF, channel 16 is
/// distress, tune it in NFM". This is that, and it is the only hand-written band data left. An
/// annotation attaches to the *pieces* of an entry that fall inside it — entries are split at its
/// edges first — so a note about 25 kHz of a 6 MHz allocation never relabels the whole thing.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Annotation {
    pub start_hz: f64,
    pub stop_hz: f64,
    /// What an operator calls this stretch.
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub suggested: Option<ChannelParams>,
    #[serde(default)]
    pub channel_step_hz: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Restrict the annotation to entries of one service, so "Marine VHF" labels the maritime
    /// allocation over 156–162 MHz and not the land-mobile row that shares it.
    #[serde(default)]
    pub service: Option<BandService>,
}

/// Every layer document, `include_str!`d so they ship inside the binary. Adding an importer is
/// adding its output here and naming it from a region.
static LAYER_DOCS: &[(&str, &str)] = &[
    ("world", include_str!("../../data/bandplan/world.json")),
    ("itu-r1", include_str!("../../data/bandplan/itu-r1.json")),
    ("itu-r2", include_str!("../../data/bandplan/itu-r2.json")),
    ("itu-r3", include_str!("../../data/bandplan/itu-r3.json")),
    ("cept", include_str!("../../data/bandplan/cept.json")),
    ("de", include_str!("../../data/bandplan/de.json")),
    ("gb", include_str!("../../data/bandplan/gb.json")),
    ("us", include_str!("../../data/bandplan/us.json")),
    ("iaru-r1", include_str!("../../data/bandplan/iaru-r1.json")),
];

static ANNOTATIONS_DOC: &str = include_str!("../../data/bandplan/annotations.json");

/// The curated annotations, sorted so the splitter can walk them.
///
/// `expect` here is not I/O: the document is `include_str!`d, so a malformed one cannot appear
/// at runtime — it fails the loader test in CI, before it can reach anybody.
#[expect(clippy::expect_used, reason = "compiled-in constant; see above")]
static ANNOTATIONS: LazyLock<Vec<Annotation>> = LazyLock::new(|| {
    let mut parsed: Vec<Annotation> =
        serde_json::from_str(ANNOTATIONS_DOC).expect("annotations.json is committed and valid");
    parsed.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    parsed
});

/// Every layer, parsed and annotated once.
///
/// `expect` is load-of-a-compiled-in-constant, not I/O: the documents are `include_str!`d, so a
/// malformed one cannot appear at runtime — it fails the test that parses them, in CI, before it
/// can reach anybody (CLAUDE.md's startup exception).
static LAYERS: LazyLock<Vec<Layer>> = LazyLock::new(|| {
    LAYER_DOCS
        .iter()
        .map(|(id, doc)| {
            let mut layer: Layer =
                serde_json::from_str(doc).unwrap_or_else(|e| panic!("{id}.json: {e}"));
            layer.entries = annotate(layer.entries, &ANNOTATIONS);
            layer
                .entries
                .sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
            layer
        })
        .collect()
});

/// Split every entry at the annotation edges inside it, then label the pieces an annotation
/// fully contains.
///
/// Splitting first is what keeps this honest. "Marine VHF" covers 156–161.9625 MHz; the ITU
/// allocation it sits in runs 156–174 MHz. Labelling the whole allocation would put the name on
/// 12 MHz of land mobile, and skipping it because it does not fit would lose the name entirely.
/// Cutting the allocation at the annotation's edges gives each piece its right answer.
fn annotate(entries: Vec<Entry>, annotations: &[Annotation]) -> Vec<Entry> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        // Only the annotations that actually touch this entry, and only the edges strictly
        // inside it: an edge at the boundary splits nothing.
        let mut cuts: Vec<f64> = annotations
            .iter()
            .filter(|a| a.stop_hz > entry.start_hz && a.start_hz < entry.stop_hz)
            .flat_map(|a| [a.start_hz, a.stop_hz])
            .filter(|hz| *hz > entry.start_hz && *hz < entry.stop_hz)
            .collect();
        cuts.push(entry.start_hz);
        cuts.push(entry.stop_hz);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();

        let entry_width = entry.stop_hz - entry.start_hz;
        for pair in cuts.windows(2) {
            let (start, stop) = (pair[0], pair[1]);
            let mut piece = Entry {
                start_hz: start,
                stop_hz: stop,
                ..entry.clone()
            };
            // The narrowest annotation containing the piece wins, so a specific note inside a
            // broad one is not overruled by it.
            let found = annotations
                .iter()
                .filter(|a| a.start_hz <= start && a.stop_hz >= stop)
                .filter(|a| a.service.is_none_or(|service| service == entry.service))
                // …and an annotation never renames a row that is *more* specific than it is.
                // "UHF land mobile" spans 440–470 MHz and would otherwise relabel CEPT's
                // 200 kHz PMR446 row sitting inside it, replacing the precise answer with the
                // vague one — the exact inversion this whole layering exists to prevent.
                .filter(|a| entry_width >= a.stop_hz - a.start_hz)
                .min_by(|a, b| (a.stop_hz - a.start_hz).total_cmp(&(b.stop_hz - b.start_hz)));
            if let Some(annotation) = found {
                piece.friendly = Some(annotation.name.clone());
                piece.aliases = annotation.aliases.clone();
                // The regulator's own raster and notes win where it published them: an
                // annotation fills gaps, it does not correct the source.
                piece.suggested = piece.suggested.or_else(|| annotation.suggested.clone());
                piece.channel_step_hz = piece.channel_step_hz.or(annotation.channel_step_hz);
                piece.notes = piece.notes.or_else(|| annotation.notes.clone());
            }
            out.push(piece);
        }
    }
    out
}

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
            generator: layer.provenance.generator.clone(),
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

    // One table per plan, filled as the lanes resolve, so an allocation split into a dozen
    // blocks by other layers' edges still travels exactly once.
    let mut pool = Pool::default();
    let mut lanes = vec![BandLane {
        id: ALLOCATION_LANE.to_string(),
        name: "Allocation".to_string(),
        overlay: false,
        blocks: resolve(&stack, &mut pool),
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
            blocks: resolve(&[found], &mut pool),
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
        allocations: pool.allocations,
        lanes,
    }
}

/// The plan's allocation table, built as the lanes resolve. Keyed by allocation id, which is
/// unique within a plan by construction — a range is not, because a table routinely gives one
/// range to several services at once.
#[derive(Default)]
struct Pool {
    allocations: Vec<BandAllocation>,
    index: std::collections::HashMap<String, u32>,
}

impl Pool {
    fn intern(&mut self, layer: &Layer, entry: &Entry) -> u32 {
        let built = allocation(layer, entry);
        if let Some(&at) = self.index.get(&built.id) {
            return at;
        }
        // The count is bounded by the tables, which are far smaller than u32.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an allocation table of four billion rows is not a thing"
        )]
        let at = self.allocations.len() as u32;
        self.index.insert(built.id.clone(), at);
        self.allocations.push(built);
        at
    }
}

fn layer(id: &str) -> Option<&'static Layer> {
    LAYERS.iter().find(|layer| layer.id == id)
}

/// Flatten a layer stack into non-overlapping blocks, most-specific-wins.
///
/// An active-set sweep, not a per-layer lookup, because **the source tables overlap themselves**.
/// A regulator routinely gives one range to several services at once — BNetzA hands 435–472 kHz
/// to aeronautical navigation, maritime mobile *and* short-range devices in three separate rows,
/// and the ITU table's 13.36–13.41 MHz is fixed and radio astronomy together. So "the entry
/// covering this frequency in this layer" is not a question with one answer, and any formulation
/// that assumes it is drops co-allocations on the floor.
///
/// Each entry's edges become events; walking them in order keeps the set of entries covering the
/// current interval, and every one of them lands in the block — the winner by rank, the rest as
/// `covered`. Adjacent intervals whose whole set is identical merge back together, so a boundary
/// that exists only because some *other* part of the spectrum has an edge there is not drawn.
fn resolve(stack: &[&'static Layer], pool: &mut Pool) -> Vec<BandBlock> {
    /// An entry's start or stop, tagged with everything the ordering needs.
    struct Event {
        hz: f64,
        opens: bool,
        at: usize,
    }
    // Flattened once so an event can name its entry by index; the order here is also the
    // tie-break of last resort, which makes the output stable across runs.
    let flat: Vec<(&Layer, &Entry, u8)> = stack
        .iter()
        .enumerate()
        .flat_map(|(rank, layer)| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a region names a handful of layers"
            )]
            let rank = rank as u8;
            layer.entries.iter().map(move |entry| (*layer, entry, rank))
        })
        .collect();

    let mut events: Vec<Event> = Vec::with_capacity(flat.len() * 2);
    for (at, (_, entry, _)) in flat.iter().enumerate() {
        events.push(Event {
            hz: entry.start_hz,
            opens: true,
            at,
        });
        events.push(Event {
            hz: entry.stop_hz,
            opens: false,
            at,
        });
    }
    // Closes before opens at the same frequency, so a band ending where the next begins does not
    // briefly appear to be both.
    events.sort_by(|a, b| a.hz.total_cmp(&b.hz).then(a.opens.cmp(&b.opens)));

    let mut active: Vec<usize> = Vec::new();
    let mut blocks: Vec<BandBlock> = Vec::new();
    let mut cursor = 0usize;
    let mut opened_at = f64::NAN;

    while cursor < events.len() {
        let hz = events[cursor].hz;
        // Emit the interval that ends here *before* applying this frequency's events.
        if !active.is_empty() && opened_at.is_finite() && hz > opened_at {
            push_block(&mut blocks, opened_at, hz, &active, &flat, pool);
        }
        while cursor < events.len() && events[cursor].hz == hz {
            let event = &events[cursor];
            if event.opens {
                active.push(event.at);
            } else if let Some(found) = active.iter().position(|&at| at == event.at) {
                active.remove(found);
            }
            cursor += 1;
        }
        opened_at = hz;
    }
    blocks
}

/// Rank the entries covering one interval and record it, merging into the previous block when
/// the whole set is unchanged.
fn push_block(
    blocks: &mut Vec<BandBlock>,
    start: f64,
    stop: f64,
    active: &[usize],
    flat: &[(&Layer, &Entry, u8)],
    pool: &mut Pool,
) {
    let mut order: Vec<usize> = active.to_vec();
    // Most specific layer first; within a layer a primary allocation outranks a secondary one,
    // because a secondary service must accept interference from every primary one and calling it
    // the winner would invert what the band actually is. Flat index last, for a stable order.
    order.sort_by(|&a, &b| {
        let (_, entry_a, rank_a) = &flat[a];
        let (_, entry_b, rank_b) = &flat[b];
        rank_b
            .cmp(rank_a)
            .then(entry_b.primary.cmp(&entry_a.primary))
            .then(a.cmp(&b))
    });
    let mut ids = order
        .iter()
        .map(|&at| {
            let (layer, entry, _) = &flat[at];
            pool.intern(layer, entry)
        })
        .collect::<Vec<u32>>();
    let of = ids.remove(0);

    if let Some(last) = blocks.last_mut()
        && last.stop_hz == start
        && last.of == of
        && last.covered == ids
    {
        last.stop_hz = stop;
        return;
    }
    blocks.push(BandBlock {
        start_hz: start,
        stop_hz: stop,
        of,
        covered: ids,
    });
}

fn allocation(layer: &Layer, entry: &Entry) -> BandAllocation {
    BandAllocation {
        // The source's own row id where it has one. A range alone cannot identify a row in a
        // table that gives one range to several services, and annotation splitting means one
        // source row can become several entries — hence the range in the fallback too.
        id: match &entry.reference {
            Some(reference) => format!("{}:{reference}:{:.0}", layer.id, entry.start_hz),
            None => format!("{}:{:.0}:{}", layer.id, entry.start_hz, entry.name),
        },
        layer: layer.id.clone(),
        start_hz: entry.start_hz,
        stop_hz: entry.stop_hz,
        service: entry.service,
        name: entry.friendly.clone().unwrap_or_else(|| entry.name.clone()),
        official_name: entry.name.clone(),
        primary: entry.primary,
        reference: entry.reference.clone(),
        aliases: entry.aliases.clone(),
        suggested: entry.suggested.clone(),
        channel_step_hz: entry.channel_step_hz,
        notes: entry.notes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sorted and non-empty — but *not* disjoint. Overlap within a layer is what a co-allocation
    /// is, and asserting it away is what would drop one.
    #[test]
    fn every_layer_is_sorted_and_its_rows_are_well_formed() {
        for layer in LAYERS.iter() {
            for entry in &layer.entries {
                assert!(!entry.name.is_empty(), "{}: an unnamed row", layer.id);
                assert!(
                    entry.stop_hz > entry.start_hz,
                    "{}: {} is empty or inverted",
                    layer.id,
                    entry.name
                );
            }
            for pair in layer.entries.windows(2) {
                assert!(
                    pair[1].start_hz >= pair[0].start_hz,
                    "{}: {} sorts before {}",
                    layer.id,
                    pair[1].name,
                    pair[0].name
                );
            }
        }
    }

    /// The annotations overlay is the one hand-written thing left, and it is applied to
    /// *generated* rows — so its attach rule has to be exactly right or an importer's precision
    /// is thrown away by a curated approximation.
    #[test]
    fn an_annotation_labels_the_pieces_it_covers_and_nothing_wider() {
        let entries: Vec<Entry> = serde_json::from_str(
            r#"[
              {"start_hz": 100, "stop_hz": 200, "service": "maritime", "name": "MARITIME MOBILE"},
              {"start_hz": 120, "stop_hz": 130, "service": "maritime", "name": "A NARROW ROW"}
            ]"#,
        )
        .expect("entries");
        let annotations: Vec<Annotation> = serde_json::from_str(
            r#"[{"start_hz": 110, "stop_hz": 150, "name": "Marine VHF", "aliases": ["marine"]}]"#,
        )
        .expect("annotations");

        let out = annotate(entries, &annotations);
        let named = |start: f64| {
            out.iter()
                .find(|e| e.start_hz == start)
                .map(|e| e.friendly.clone().unwrap_or_else(|| e.name.clone()))
                .unwrap_or_default()
        };
        // The wide row is cut at the annotation's edges, and only the middle piece is renamed:
        // labelling all of 100–200 "Marine VHF" would put the name on 90 units that are not.
        assert_eq!(named(100.0), "MARITIME MOBILE");
        assert_eq!(named(110.0), "Marine VHF");
        assert_eq!(named(150.0), "MARITIME MOBILE");
        // The narrow row sits wholly inside the annotation but is the more specific statement,
        // so it keeps its own name — the PMR446-inside-UHF-land-mobile case.
        assert_eq!(named(120.0), "A NARROW ROW");
    }

    /// An allocation's id is what a block references and what the client keys on. A table that
    /// repeats ranges — every real one does — makes `layer:start_hz` ambiguous, so a collision
    /// here would silently merge two services into one.
    #[test]
    fn allocation_ids_are_unique_within_every_plan() {
        for plan in PLANS.iter() {
            let mut seen = std::collections::HashSet::new();
            for allocation in &plan.allocations {
                assert!(
                    seen.insert(allocation.id.clone()),
                    "{}: duplicate allocation id {}",
                    plan.region.id,
                    allocation.id
                );
            }
        }
    }

    /// The property the sweep exists for: where a source gives one range to several services,
    /// all of them survive into the block rather than the last one read winning.
    #[test]
    fn co_allocations_all_survive_into_the_block() {
        // Authored as a document rather than a literal, so this also holds the loader's schema:
        // the shape here is exactly what an importer has to write.
        let layer: &'static Layer = Box::leak(Box::new(
            serde_json::from_str::<Layer>(
                r#"{
                    "id": "test", "name": "test", "authority": "test", "source": "test",
                    "kind": "regulatory",
                    "entries": [
                      {"start_hz": 100, "stop_hz": 200, "service": "maritime",
                       "name": "MARITIME MOBILE", "reference": "a"},
                      {"start_hz": 100, "stop_hz": 200, "service": "navigation",
                       "name": "Radionavigation", "reference": "b", "primary": false},
                      {"start_hz": 150, "stop_hz": 300, "service": "ism",
                       "name": "ISM", "reference": "c"}
                    ]
                }"#,
            )
            .expect("layer document"),
        ));
        let mut pool = Pool::default();
        let blocks = resolve(&[layer], &mut pool);
        let name = |at: u32| pool.allocations[at as usize].official_name.clone();

        // 100–150: two services, the primary one winning over the secondary.
        assert_eq!(blocks[0].start_hz, 100.0);
        assert_eq!(blocks[0].stop_hz, 150.0);
        assert_eq!(name(blocks[0].of), "MARITIME MOBILE");
        assert_eq!(
            blocks[0]
                .covered
                .iter()
                .copied()
                .map(name)
                .collect::<Vec<_>>(),
            vec!["Radionavigation"],
            "a secondary co-allocation must be kept, not dropped"
        );

        // 150–200: all three overlap.
        assert_eq!(blocks[1].stop_hz, 200.0);
        assert_eq!(blocks[1].covered.len(), 2);

        // 200–300: only the one that runs on.
        assert_eq!(name(blocks[2].of), "ISM");
        assert!(blocks[2].covered.is_empty());
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
    /// would be a 400 the operator sees only on click. Now guards `annotations.json`, which is
    /// where the modes live once the layers are generated.
    #[test]
    fn every_suggested_mode_is_a_channel_type_this_build_ships() {
        let types: Vec<String> = sdrmm_engine::Engine::new(None)
            .channel_types()
            .into_iter()
            .map(|descriptor| descriptor.type_id)
            .collect();
        let mut checked = 0usize;
        for (source, suggested) in LAYERS
            .iter()
            .flat_map(|layer| {
                layer
                    .entries
                    .iter()
                    .map(move |entry| (layer.id.clone(), entry.suggested.as_ref()))
            })
            .chain(
                ANNOTATIONS
                    .iter()
                    .map(|a| ("annotations".to_string(), a.suggested.as_ref())),
            )
        {
            let Some(params) = suggested else { continue };
            checked += 1;
            let id = params.type_id();
            assert!(
                types.iter().any(|known| known == id),
                "{source}: unknown channel type {id}"
            );
        }
        assert!(
            checked > 0,
            "nothing suggests a mode — the overlay is empty"
        );
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
                    let of = &plan.allocations[block.of as usize];
                    assert!(of.start_hz <= block.start_hz);
                    assert!(of.stop_hz >= block.stop_hz);
                }
            }
        }
    }

    /// Every index a block carries must be in range and name a layer the plan describes, or the
    /// popover reads the wrong row — or panics.
    #[test]
    fn every_block_indexes_an_allocation_the_plan_carries() {
        for plan in PLANS.iter() {
            for lane in &plan.lanes {
                for block in &lane.blocks {
                    for &at in std::iter::once(&block.of).chain(&block.covered) {
                        let allocation = plan.allocations.get(at as usize).unwrap_or_else(|| {
                            panic!("{}: index {at} out of range", plan.region.id)
                        });
                        assert!(
                            plan.layers.iter().any(|info| info.id == allocation.layer),
                            "{}: block cites unknown layer {}",
                            plan.region.id,
                            allocation.layer
                        );
                    }
                    assert!(
                        !block.covered.contains(&block.of),
                        "{}: a block covers its own winner",
                        plan.region.id
                    );
                }
            }
        }
    }

    /// The property the whole feature rests on: the most specific layer wins, and everything it
    /// covers is still readable underneath it.
    ///
    /// 446.1 MHz is the case worth pinning. BNetzA's own row (Eintrag 248011, 446.0–446.2 MHz)
    /// outranks the CEPT PMR446 entry over the same range, which outranks the ITU land-mobile
    /// allocation over all of it — three layers, narrowing, and none of them lost.
    #[test]
    fn the_national_layer_wins_and_keeps_what_it_covers() {
        let plan = plan("de").expect("germany ships");
        let block = plan.lanes[0]
            .blocks
            .iter()
            .find(|block| block.start_hz <= 446_100_000.0 && block.stop_hz > 446_100_000.0)
            .expect("446.1 MHz is allocated");
        let winner = &plan.allocations[block.of as usize];
        assert_eq!(
            winner.layer, "de",
            "the national table is the most specific"
        );
        assert!(
            winner.reference.is_some(),
            "an imported row carries the id it had in the source document"
        );

        let under: Vec<&str> = block
            .covered
            .iter()
            .map(|&at| plan.allocations[at as usize].layer.as_str())
            .collect();
        assert!(under.contains(&"cept"), "PMR446 is lost under {under:?}");
        assert!(
            under.contains(&"world") || under.contains(&"itu-r1"),
            "the ITU allocation is lost under {under:?}"
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
                .map(|block| plan.allocations[block.of as usize].name.clone())
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
        let two_metres = plan.lanes[0]
            .blocks
            .iter()
            .find(|block| block.start_hz <= 145_000_000.0 && block.stop_hz > 145_000_000.0)
            .expect("2 m is allocated");
        assert_eq!(
            plan.allocations[two_metres.of as usize].service,
            BandService::Amateur
        );
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
