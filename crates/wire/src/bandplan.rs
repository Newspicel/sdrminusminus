//! Frequency-allocation types (FEATURES §5): what a frequency is *for*, layered from the ITU
//! world table down through a national regulator, with the amateur band plan as an overlay.
//!
//! The layering is resolved server-side and shipped as one already-merged document per region
//! (`BandPlan`), because a curated table is small and static: clipping it to the window a scope
//! is showing, and searching it, are then pure client arithmetic that survive a pan without a
//! round trip. Nothing on the server is stateful about it — the region is the client's choice
//! and travels in the path.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::channel::ChannelParams;

/// The three ITU radio regions (Radio Regulations Art. 5 §1). Region 1 is Europe, Africa, the
/// Middle East west of the Persian Gulf and northern Asia; Region 2 the Americas; Region 3 the
/// rest of Asia and Oceania.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ItuRegion {
    R1,
    R2,
    R3,
}

/// What kind of authority a layer speaks with. This is not decoration: it is why the amateur
/// plan renders as a separate lane instead of overriding the regulator — IARU band plans are
/// agreements between operators, not allocations, and they subdivide a band the regulator has
/// already given to the amateur service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BandLayerKind {
    /// The ITU table — global footnotes and the per-region allocations.
    World,
    /// A national or supranational regulator (BNetzA, FCC, Ofcom, CEPT).
    Regulatory,
    /// A band plan agreed between users of a service rather than imposed on it.
    Amateur,
}

/// The service category a block belongs to. Drives the ruler's colour, so it is deliberately
/// coarse: ten categories a reader can hold in their head, not the ITU's full service list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BandService {
    Amateur,
    Broadcast,
    Aeronautical,
    Maritime,
    /// Land mobile, PMR, cellular — anything that moves on the ground.
    Mobile,
    Satellite,
    /// Radionavigation and radiolocation, including radar.
    Navigation,
    /// Radio astronomy, Earth-exploration satellite, space research, standard time and frequency.
    Science,
    /// Licence-exempt short-range and ISM.
    Ism,
    /// Fixed links, government, and anything the table names but this categorisation does not.
    Other,
}

/// One source of allocations: a table someone published, identified so a block can say where it
/// came from. Adding a region is adding one of these plus its entries (FEATURES §5's "pluggable
/// importers"); nothing else in the resolution knows the difference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandLayerInfo {
    /// Stable id, referenced by [`BandAllocation::layer`] and [`BandRegion::layers`].
    pub id: String,
    pub name: String,
    /// Who publishes it — "ITU", "BNetzA", "Ofcom", "IARU Region 1".
    pub authority: String,
    /// The document and edition the entries were taken from, so a stale table is visible rather
    /// than merely wrong.
    pub source: String,
    pub kind: BandLayerKind,
    /// Least-specific first: where this layer sits when two of them cover one frequency. Ties
    /// cannot happen — every layer in a region has a distinct rank.
    pub rank: u8,
}

/// What a stretch of spectrum is allocated to, as one layer states it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandAllocation {
    /// `layer:start_hz`, stable across requests.
    pub id: String,
    /// [`BandLayerInfo::id`] this entry came from.
    pub layer: String,
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: BandService,
    /// What an operator calls it: "2 m amateur", "Marine VHF", "Airband".
    pub name: String,
    /// Other names the explorer matches on — wavelengths ("70 cm"), colloquialisms ("CB"),
    /// and the service spelled the long way ("marine", "maritime mobile").
    pub aliases: Vec<String>,
    /// The mode to tune with, where the band has an obvious one. Sent to the channel verbatim,
    /// so "tune here with the suggested mode" needs no client-side mode table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested: Option<ChannelParams>,
    /// Channel raster, where the band is channelized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_step_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One resolved stretch: what wins here, and what it covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandBlock {
    pub start_hz: f64,
    pub stop_hz: f64,
    /// The most specific layer's entry over this stretch.
    pub allocation: BandAllocation,
    /// The layers underneath, most specific first. Empty when nothing else covers this stretch.
    /// This is what lets the identify popover say "BNetzA calls it X, over ITU's Y".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<BandAllocation>,
}

/// A row of the ruler. The regulatory layers merge into one lane by most-specific-wins; an
/// overlay is a lane of its own, because it describes the same spectrum from a different angle
/// rather than contradicting it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandLane {
    pub id: String,
    pub name: String,
    /// Whether the lane is supplementary and may be switched off without losing the allocation.
    pub overlay: bool,
    /// Sorted by `start_hz`, non-overlapping.
    pub blocks: Vec<BandBlock>,
}

/// A region the operator can select: the ITU region plus whichever regulator applies there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegion {
    pub id: String,
    pub name: String,
    /// ISO 3166-1 alpha-2 where the region is one country; `None` for the bare ITU regions and
    /// for CEPT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    pub itu_region: ItuRegion,
    /// Layer ids, least specific first.
    pub layers: Vec<String>,
    /// Overlay layer ids — the amateur plans that apply here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<String>,
}

/// `GET /api/bandplan/regions`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegionsResponse {
    pub regions: Vec<BandRegion>,
    /// What a client with no stored preference should select.
    pub default_region: String,
}

/// `GET /api/bandplan/regions/{region}` — the whole region, already layered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandPlan {
    pub region: BandRegion,
    /// Every layer the lanes reference, so a block's `layer` id resolves without a second call.
    pub layers: Vec<BandLayerInfo>,
    pub lanes: Vec<BandLane>,
}

/// `GET /api/bandplan/locate` — which region a coordinate falls in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegionMatch {
    /// The most specific region whose footprint contains the coordinate; never empty, because
    /// the three ITU regions cover the globe.
    pub region: String,
    pub itu_region: ItuRegion,
    /// True when only the ITU region could be decided — the coordinate is outside every
    /// national footprint this build ships, so the answer is coarse and the UI should say so.
    pub approximate: bool,
}

/// Query for `GET /api/bandplan/locate`. Flattened into the operation's parameters by utoipa,
/// the way [`crate::rest::DecoderLogQuery`] is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, utoipa::IntoParams)]
pub struct LocateQuery {
    /// Degrees north, −90…90.
    pub lat: f64,
    /// Degrees east, −180…180.
    pub lon: f64,
}
