use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub use crate::array::LIGHT_SPEED_M_S;
use crate::array::MAX_ARRAY_LANES;
use crate::limits::RADAR_LIMITS as LIMITS;
use crate::processor::reserved_at;

pub const RADAR_MIN_CPI_MS: u32 = 50;
pub const RADAR_MAX_CPI_MS: u32 = 2_000;
pub const RADAR_MAX_OVERLAP: f32 = 0.75;
pub const RADAR_MAX_RANGE_KM: f32 = 400.0;
pub const RADAR_MIN_SPEED_MPS: f32 = 10.0;
pub const RADAR_MAX_SPEED_MPS: f32 = 2_000.0;
pub const RADAR_MAX_GATES: u32 = 2_048;
pub const RADAR_MIN_BATCHES: u32 = 16;
pub const RADAR_MAX_BATCHES: u32 = 4_096;
pub const RADAR_MAX_OFFSET_HZ: f64 = 50_000_000.0;
pub const RADAR_MIN_BANDWIDTH_HZ: f64 = 10_000.0;
pub const RADAR_MAX_BANDWIDTH_HZ: f64 = 20_000_000.0;
pub const RADAR_MAX_WORKING_BYTES: u64 = 512 * 1024 * 1024;
pub const FM_BANDWIDTH_HZ: f64 = 200_000.0;
pub const DAB_BANDWIDTH_HZ: f64 = 1_536_000.0;
pub const DAB_SAMPLE_RATE_HZ: f64 = 2_048_000.0;
pub const DVBT_BANDWIDTH_HZ: f64 = 7_610_000.0;
pub const MAX_ECA_ORDER: u32 = 512;
pub const MAX_ECA_LEAD: u32 = 16;
pub const MAX_ECA_DOPPLER_TAPS: u32 = 2;
pub const MAX_CMA_TAPS: u32 = 64;
pub const MAX_CFAR_GUARD: u32 = 16;
pub const MAX_CFAR_TRAIN_RANGE: u32 = 64;
pub const MAX_CFAR_TRAIN_DOPPLER: u32 = 16;
pub const MAX_TRACK_WINDOW: u32 = 16;
pub const MAX_RADAR_DETECTIONS: usize = 128;
pub const MAX_RADAR_TRACKS: usize = 64;
pub const MAX_RADAR_TENTATIVE: usize = 128;
pub const MAX_TRACK_TRAIL: usize = 16;
pub const MAX_RADAR_TRUTH: usize = 64;
pub const SURFACE_DB_MIN: f32 = -3.0;
pub const SURFACE_DB_MAX: f32 = 30.0;
pub const RADAR_TRACK_EVENT_REPEAT_S: u64 = 5;
pub const MAX_RADAR_ALTITUDE_M: f32 = 15_000.0;
pub const DEFAULT_CUSTOM_BANDWIDTH_HZ: f64 = FM_BANDWIDTH_HZ;
pub const DEFAULT_CMA_TAPS: u32 = 16;
pub const DEFAULT_CMA_STEP: f32 = 1e-3;
pub const DEFAULT_OS_RANK: f32 = 0.75;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Illuminator {
    Fm,
    Dab,
    DvbtPartial { bandwidth_hz: f64 },
    Custom { bandwidth_hz: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurveillanceSet {
    AllOthers,
    Mask { mask: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReferenceCleaning {
    Off,
    Cma { taps: u32, step: f32 },
    DabRemod,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClutterMethod {
    #[default]
    EcaBatch,
    EcaSliding,
    Nlms,
    BlockNlms,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ClutterParams {
    pub method: ClutterMethod,
    pub reach_km: f32,
    pub lead: u32,
    pub doppler_taps: u32,
    pub batch_ms: f32,
    pub taper: bool,
    pub extension_ms: f32,
    pub step: f32,
    pub loading: f32,
}

impl Default for ClutterParams {
    fn default() -> Self {
        Self {
            method: ClutterMethod::EcaBatch,
            reach_km: 15.0,
            lead: 2,
            doppler_taps: 0,
            batch_ms: 50.0,
            taper: true,
            extension_ms: 25.0,
            step: 0.05,
            loading: 1e-4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DopplerWindow {
    #[default]
    Hann,
    BlackmanHarris,
    Rectangular,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CfarKind {
    Ca,
    Os { rank: f32 },
    Go,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CfarWindow {
    #[default]
    Range,
    Plane,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct CfarParams {
    pub kind: CfarKind,
    pub window: CfarWindow,
    pub pfa: f64,
    pub guard_range: u32,
    pub train_range: u32,
    pub guard_doppler: u32,
    pub train_doppler: u32,
    pub min_doppler_hz: f32,
    pub min_range_km: f32,
    pub min_snr_db: f32,
}

impl Default for CfarParams {
    fn default() -> Self {
        Self {
            kind: CfarKind::Ca,
            window: CfarWindow::Range,
            pfa: 1e-5,
            guard_range: 2,
            train_range: 8,
            guard_doppler: 1,
            train_doppler: 4,
            min_doppler_hz: 5.0,
            min_range_km: 1.0,
            min_snr_db: 8.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct TrackerParams {
    pub confirm_hits: u32,
    pub confirm_window: u32,
    pub coast_looks: u32,
    pub max_accel_mps2: f32,
    pub gate: f32,
    pub jerk: f32,
}

impl Default for TrackerParams {
    fn default() -> Self {
        Self {
            confirm_hits: 3,
            confirm_window: 5,
            coast_looks: 10,
            max_accel_mps2: 30.0,
            gate: 11.8,
            jerk: 5.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuUse {
    #[default]
    Auto,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct PassiveRadarParams {
    pub reference_element: u32,
    pub surveillance: SurveillanceSet,
    pub illuminator: Illuminator,
    pub offset_hz: f64,
    pub cpi_ms: u32,
    pub overlap: f32,
    pub max_range_km: f32,
    pub max_speed_mps: f32,
    pub window: DopplerWindow,
    pub reference: ReferenceCleaning,
    pub clutter: ClutterParams,
    pub cfar: CfarParams,
    pub tracker: TrackerParams,
    pub aoa: bool,
    pub assumed_altitude_m: f32,
    pub gpu: GpuUse,
}

impl Default for PassiveRadarParams {
    fn default() -> Self {
        Self {
            reference_element: 0,
            surveillance: SurveillanceSet::AllOthers,
            illuminator: Illuminator::Fm,
            offset_hz: 0.0,
            cpi_ms: 500,
            overlap: 0.0,
            max_range_km: 80.0,
            max_speed_mps: 400.0,
            window: DopplerWindow::Hann,
            reference: ReferenceCleaning::Cma {
                taps: DEFAULT_CMA_TAPS,
                step: DEFAULT_CMA_STEP,
            },
            clutter: ClutterParams::default(),
            cfar: CfarParams::default(),
            tracker: TrackerParams::default(),
            aoa: true,
            assumed_altitude_m: 8_000.0,
            gpu: GpuUse::Auto,
        }
    }
}

impl Illuminator {
    #[must_use]
    pub const fn bandwidth_hz(&self) -> f64 {
        match self {
            Self::Fm => FM_BANDWIDTH_HZ,
            Self::Dab => DAB_BANDWIDTH_HZ,
            Self::DvbtPartial { bandwidth_hz } | Self::Custom { bandwidth_hz } => *bandwidth_hz,
        }
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Fm => "FM",
            Self::Dab => "DAB",
            Self::DvbtPartial { .. } => "DVB-T",
            Self::Custom { .. } => "Custom",
        }
    }
}

impl SurveillanceSet {
    const ELEMENT_BITS: u32 = MAX_ARRAY_LANES;

    pub fn elements(&self, reference: u32, elements: u32) -> impl Iterator<Item = u32> {
        let mask = match *self {
            Self::AllOthers => u32::MAX,
            Self::Mask { mask } => mask,
        };
        (0..elements.min(Self::ELEMENT_BITS))
            .filter(move |element| *element != reference && mask & (1 << element) != 0)
    }

    fn problem(&self, reference: u32) -> Option<&'static str> {
        let Self::Mask { mask } = *self else {
            return None;
        };
        let usable = mask & ((1 << Self::ELEMENT_BITS) - 1);
        if usable == 0 || usable != mask {
            Some("No surveillance element")
        } else if reference < Self::ELEMENT_BITS && mask & (1 << reference) != 0 {
            Some("Surveillance holds the reference")
        } else {
            None
        }
    }
}

impl ReferenceCleaning {
    fn problem(&self, illuminator: Illuminator) -> Option<&'static str> {
        match *self {
            Self::Off => None,
            Self::DabRemod => {
                (!matches!(illuminator, Illuminator::Dab)).then_some("DAB remod needs DAB")
            }
            Self::Cma { taps, step } => {
                if !matches!(illuminator, Illuminator::Fm | Illuminator::Custom { .. }) {
                    Some("CMA needs FM")
                } else if !LIMITS.cma_taps.contains(taps) {
                    Some("CMA taps out of range")
                } else if !LIMITS.cma_step.contains(step) {
                    Some("CMA step out of range")
                } else {
                    None
                }
            }
        }
    }
}

fn first_problem(checks: &[(bool, &'static str)]) -> Option<&'static str> {
    checks.iter().find(|(ok, _)| !ok).map(|(_, text)| *text)
}

impl ClutterParams {
    fn problem(&self) -> Option<&'static str> {
        first_problem(&[
            (
                LIMITS.reach_km.contains(self.reach_km),
                "Clutter reach out of range",
            ),
            (LIMITS.lead.contains(self.lead), "Lead out of range"),
            (
                LIMITS.doppler_taps.contains(self.doppler_taps),
                "Doppler taps out of range",
            ),
            (
                LIMITS.batch_ms.contains(self.batch_ms),
                "Batch out of range",
            ),
            (
                LIMITS.extension_ms.contains(self.extension_ms),
                "Extension out of range",
            ),
            (LIMITS.clutter_step.contains(self.step), "Step out of range"),
            (
                LIMITS.loading.contains(self.loading),
                "Loading out of range",
            ),
        ])
    }
}

impl CfarParams {
    fn problem(&self) -> Option<&'static str> {
        let rank_ok = match self.kind {
            CfarKind::Os { rank } => LIMITS.os_rank.contains(rank),
            CfarKind::Ca | CfarKind::Go => true,
        };
        first_problem(&[
            (rank_ok, "Rank out of range"),
            (LIMITS.pfa.contains(self.pfa), "Pfa out of range"),
            (
                LIMITS.guard.contains(self.guard_range)
                    && LIMITS.guard.contains(self.guard_doppler),
                "Guard out of range",
            ),
            (
                LIMITS.train_range.contains(self.train_range)
                    && LIMITS.train_doppler.contains(self.train_doppler),
                "Train out of range",
            ),
            (
                LIMITS.min_doppler_hz.contains(self.min_doppler_hz),
                "Min Doppler out of range",
            ),
            (
                LIMITS.min_range_km.contains(self.min_range_km),
                "Min range out of range",
            ),
            (
                LIMITS.min_snr_db.contains(self.min_snr_db),
                "Min SNR out of range",
            ),
        ])
    }
}

impl TrackerParams {
    fn problem(&self) -> Option<&'static str> {
        first_problem(&[
            (
                LIMITS.track_window.contains(self.confirm_hits)
                    && LIMITS.track_window.contains(self.confirm_window)
                    && self.confirm_hits <= self.confirm_window,
                "M of N out of range",
            ),
            (
                LIMITS.coast_looks.contains(self.coast_looks),
                "Coast out of range",
            ),
            (
                LIMITS.max_accel_mps2.contains(self.max_accel_mps2),
                "Accel out of range",
            ),
            (LIMITS.gate.contains(self.gate), "Gate out of range"),
            (LIMITS.jerk.contains(self.jerk), "Jerk out of range"),
        ])
    }
}

impl PassiveRadarParams {
    fn frame_problem(&self) -> Option<&'static str> {
        first_problem(&[
            (LIMITS.cpi_ms.contains(self.cpi_ms), "CPI out of range"),
            (
                LIMITS.overlap.contains(self.overlap),
                "Overlap out of range",
            ),
            (
                LIMITS.max_range_km.contains(self.max_range_km),
                "Range out of range",
            ),
            (
                LIMITS.max_speed_mps.contains(self.max_speed_mps),
                "Speed out of range",
            ),
            (
                LIMITS.offset_hz.contains(self.offset_hz),
                "Offset out of range",
            ),
            (
                LIMITS
                    .bandwidth_hz
                    .contains(self.illuminator.bandwidth_hz()),
                "Bandwidth out of range",
            ),
            (
                self.reference_element < MAX_ARRAY_LANES,
                "Reference out of range",
            ),
        ])
    }

    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        self.frame_problem()
            .or_else(|| self.surveillance.problem(self.reference_element))
            .or_else(|| self.reference.problem(self.illuminator))
            .or_else(|| self.clutter.problem())
            .or_else(|| self.cfar.problem())
            .or_else(|| self.tracker.problem())
            .or_else(|| {
                (!LIMITS.altitude_m.contains(self.assumed_altitude_m))
                    .then_some("Altitude out of range")
            })
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarAxes {
    pub sample_rate_hz: f64,
    pub carrier_hz: f64,
    pub range_step_m: f32,
    pub gates: u32,
    pub doppler_step_hz: f32,
    pub doppler_rows: u32,
    pub batches: u32,
    pub cpi_ms: f32,
    pub hop_ms: f32,
    pub lanes: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarAoa {
    pub azimuth_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing_deg: Option<f32>,
    pub sigma_deg: f32,
    pub quality: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_deg: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarDetection {
    pub range_km: f32,
    pub doppler_hz: f32,
    pub range_rate_mps: f32,
    pub snr_db: f32,
    pub cells: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aoa: Option<RadarAoa>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
    #[default]
    Confirmed,
    Coasting,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarTrailPoint {
    pub range_km: f32,
    pub doppler_hz: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarFix {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
    #[serde(default)]
    pub alt_from_adsb: bool,
    pub major_m: f32,
    pub minor_m: f32,
    pub orientation_deg: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbMatch {
    pub icao: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarTrack {
    pub id: u32,
    pub state: TrackState,
    pub range_km: f32,
    pub range_rate_mps: f32,
    pub doppler_hz: f32,
    pub accel_mps2: f32,
    pub range_sigma_m: f32,
    pub rate_sigma_mps: f32,
    pub snr_db: f32,
    pub looks: u32,
    pub misses: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aoa: Option<RadarAoa>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<RadarFix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adsb: Option<AdsbMatch>,
    pub trail: Vec<RadarTrailPoint>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbTruth {
    pub icao: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    pub range_km: f32,
    pub doppler_hz: f32,
    pub bearing_deg: f32,
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: f32,
    pub age_s: f32,
    pub in_view: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMode {
    #[default]
    Raw,
    Cma,
    DabRemod,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReferenceHealth {
    pub mode: ReferenceMode,
    pub locked: bool,
    pub quality_db: f32,
    pub fallback_frames: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AoaState {
    #[default]
    Off,
    Ready,
    PhaseUnknown,
    OneLane,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarHealth {
    pub suppression_db: Vec<f32>,
    pub unsuppressed_groups: u64,
    pub dropped_samples: u64,
    pub dropped_cpis: u64,
    pub discarded_cpis: u64,
    pub dropped_reports: u64,
    pub lagged_updates: u64,
    pub truncated_detections: u64,
    pub dropped_tracks: u64,
    pub gpu_failures: u64,
    pub load: f32,
    pub front_load: f32,
    pub compute_ms: f32,
    pub noise_floor_db: f32,
    #[serde(default)]
    pub cfar_looks: u32,
    #[serde(default)]
    pub range_correlation: f32,
    pub gpu: bool,
    pub threads: u32,
    pub aoa: AoaState,
    #[serde(default)]
    pub table_out_of_range: bool,
    pub reference: ReferenceHealth,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarSite {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarGeometry {
    pub receiver: RadarSite,
    pub transmitter: RadarSite,
    pub baseline_km: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum RadarProblem {
    NoArray,
    NoTransmitter,
    NoReceiver,
    NoHeading,
    PhaseUnknown,
    TableOutOfRange,
    Overloaded,
    ReferenceLost,
    Refused(String),
}

impl RadarProblem {
    pub const ALL: [Self; 9] = [
        Self::NoArray,
        Self::NoTransmitter,
        Self::NoReceiver,
        Self::NoHeading,
        Self::PhaseUnknown,
        Self::TableOutOfRange,
        Self::Overloaded,
        Self::ReferenceLost,
        Self::Refused(String::new()),
    ];

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NoArray => "no_array",
            Self::NoTransmitter => "no_transmitter",
            Self::NoReceiver => "no_receiver",
            Self::NoHeading => "no_heading",
            Self::PhaseUnknown => "phase_unknown",
            Self::TableOutOfRange => "table_out_of_range",
            Self::Overloaded => "overloaded",
            Self::ReferenceLost => "reference_lost",
            Self::Refused(_) => "refused",
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::NoArray => "No array",
            Self::NoTransmitter => "No transmitter",
            Self::NoReceiver => "No array position",
            Self::NoHeading => "No heading",
            Self::PhaseUnknown => "Phase unknown",
            Self::TableOutOfRange => "Outside cal table",
            Self::Overloaded => "Overloaded",
            Self::ReferenceLost => "Reference lost",
            Self::Refused(text) => text,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarUpdate {
    pub seq: u64,
    pub at: String,
    pub axes: RadarAxes,
    pub detections: Vec<RadarDetection>,
    pub tracks: Vec<RadarTrack>,
    pub truth: Vec<AdsbTruth>,
    pub health: RadarHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<RadarGeometry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<RadarProblem>,
    #[serde(skip)]
    pub events: Vec<RadarTrackEvent>,
}

impl RadarUpdate {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            detections: Vec::with_capacity(MAX_RADAR_DETECTIONS),
            tracks: Vec::with_capacity(MAX_RADAR_TRACKS),
            truth: Vec::with_capacity(MAX_RADAR_TRUTH),
            health: RadarHealth {
                suppression_db: Vec::with_capacity(MAX_ARRAY_LANES as usize),
                ..RadarHealth::default()
            },
            problems: Vec::with_capacity(RadarProblem::ALL.len()),
            events: Vec::with_capacity(MAX_RADAR_TRACKS),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackChange {
    Confirmed,
    Update,
    Lost,
}

impl TrackChange {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Update => "update",
            Self::Lost => "lost",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarTrackEvent {
    pub track_id: u32,
    pub change: TrackChange,
    pub range_km: f32,
    pub range_rate_mps: f32,
    pub doppler_hz: f32,
    pub snr_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icao: Option<String>,
}

#[cfg(test)]
mod tests;
