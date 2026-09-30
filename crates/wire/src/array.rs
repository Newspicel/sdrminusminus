use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::device::{Coherence, Range};
use crate::geo::LatLon;
use crate::position::HeadingSource;

pub const LIGHT_SPEED_M_S: f64 = 299_792_458.0;
pub const MAX_ARRAY_LANES: u32 = 16;
pub const MIN_ARRAY_LANES: u32 = 2;
pub const MAX_VIRTUAL_LANES: u32 = 16;
pub const MAX_ARRAY_EXTENT_M: f64 = 100.0;
pub const MIN_CHECK_S: u32 = 10;
pub const MAX_CHECK_S: u32 = 3_600;
pub const DEFAULT_CHECK_S: u32 = 60;
pub const MIN_CAL_BANDWIDTH_HZ: f64 = 100.0;
pub const MAX_CAL_BANDWIDTH_HZ: f64 = 2_000_000.0;
pub const DEFAULT_CAL_BANDWIDTH_HZ: f64 = 20_000.0;
pub const MAX_CAL_OFFSET_HZ: f64 = 50_000_000.0;
pub const MIN_ARRAY_GAIN_DB: f64 = -20.0;
pub const MAX_ARRAY_GAIN_DB: f64 = 80.0;
pub const EQ_POINTS: usize = 64;
pub const MAX_CAL_RECORDS_PER_ARRAY: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Winding {
    #[default]
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayElement {
    pub x_m: f64,
    pub y_m: f64,
    #[serde(default)]
    pub z_m: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayGeometry {
    Uca {
        radius_m: f64,
        #[serde(default)]
        first_deg: f64,
        #[serde(default)]
        winding: Winding,
    },
    Ula {
        spacing_m: f64,
        axis_deg: f64,
    },
    Explicit {
        positions: Vec<ArrayElement>,
    },
}

impl Default for ArrayGeometry {
    fn default() -> Self {
        Self::Uca {
            radius_m: 0.35,
            first_deg: 0.0,
            winding: Winding::Clockwise,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    #[error("needs at least {MIN_ARRAY_LANES} lanes")]
    TooFewLanes,
    #[error("max {MAX_ARRAY_LANES} lanes")]
    TooManyLanes,
    #[error("geometry has {positions}, wired {lanes}")]
    CountMismatch { positions: u32, lanes: u32 },
    #[error("an element is outside {MAX_ARRAY_EXTENT_M} m")]
    OutOfRange,
    #[error("geometry values must be finite")]
    NotFinite,
}

fn within_extent(value: f64) -> bool {
    value.abs() <= MAX_ARRAY_EXTENT_M
}

fn positive_extent(value: f64) -> bool {
    value > 0.0 && value <= MAX_ARRAY_EXTENT_M
}

fn lane_count(lanes: usize) -> Result<u32, GeometryError> {
    let count = u32::try_from(lanes).map_err(|_| GeometryError::TooManyLanes)?;
    if count < MIN_ARRAY_LANES {
        Err(GeometryError::TooFewLanes)
    } else if count > MAX_ARRAY_LANES {
        Err(GeometryError::TooManyLanes)
    } else {
        Ok(count)
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dz.mul_add(dz, dx.mul_add(dx, dy * dy)).sqrt()
}

fn uca(radius_m: f64, first_deg: f64, winding: Winding, lanes: u32) -> Vec<[f64; 3]> {
    let sign = match winding {
        Winding::Clockwise => 1.0,
        Winding::CounterClockwise => -1.0,
    };
    let n = f64::from(lanes);
    (0..lanes)
        .map(|i| {
            let alpha = (sign * 360.0 * f64::from(i) / n + first_deg).to_radians();
            [radius_m * alpha.sin(), radius_m * alpha.cos(), 0.0]
        })
        .collect()
}

fn ula(spacing_m: f64, axis_deg: f64, lanes: u32) -> Vec<[f64; 3]> {
    let axis = axis_deg.to_radians();
    let centre = (f64::from(lanes) - 1.0) / 2.0;
    (0..lanes)
        .map(|i| {
            let along = (f64::from(i) - centre) * spacing_m;
            [along * axis.sin(), along * axis.cos(), 0.0]
        })
        .collect()
}

impl ArrayGeometry {
    fn finite(&self) -> bool {
        match self {
            Self::Uca {
                radius_m,
                first_deg,
                ..
            } => radius_m.is_finite() && first_deg.is_finite(),
            Self::Ula {
                spacing_m,
                axis_deg,
            } => spacing_m.is_finite() && axis_deg.is_finite(),
            Self::Explicit { positions } => positions
                .iter()
                .all(|p| p.x_m.is_finite() && p.y_m.is_finite() && p.z_m.is_finite()),
        }
    }

    fn layout(&self, lanes: u32) -> Result<Vec<[f64; 3]>, GeometryError> {
        match self {
            Self::Uca {
                radius_m,
                first_deg,
                winding,
            } => positive_extent(*radius_m)
                .then(|| uca(*radius_m, *first_deg, *winding, lanes))
                .ok_or(GeometryError::OutOfRange),
            Self::Ula {
                spacing_m,
                axis_deg,
            } => positive_extent(*spacing_m)
                .then(|| ula(*spacing_m, *axis_deg, lanes))
                .ok_or(GeometryError::OutOfRange),
            Self::Explicit { positions } => {
                let count = u32::try_from(positions.len()).unwrap_or(u32::MAX);
                if count == lanes {
                    Ok(positions.iter().map(|p| [p.x_m, p.y_m, p.z_m]).collect())
                } else {
                    Err(GeometryError::CountMismatch {
                        positions: count,
                        lanes,
                    })
                }
            }
        }
    }

    pub fn positions(&self, lanes: usize) -> Result<Vec<[f64; 3]>, GeometryError> {
        let lanes = lane_count(lanes)?;
        if !self.finite() {
            return Err(GeometryError::NotFinite);
        }
        let positions = self.layout(lanes)?;
        if positions.iter().flatten().all(|v| within_extent(*v)) {
            Ok(positions)
        } else {
            Err(GeometryError::OutOfRange)
        }
    }

    #[must_use]
    pub fn adjacent_spacing_m(&self, lanes: usize) -> Option<f64> {
        let positions = self.positions(lanes).ok()?;
        let spacing = match self {
            Self::Uca { radius_m, .. } => {
                let n = f64::from(lane_count(lanes).ok()?);
                2.0 * radius_m * (180.0 / n).to_radians().sin()
            }
            Self::Ula { spacing_m, .. } => *spacing_m,
            Self::Explicit { .. } => positions
                .iter()
                .enumerate()
                .flat_map(|(i, a)| positions[i + 1..].iter().map(|b| distance(*a, *b)))
                .fold(f64::INFINITY, f64::min),
        };
        (spacing.is_finite() && spacing > 0.0).then_some(spacing)
    }

    #[must_use]
    pub fn unambiguous_hz(&self, lanes: usize) -> Option<f64> {
        self.adjacent_spacing_m(lanes)
            .map(|spacing| LIGHT_SPEED_M_S / (2.0 * spacing))
    }

    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        match self {
            Self::Uca {
                radius_m,
                first_deg,
                ..
            } => {
                if !positive_extent(*radius_m) {
                    Some("Radius out of range")
                } else if !first_deg.is_finite() {
                    Some("First element out of range")
                } else {
                    None
                }
            }
            Self::Ula {
                spacing_m,
                axis_deg,
            } => {
                if !positive_extent(*spacing_m) {
                    Some("Spacing out of range")
                } else if !axis_deg.is_finite() {
                    Some("Axis out of range")
                } else {
                    None
                }
            }
            Self::Explicit { positions } => {
                let count_ok = lane_count(positions.len()).is_ok();
                let values_ok = positions
                    .iter()
                    .flat_map(|p| [p.x_m, p.y_m, p.z_m])
                    .all(|v| v.is_finite() && within_extent(v));
                (!(count_ok && values_ok)).then_some("Positions out of range")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayOrientation {
    Fixed { azimuth_deg: f64 },
    Heading { mount_offset_deg: f64 },
}

impl Default for ArrayOrientation {
    fn default() -> Self {
        Self::Fixed { azimuth_deg: 0.0 }
    }
}

impl ArrayOrientation {
    fn problem(&self) -> Option<&'static str> {
        match *self {
            Self::Fixed { azimuth_deg } if !azimuth_deg.is_finite() => Some("Azimuth out of range"),
            Self::Heading { mount_offset_deg } if !mount_offset_deg.is_finite() => {
                Some("Mount out of range")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrayTuningMode {
    #[default]
    Together,
    Spread,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayGain {
    Manual { db: f64 },
    Auto,
}

impl Default for ArrayGain {
    fn default() -> Self {
        Self::Manual { db: 30.0 }
    }
}

impl ArrayGain {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        match self {
            Self::Manual { db } if !(MIN_ARRAY_GAIN_DB..=MAX_ARRAY_GAIN_DB).contains(db) => {
                Some("Gain out of range")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayCalSource {
    Noise,
    Pilot {
        offset_hz: f64,
        bandwidth_hz: f64,
    },
    Emitter {
        offset_hz: f64,
        bandwidth_hz: f64,
        bearing_deg: f64,
    },
    Off,
}

impl ArrayCalSource {
    #[must_use]
    pub const fn kind(&self) -> Option<CalSourceKind> {
        match self {
            Self::Noise => Some(CalSourceKind::Noise),
            Self::Pilot { .. } => Some(CalSourceKind::Pilot),
            Self::Emitter { .. } => Some(CalSourceKind::Emitter),
            Self::Off => None,
        }
    }

    fn problem(&self) -> Option<&'static str> {
        let (offset_hz, bandwidth_hz, bearing_deg) = match *self {
            Self::Noise | Self::Off => return None,
            Self::Pilot {
                offset_hz,
                bandwidth_hz,
            } => (offset_hz, bandwidth_hz, 0.0),
            Self::Emitter {
                offset_hz,
                bandwidth_hz,
                bearing_deg,
            } => (offset_hz, bandwidth_hz, bearing_deg),
        };
        if !(-MAX_CAL_OFFSET_HZ..=MAX_CAL_OFFSET_HZ).contains(&offset_hz) {
            Some("Offset out of range")
        } else if !(MIN_CAL_BANDWIDTH_HZ..=MAX_CAL_BANDWIDTH_HZ).contains(&bandwidth_hz) {
            Some("Width out of range")
        } else if !bearing_deg.is_finite() {
            Some("Bearing out of range")
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ArrayCal {
    pub source: ArrayCalSource,
    pub check_s: u32,
    pub equaliser: bool,
    pub warm_start: bool,
}

impl Default for ArrayCal {
    fn default() -> Self {
        Self {
            source: ArrayCalSource::Noise,
            check_s: DEFAULT_CHECK_S,
            equaliser: false,
            warm_start: true,
        }
    }
}

impl ArrayCal {
    fn problem(&self) -> Option<&'static str> {
        self.source.problem().or_else(|| {
            let check = self.check_s;
            (check != 0 && !(MIN_CHECK_S..=MAX_CHECK_S).contains(&check))
                .then_some("Check out of range")
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ArrayNode {
    pub geometry: ArrayGeometry,
    pub orientation: ArrayOrientation,
    pub declared: Coherence,
    pub tuning: ArrayTuningMode,
    pub cal: ArrayCal,
}

impl Default for ArrayNode {
    fn default() -> Self {
        Self {
            geometry: ArrayGeometry::default(),
            orientation: ArrayOrientation::default(),
            declared: Coherence::TimeSync,
            tuning: ArrayTuningMode::Together,
            cal: ArrayCal::default(),
        }
    }
}

impl ArrayNode {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        self.geometry
            .problem()
            .or_else(|| (self.declared == Coherence::None).then_some("Tier out of range"))
            .or_else(|| self.orientation.problem())
            .or_else(|| self.cal.problem())
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayTune {
    pub center_hz: f64,
    pub gain: ArrayGain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayTuneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<ArrayGain>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceArray {
    pub node: String,
    pub tune: ArrayTune,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    #[default]
    Idle,
    Searching,
    Locked,
    Drifting,
    Lost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalPhase {
    #[default]
    None,
    Waiting,
    Measuring,
    Solved,
    Warm,
    Stale,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessorGate {
    Sync,
    Phase,
    Gain,
    Calibrating,
    Retuning,
    Tier,
    TuningMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayFailure {
    Unwired,
    LaneGap { lane: u32 },
    DuplicateLane { lane: u32 },
    TooManyLanes,
    LaneHeld { lane: u32, by: String },
    DeviceDown { lane: u32 },
    NotCoherent,
    RatesDiffer,
    SpreadUnsupported,
    NoNoiseSource,
    NoiseShared,
    NoiseNotSeen,
    NoiseClips { lane: u32 },
    LowCoherence { lane: u32, coherence: f32 },
    NoCommonSignal,
    ClockDrift { ppm: f64 },
    SlipsRepeated,
    GeometryMismatch { positions: u32, lanes: u32 },
    NeedsPosition,
    Stopped { message: String },
    Busy,
}

pub const ARRAY_FAILURE_LABELS: [(&str, &str); 21] = [
    ("unwired", "Wire lanes"),
    ("lane_gap", "Lane {n} unwired"),
    ("duplicate_lane", "Lane {n} twice"),
    ("too_many_lanes", "Max 16 lanes"),
    ("lane_held", "Lane {n} in {by}"),
    ("device_down", "Radio {n} down"),
    ("not_coherent", "Lanes not coherent"),
    ("rates_differ", "Rates differ"),
    ("spread_unsupported", "Radio tunes lanes together"),
    ("no_noise_source", "No noise source"),
    ("noise_shared", "Noise needs all lanes"),
    ("noise_not_seen", "Noise not seen"),
    ("noise_clips", "Noise clips, lower gain"),
    ("low_coherence", "Lane {n} weak"),
    ("no_common_signal", "No common signal"),
    ("clock_drift", "Clocks drift {ppm} ppm"),
    ("slips_repeated", "Lanes keep slipping"),
    ("geometry_mismatch", "Geometry has {p}, wired {l}"),
    ("needs_position", "Wire a GPS"),
    ("stopped", "Stopped"),
    ("busy", "Busy"),
];

impl ArrayFailure {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Unwired => "unwired",
            Self::LaneGap { .. } => "lane_gap",
            Self::DuplicateLane { .. } => "duplicate_lane",
            Self::TooManyLanes => "too_many_lanes",
            Self::LaneHeld { .. } => "lane_held",
            Self::DeviceDown { .. } => "device_down",
            Self::NotCoherent => "not_coherent",
            Self::RatesDiffer => "rates_differ",
            Self::SpreadUnsupported => "spread_unsupported",
            Self::NoNoiseSource => "no_noise_source",
            Self::NoiseShared => "noise_shared",
            Self::NoiseNotSeen => "noise_not_seen",
            Self::NoiseClips { .. } => "noise_clips",
            Self::LowCoherence { .. } => "low_coherence",
            Self::NoCommonSignal => "no_common_signal",
            Self::ClockDrift { .. } => "clock_drift",
            Self::SlipsRepeated => "slips_repeated",
            Self::GeometryMismatch { .. } => "geometry_mismatch",
            Self::NeedsPosition => "needs_position",
            Self::Stopped { .. } => "stopped",
            Self::Busy => "busy",
        }
    }

    #[must_use]
    pub fn template(&self) -> &'static str {
        let kind = self.kind();
        ARRAY_FAILURE_LABELS
            .iter()
            .find(|(named, _)| *named == kind)
            .map_or("", |(_, template)| template)
    }
}

impl std::fmt::Display for ArrayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = self.template();
        let filled = match self {
            Self::LaneGap { lane }
            | Self::DuplicateLane { lane }
            | Self::DeviceDown { lane }
            | Self::LowCoherence { lane, .. } => {
                text.replace("{n}", &lane.saturating_add(1).to_string())
            }
            Self::LaneHeld { lane, by } => text
                .replace("{n}", &lane.saturating_add(1).to_string())
                .replace("{by}", by),
            Self::ClockDrift { ppm } => text.replace("{ppm}", &format!("{ppm:.1}")),
            Self::GeometryMismatch { positions, lanes } => text
                .replace("{p}", &positions.to_string())
                .replace("{l}", &lanes.to_string()),
            _ => text.to_owned(),
        };
        f.write_str(&filled)
    }
}

impl SyncState {
    pub const ALL: [Self; 5] = [
        Self::Idle,
        Self::Searching,
        Self::Locked,
        Self::Drifting,
        Self::Lost,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Searching => "Syncing",
            Self::Locked => "Locked",
            Self::Drifting => "Drifting",
            Self::Lost => "Lost",
        }
    }
}

impl CalPhase {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Waiting,
        Self::Measuring,
        Self::Solved,
        Self::Warm,
        Self::Stale,
        Self::Failed,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "No cal",
            Self::Waiting => "Waiting",
            Self::Measuring => "Calibrating",
            Self::Solved => "Calibrated",
            Self::Warm => "Warm",
            Self::Stale => "Stale",
            Self::Failed => "Cal failed",
        }
    }
}

impl ProcessorGate {
    pub const ALL: [Self; 7] = [
        Self::Sync,
        Self::Phase,
        Self::Gain,
        Self::Calibrating,
        Self::Retuning,
        Self::Tier,
        Self::TuningMode,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sync => "Syncing",
            Self::Phase | Self::Gain => "Needs cal",
            Self::Calibrating => "Calibrating",
            Self::Retuning => "Retuning",
            Self::Tier => "Not coherent",
            Self::TuningMode => "Wrong tuning",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayLaneStatus {
    pub lane: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_set: Option<u32>,
    pub stream: u32,
    pub sync: SyncState,
    pub delay_samples: f64,
    pub phase_deg: f32,
    pub gain_db: f32,
    pub coherence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_delay: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_phase_deg: Option<f32>,
    pub level_dbfs: f32,
    pub clipping: bool,
    pub gaps: u64,
    pub gap_samples: u64,
    pub uncertain: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProcessorStatus {
    pub node: String,
    pub kind: String,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gated: Option<ProcessorGate>,
    pub gated_samples: u64,
    pub dropped_samples: u64,
    pub dropped_reports: u64,
    pub lane_overflows: u64,
    pub lane_mismatch: u64,
    pub solver_failures: u64,
    pub resets: u64,
    #[serde(default)]
    pub truncated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayRecordingStatus {
    pub stem: String,
    pub started_at: String,
    pub samples: u64,
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayStatus {
    pub node: String,
    pub lanes: Vec<ArrayLaneStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<u32>,
    pub tier: Coherence,
    pub declared: Coherence,
    pub tier_capped: bool,
    pub sync: SyncState,
    pub cal: CalPhase,
    pub phase_ready: bool,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub tuning: ArrayTuningMode,
    pub gain: ArrayGain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_range_db: Option<Range>,
    pub generation: u32,
    pub realigns: u64,
    pub dropped_samples: u64,
    pub events_lost: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_ppm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_solve_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_check_in_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_source: Option<HeadingSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<LatLon>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unambiguous_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ArrayFailure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processors: Vec<ProcessorStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<ArrayRecordingStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct LaneKey {
    pub device: String,
    pub stream: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalSourceKind {
    Noise,
    Pilot,
    Emitter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LaneSolution {
    pub delay_samples: f64,
    pub phase_deg: f64,
    pub gain_db: f64,
    pub coherence: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Vec<f32>>)]
    pub equaliser: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayCalRecord {
    pub lanes: Vec<LaneKey>,
    pub center_hz: f64,
    pub sample_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    pub source: CalSourceKind,
    pub keeps_phase: bool,
    pub solved_at: String,
    pub solution: Vec<LaneSolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeldLane {
    pub stream: u32,
    pub array: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VirtualLane {
    pub stream: u32,
    pub node: String,
    pub port: String,
    pub center_hz: f64,
    pub sample_rate: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ArrayRecordingRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ArrayRecordingStarted {
    pub stem: String,
}

#[cfg(test)]
mod tests;
