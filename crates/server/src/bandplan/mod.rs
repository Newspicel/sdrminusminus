use std::sync::LazyLock;

use sdrmm_wire::{
    BandAllocation, BandBlock, BandLane, BandLayerInfo, BandLayerKind, BandPlan, BandProvision,
    BandRegion, BandRegionMatch, BandRegionsResponse, BandService, ChannelParams, ItuRegion,
};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Entry {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: BandService,
    pub name: String,
    #[serde(default)]
    pub friendly: Option<String>,
    #[serde(default = "primary_by_default")]
    pub primary: bool,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub suggested: Option<ChannelParams>,
    #[serde(default)]
    pub channel_step_hz: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub provisions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Provision {
    pub id: String,
    pub text: String,
}

const fn primary_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize)]
#[expect(
    dead_code,
    reason = "the URL, timestamp and digest are written for a human reviewing the generated \
              document and its diff; only `generator` is answered over the wire"
)]
pub(crate) struct Provenance {
    pub generator: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Layer {
    pub id: String,
    pub name: String,
    pub authority: String,
    pub source: String,
    pub kind: BandLayerKind,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub provisions: Vec<Provision>,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Annotation {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub suggested: Option<ChannelParams>,
    #[serde(default)]
    pub channel_step_hz: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub service: Option<BandService>,
}

static LAYER_DOCS: &[(&str, &str)] = &[
    ("world", include_str!("../../data/bandplan/world.json")),
    ("itu-r1", include_str!("../../data/bandplan/itu-r1.json")),
    ("itu-r2", include_str!("../../data/bandplan/itu-r2.json")),
    ("itu-r3", include_str!("../../data/bandplan/itu-r3.json")),
    ("cept", include_str!("../../data/bandplan/cept.json")),
    ("de", include_str!("../../data/bandplan/de.json")),
    (
        "de-sonstige",
        include_str!("../../data/bandplan/de-sonstige.json"),
    ),
    ("gb", include_str!("../../data/bandplan/gb.json")),
    ("us", include_str!("../../data/bandplan/us.json")),
    ("iaru-r1", include_str!("../../data/bandplan/iaru-r1.json")),
];

static ANNOTATIONS_DOC: &str = include_str!("../../data/bandplan/annotations.json");

#[expect(clippy::expect_used, reason = "compiled-in constant; see above")]
static ANNOTATIONS: LazyLock<Vec<Annotation>> = LazyLock::new(|| {
    let mut parsed: Vec<Annotation> =
        serde_json::from_str(ANNOTATIONS_DOC).expect("annotations.json is committed and valid");
    parsed.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    parsed
});

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

fn annotate(entries: Vec<Entry>, annotations: &[Annotation]) -> Vec<Entry> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
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
            let found = annotations
                .iter()
                .filter(|a| a.start_hz <= start && a.stop_hz >= stop)
                .filter(|a| a.service.is_none_or(|service| service == entry.service))
                .min_by(|a, b| (a.stop_hz - a.start_hz).total_cmp(&(b.stop_hz - b.start_hz)));
            if let Some(annotation) = found {
                if entry_width >= annotation.stop_hz - annotation.start_hz {
                    piece.friendly = Some(annotation.name.clone());
                    piece.aliases = annotation.aliases.clone();
                    piece.notes = piece.notes.or_else(|| annotation.notes.clone());
                }
                piece.suggested = piece.suggested.or_else(|| annotation.suggested.clone());
                piece.channel_step_hz = piece.channel_step_hz.or(annotation.channel_step_hz);
            }
            out.push(piece);
        }
    }
    out
}

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
    layers: &'static [&'static str],
    overlays: &'static [&'static str],
    footprint: &'static [Bbox],
}

