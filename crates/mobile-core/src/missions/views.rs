use crate::records::LatLon;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MissionKind {
    Hunt,
    DfDrive,
    RadarWatch,
    Survey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MissionControl {
    Tune,
    Calibrate,
    ClearFusion,
    HuntRun,
    Sweep,
    Mark,
    SurveyRun,
    SurveyClear,
    TargetMode,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Mission {
    pub id: String,
    pub kind: MissionKind,
    pub title: String,
    pub detail: String,
    pub ready: bool,
    pub blocker: Option<String>,
    pub controls: Vec<MissionControl>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MissionsView {
    pub workspace: WorkspaceRef,
    pub workspaces: Vec<WorkspaceRef>,
    pub missions: Vec<Mission>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TargetMode {
    Auto,
    Direct,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum MissionCommand {
    StartHunt,
    StopHunt,
    Sweep { on: bool },
    Mark,
    Tune { hz: f64 },
    Calibrate,
    ClearFusion,
    SetTargetMode { mode: TargetMode },
    StartSurvey,
    StopSurvey,
    ClearSurvey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Trend {
    Waiting,
    Warmer,
    Colder,
    OnTop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SweepPhase {
    Off,
    Idle,
    Sweeping,
    NoHeading,
    ShortSpan,
    LowContrast,
    PoorFit,
    HeadingPoor,
    TooFast,
    Done,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct SweepView {
    pub bins: Vec<u8>,
    pub peak_deg: Option<f32>,
    pub covered_deg: f32,
    pub phase: SweepPhase,
    pub sigma_deg: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HuntView {
    pub mission: String,
    pub freq_hz: f64,
    pub level_db: Option<f32>,
    pub smooth_db: Option<f32>,
    pub floor_db: Option<f32>,
    pub best_db: Option<f32>,
    pub strength: f32,
    pub trend: Trend,
    pub running: bool,
    pub refusal: Option<String>,
    pub readings: u64,
    pub sweep: Option<SweepView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DfState {
    Waiting,
    Calibrating,
    PhaseUnknown,
    NoHeading,
    Squelched,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum GuidanceKind {
    Probe,
    Estimate,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct NavPoint {
    pub at: LatLon,
    pub kind: GuidanceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct GuidanceView {
    pub kind: GuidanceKind,
    pub heading_true_deg: f64,
    pub heading_rel_deg: Option<f64>,
    pub distance_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct EstimateView {
    pub at: LatLon,
    pub major_m: f64,
    pub minor_m: f64,
    pub axis_deg: f64,
    pub converged: bool,
    pub samples: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Ray {
    pub from: LatLon,
    pub to: LatLon,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct Station {
    pub id: String,
    pub at: LatLon,
    pub bearings: u32,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HeatBand {
    pub level: f32,
    pub rings: Vec<Vec<LatLon>>,
}

#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct DfOverlay {
    pub rays: Vec<Ray>,
    pub stations: Vec<Station>,
    pub ellipse: Vec<LatLon>,
    pub heat: Vec<HeatBand>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DfView {
    pub mission: String,
    pub state: DfState,
    pub bearing_true_deg: Option<f32>,
    pub bearing_rel_deg: Option<f32>,
    pub confidence: f32,
    pub sigma_deg: Option<f32>,
    pub freq_hz: f64,
    pub target_mode: TargetMode,
    pub guidance: Option<GuidanceView>,
    pub target: Option<NavPoint>,
    pub estimate: Option<EstimateView>,
    pub overlay: DfOverlay,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct RadarTrack {
    pub id: u32,
    pub range_km: f32,
    pub doppler_hz: f32,
    pub speed_mps: f32,
    pub snr_db: f32,
    pub closing: bool,
    pub coasting: bool,
    pub bearing_deg: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RadarView {
    pub mission: String,
    pub echoes: u32,
    pub tracks: Vec<RadarTrack>,
    pub stale: bool,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub range_max_km: f32,
    pub doppler_span_hz: f32,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct SurveyView {
    pub mission: String,
    pub freq_hz: f64,
    pub level_db: Option<f32>,
    pub min_db: f32,
    pub max_db: f32,
    pub total: u64,
    pub recording: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct SurveyPoint {
    pub at: LatLon,
    pub level_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RetargetReason {
    First,
    Moved,
    KindChanged,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RetargetNotice {
    pub mission: String,
    pub target: NavPoint,
    pub moved_m: f64,
    pub reason: RetargetReason,
}