static REGIONS: &[RegionDef] = &[
    RegionDef {
        id: "de",
        name: "Germany — BNetzA",
        country: Some("DE"),
        itu: ItuRegion::R1,
        layers: &["world", "itu-r1", "cept", "de"],
        overlays: &["de-sonstige", "iaru-r1"],
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

pub(crate) const DEFAULT_REGION: &str = "itu-r1";

const ALLOCATION_LANE: &str = "allocation";

static PLANS: LazyLock<Vec<BandPlan>> = LazyLock::new(|| REGIONS.iter().map(build).collect());

pub(crate) fn regions() -> BandRegionsResponse {
    BandRegionsResponse {
        regions: PLANS.iter().map(|plan| plan.region.clone()).collect(),
        default_region: DEFAULT_REGION.to_string(),
    }
}

pub(crate) fn plan(region: &str) -> Option<BandPlan> {
    PLANS.iter().find(|plan| plan.region.id == region).cloned()
}

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

fn itu_region_of(lat: f64, lon: f64) -> ItuRegion {
    const GREENLAND: Bbox = Bbox {
        lat: (58.0, 84.0),
        lon: (-74.0, -11.0),
    };
    const ASIA_PACIFIC: Bbox = Bbox {
        lat: (-55.0, 45.0),
        lon: (60.0, 180.0),
    };
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

    let provisions = cited(&layers);
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
        provisions,
    }
}

fn cited(layers: &[BandLayerInfo]) -> Vec<BandProvision> {
    layers
        .iter()
        .filter_map(|info| layer(&info.id))
        .flat_map(|found| {
            found.provisions.iter().map(|provision| BandProvision {
                layer: found.id.clone(),
                id: provision.id.clone(),
                text: provision.text.clone(),
            })
        })
        .collect()
}

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

fn resolve(stack: &[&'static Layer], pool: &mut Pool) -> Vec<BandBlock> {
    struct Event {
        hz: f64,
        opens: bool,
        at: usize,
    }
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
    events.sort_by(|a, b| a.hz.total_cmp(&b.hz).then(a.opens.cmp(&b.opens)));

    let mut active: Vec<usize> = Vec::new();
    let mut blocks: Vec<BandBlock> = Vec::new();
    let mut cursor = 0usize;
    let mut opened_at = f64::NAN;

    while cursor < events.len() {
        let hz = events[cursor].hz;
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

fn push_block(
    blocks: &mut Vec<BandBlock>,
    start: f64,
    stop: f64,
    active: &[usize],
    flat: &[(&Layer, &Entry, u8)],
    pool: &mut Pool,
) {
    let mut order: Vec<usize> = active.to_vec();
    order.sort_by(|&a, &b| {
        let (_, entry_a, rank_a) = &flat[a];
        let (_, entry_b, rank_b) = &flat[b];
        rank_b
            .cmp(rank_a)
            .then(entry_b.primary.cmp(&entry_a.primary))
            .then(
                (entry_a.stop_hz - entry_a.start_hz)
                    .total_cmp(&(entry_b.stop_hz - entry_b.start_hz)),
            )
            .then(a.cmp(&b))
    });
    let mut ids: Vec<u32> = Vec::with_capacity(order.len());
    for &at in &order {
        let (layer, entry, _) = &flat[at];
        let id = pool.intern(layer, entry);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
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
        id: match &entry.reference {
            Some(reference) => format!(
                "{}:{reference}:{:.0}:{:.0}",
                layer.id, entry.start_hz, entry.stop_hz
            ),
            None => format!(
                "{}:{:.0}:{:.0}:{}",
                layer.id, entry.start_hz, entry.stop_hz, entry.name
            ),
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
        provisions: entry.provisions.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(named(100.0), "MARITIME MOBILE");
        assert_eq!(named(110.0), "Marine VHF");
        assert_eq!(named(150.0), "MARITIME MOBILE");
        assert_eq!(named(120.0), "A NARROW ROW");
    }

    #[test]
    fn a_row_too_narrow_to_be_renamed_still_takes_the_annotation_it_sits_in() {
        let entries: Vec<Entry> = serde_json::from_str(
            r#"[{"start_hz": 120, "stop_hz": 130, "service": "maritime", "name": "A NARROW ROW"}]"#,
        )
        .expect("entries");
        let annotations: Vec<Annotation> = serde_json::from_str(
            r#"[{"start_hz": 110, "stop_hz": 150, "name": "Marine VHF", "channel_step_hz": 25000}]"#,
        )
        .expect("annotations");

        let out = annotate(entries, &annotations);
        assert_eq!(
            out[0].friendly, None,
            "the wider annotation does not rename it"
        );
        assert_eq!(
            out[0].channel_step_hz,
            Some(25_000.0),
            "but its tuning hints apply to everything inside it"
        );
        assert_eq!(
            out[0].notes, None,
            "nor does it describe a row it does not name"
        );
    }

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

    #[test]
    fn co_allocations_all_survive_into_the_block() {
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

        assert_eq!(blocks[1].stop_hz, 200.0);
        assert_eq!(blocks[1].covered.len(), 2);

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
                    assert!(
                        of.start_hz <= block.start_hz,
                        "{}/{}: block {}-{} starts before {} {}-{}",
                        plan.region.id,
                        lane.id,
                        block.start_hz,
                        block.stop_hz,
                        of.id,
                        of.start_hz,
                        of.stop_hz
                    );
                    assert!(
                        of.stop_hz >= block.stop_hz,
                        "{}/{}: block {}-{} ends after {} {}-{}",
                        plan.region.id,
                        lane.id,
                        block.start_hz,
                        block.stop_hz,
                        of.id,
                        of.start_hz,
                        of.stop_hz
                    );
                }
            }
        }
    }

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
        assert!(of("us", 915_000_000.0).contains("ISM"));
        assert!(!of("de", 915_000_000.0).contains("ISM"));
    }

    #[test]
    fn the_annex_lane_prefers_the_narrowest_application_over_a_blanket_permission() {
        let plan = plan("de").expect("germany ships");
        let lane = plan
            .lanes
            .iter()
            .find(|lane| lane.id == "de-sonstige")
            .expect("germany carries the annex of other applications");
        assert!(
            lane.overlay,
            "it allocates nothing, so it cannot win a band"
        );
        let block = lane
            .blocks
            .iter()
            .find(|block| block.start_hz <= 433_920_000.0 && block.stop_hz > 433_920_000.0)
            .expect("433,92 MHz is an ISM frequency");
        let winner = &plan.allocations[block.of as usize];
        assert_eq!(winner.start_hz, 433_050_000.0);
        assert_eq!(winner.stop_hz, 434_790_000.0);
        let under: Vec<&str> = block
            .covered
            .iter()
            .map(|&at| plan.allocations[at as usize].official_name.as_str())
            .collect();
        assert!(
            under.contains(&"UWB-Funkanwendungen"),
            "the wider permissions still sit beneath it: {under:?}"
        );
        assert!(
            !plan.lanes[0]
                .blocks
                .iter()
                .any(|block| { plan.allocations[block.of as usize].layer == "de-sonstige" }),
            "the annex never wins a block in the allocation lane"
        );
    }

    #[test]
    fn a_german_allocation_cites_nutzungsbestimmungen_the_plan_carries() {
        let plan = plan("de").expect("germany ships");
        let cited = plan
            .allocations
            .iter()
            .find(|allocation| allocation.reference.as_deref() == Some("27004"))
            .expect("the avalanche beacon band");
        assert_eq!(cited.provisions, ["1", "2", "5"]);
        for id in &cited.provisions {
            let provision = plan
                .provisions
                .iter()
                .find(|found| found.layer == cited.layer && found.id == *id)
                .unwrap_or_else(|| panic!("{id} has no text"));
            assert!(!provision.text.is_empty());
        }
    }

    #[test]
    fn the_amateur_overlay_is_its_own_lane() {
        let plan = plan("de").expect("germany ships");
        let overlay = plan
            .lanes
            .iter()
            .find(|lane| lane.id == "iaru-r1")
            .expect("germany carries the IARU R1 overlay");
        assert!(overlay.overlay);
        assert_eq!(
            plan.lanes.last().map(|lane| lane.id.as_str()),
            Some("iaru-r1"),
            "the amateur overlay has the last word on what a click tunes"
        );
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

        let saopaulo = locate(-23.55, -46.63);
        assert_eq!(saopaulo.region, "itu-r2");

        assert_eq!(locate(72.0, -40.0).itu_region, ItuRegion::R1);
    }

    #[test]
    fn unknown_regions_are_none() {
        assert!(plan("nope").is_none());
        assert!(plan("").is_none());
    }

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
