use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{device::Coherence, position::PositionFix};

pub const MAX_ARRAY_ELEMENTS: u32 = 16;
pub const MIN_ARRAY_ELEMENTS: u32 = 2;
pub const MAX_ARRAY_EXTENT_M: f64 = 100.0;
pub const MIN_DF_REPORT_MS: u32 = 100;
pub const MAX_DF_REPORT_MS: u32 = 10_000;
pub const MIN_DF_BANDWIDTH_HZ: f64 = 100.0;
pub const MAX_DF_BANDWIDTH_HZ: f64 = 20_000_000.0;
pub const MAX_DF_OFFSET_HZ: f64 = 100_000_000.0;
pub const DF_SPECTRUM_POINTS: usize = 360;

pub const MIN_CAL_BANDWIDTH_HZ: f64 = 100.0;
pub const MAX_CAL_BANDWIDTH_HZ: f64 = 20_000_000.0;

pub const MIN_CPI_MS: u32 = 10;
pub const MAX_CPI_MS: u32 = 2_000;
pub const MAX_RANGE_BINS: u32 = 2_048;
pub const MAX_DOPPLER_SPAN_HZ: f64 = 5_000.0;
pub const MAX_ECA_TAPS: u32 = 256;
pub const MAX_ECA_DOPPLER_BINS: u32 = 4;
pub const MAX_CFAR_WINDOW: u32 = 64;

pub const MAX_STATION_ID_LEN: usize = 64;

/// Where the elements are, in the terms an operator can measure with a tape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrayGeometry {
    /// A circle of evenly spaced elements, element zero due north.
    Uca { radius_m: f64, count: u32 },
    /// A straight line laid out east–west, centred on the operator's position.
    Ula { spacing_m: f64, count: u32 },
    /// Anything else, one position per lane in metres east and north of the array centre.
    Explicit { positions: Vec<ArrayElement> },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArrayElement {
    pub x_m: f64,
    pub y_m: f64,
}

impl Default for ArrayGeometry {
    fn default() -> Self {
        Self::Uca {
            radius_m: 0.35,
            count: 4,
        }
    }
}

impl ArrayGeometry {
    #[must_use]
    pub fn count(&self) -> u32 {
        match self {
            Self::Uca { count, .. } | Self::Ula { count, .. } => *count,
            Self::Explicit { positions } => positions.len() as u32,
        }
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        let count = self.count();
        if !(MIN_ARRAY_ELEMENTS..=MAX_ARRAY_ELEMENTS).contains(&count) {
            return false;
        }
        match self {
            Self::Uca { radius_m, .. } => {
                radius_m.is_finite() && *radius_m > 0.0 && *radius_m <= MAX_ARRAY_EXTENT_M
            }
            Self::Ula { spacing_m, .. } => {
                spacing_m.is_finite() && *spacing_m > 0.0 && *spacing_m <= MAX_ARRAY_EXTENT_M
            }
            Self::Explicit { positions } => positions.iter().all(|element| {
                element.x_m.is_finite()
                    && element.y_m.is_finite()
                    && element.x_m.abs() <= MAX_ARRAY_EXTENT_M
                    && element.y_m.abs() <= MAX_ARRAY_EXTENT_M
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DfAlgorithm {
    /// Conventional beamforming. Blunt, and it always answers.
    #[default]
    Correlative,
    /// Subspace estimation. Far sharper, and it needs the source count to be right.
    Music,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalSource {
    /// Any strong signal every lane can hear, which is the ordinary case on the air.
    #[default]
    Signal,
    /// A noise burst injected through a splitter, for a bench calibration with nothing on.
    Noise,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct CalParams {
    pub source: CalSource,
    /// The width around the tuned centre the solve looks at.
    pub bandwidth_hz: f64,
    /// A continuous carrier that lets a time-synced array re-solve phase after every retune.
    /// Without one such an array reports `phase_unknown` and refuses to guess a bearing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pilot_hz: Option<f64>,
    /// Whether the solution keeps being refined once it is good, rather than being frozen.
    pub track: bool,
}

impl Default for CalParams {
    fn default() -> Self {
        Self {
            source: CalSource::Signal,
            bandwidth_hz: 200_000.0,
            pilot_hz: None,
            track: true,
        }
    }
}

impl CalParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        (MIN_CAL_BANDWIDTH_HZ..=MAX_CAL_BANDWIDTH_HZ).contains(&self.bandwidth_hz)
            && self
                .pilot_hz
                .is_none_or(|hz| hz.is_finite() && hz.abs() <= MAX_DF_OFFSET_HZ)
    }
}

/// Which coherent processor a node runs, and how it is set up. The one place a coherent node's
/// settings live, exactly as `ChannelParams` is for an ordinary channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum CoherentParams {
    Df(DfParams),
    PassiveRadar(PassiveRadarParams),
}

impl CoherentParams {
    #[must_use]
    pub const fn type_id(&self) -> &'static str {
        match self {
            Self::Df(_) => "df",
            Self::PassiveRadar(_) => "passive_radar",
        }
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        match self {
            Self::Df(params) => params.valid(),
            Self::PassiveRadar(params) => params.valid(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct DfParams {
    pub geometry: ArrayGeometry,
    pub algorithm: DfAlgorithm,
    pub report_ms: u32,
    /// Where in the tuned span the signal of interest sits, and how much of it to take.
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    /// How many arrivals MUSIC should assume. One is right far more often than not.
    pub sources: u32,
    pub cal: CalParams,
}

impl Default for DfParams {
    fn default() -> Self {
        Self {
            geometry: ArrayGeometry::default(),
            algorithm: DfAlgorithm::Correlative,
            report_ms: 500,
            offset_hz: 0.0,
            bandwidth_hz: 20_000.0,
            sources: 1,
            cal: CalParams::default(),
        }
    }
}

impl DfParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.geometry.valid()
            && (MIN_DF_REPORT_MS..=MAX_DF_REPORT_MS).contains(&self.report_ms)
            && self.offset_hz.is_finite()
            && self.offset_hz.abs() <= MAX_DF_OFFSET_HZ
            && (MIN_DF_BANDWIDTH_HZ..=MAX_DF_BANDWIDTH_HZ).contains(&self.bandwidth_hz)
            && (1..self.geometry.count()).contains(&self.sources)
            && self.cal.valid()
    }
}

/// One bearing, and the whole circle it was read off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfReading {
    pub bearing_deg: f32,
    pub confidence: f32,
    pub peak_to_floor_db: f32,
    /// One byte per degree, full scale at the peak.
    pub pseudospectrum: Vec<u8>,
}

impl Default for DfReading {
    fn default() -> Self {
        Self {
            bearing_deg: 0.0,
            confidence: 0.0,
            peak_to_floor_db: 0.0,
            pseudospectrum: vec![0; DF_SPECTRUM_POINTS],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LaneCal {
    pub delay_samples: f32,
    pub phase_deg: f32,
    pub gain_db: f32,
    /// Magnitude-squared coherence against lane zero, in `0..=1`.
    pub quality: f32,
}

/// What the calibration currently knows, published whether or not it is good news.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CalState {
    pub tier: Coherence,
    pub lanes: Vec<LaneCal>,
    /// Set when inter-lane phase cannot be trusted — a time-synced array with no pilot to
    /// re-solve against. Everything that depends on phase stays off while it is set.
    pub phase_unknown: bool,
    pub solved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct EcaParams {
    pub delay_taps: u32,
    pub doppler_bins: u32,
    pub batch_samples: u32,
    pub loading: f32,
}

impl Default for EcaParams {
    fn default() -> Self {
        Self {
            delay_taps: 32,
            doppler_bins: 0,
            batch_samples: 16_384,
            loading: 1e-4,
        }
    }
}

impl EcaParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        (1..=MAX_ECA_TAPS).contains(&self.delay_taps)
            && self.doppler_bins <= MAX_ECA_DOPPLER_BINS
            && self.batch_samples >= self.delay_taps
            && self.batch_samples <= 1 << 20
            && (0.0..=1.0).contains(&self.loading)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct CfarParams {
    pub guard_range: u32,
    pub guard_doppler: u32,
    pub train_range: u32,
    pub train_doppler: u32,
    pub probability_false_alarm: f32,
    pub min_snr_db: f32,
    /// Doppler rows either side of zero that are never reported: the direct path and the ground
    /// live there, and neither is a target.
    pub zero_doppler_guard: u32,
}

impl Default for CfarParams {
    fn default() -> Self {
        Self {
            guard_range: 2,
            guard_doppler: 1,
            train_range: 8,
            train_doppler: 4,
            probability_false_alarm: 1e-4,
            min_snr_db: 6.0,
            zero_doppler_guard: 1,
        }
    }
}

impl CfarParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.guard_range <= MAX_CFAR_WINDOW
            && self.guard_doppler <= MAX_CFAR_WINDOW
            && (1..=MAX_CFAR_WINDOW).contains(&self.train_range)
            && (1..=MAX_CFAR_WINDOW).contains(&self.train_doppler)
            && self.probability_false_alarm > 0.0
            && self.probability_false_alarm < 1.0
            && (0.0..=60.0).contains(&self.min_snr_db)
            && self.zero_doppler_guard <= MAX_CFAR_WINDOW
    }
}

/// The transmitter being borrowed, so a bistatic range can be drawn on a map.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Illuminator {
    pub lat: f64,
    pub lon: f64,
    pub freq_hz: f64,
}

impl Illuminator {
    #[must_use]
    pub fn valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.lat)
            && (-180.0..=180.0).contains(&self.lon)
            && self.freq_hz > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct PassiveRadarParams {
    pub cpi_ms: u32,
    pub max_range_bins: u32,
    pub doppler_span_hz: f64,
    pub eca: EcaParams,
    pub cfar: CfarParams,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illuminator: Option<Illuminator>,
}

impl Default for PassiveRadarParams {
    fn default() -> Self {
        Self {
            cpi_ms: 200,
            max_range_bins: 256,
            doppler_span_hz: 200.0,
            eca: EcaParams::default(),
            cfar: CfarParams::default(),
            illuminator: None,
        }
    }
}

impl PassiveRadarParams {
    #[must_use]
    pub fn valid(&self) -> bool {
        (MIN_CPI_MS..=MAX_CPI_MS).contains(&self.cpi_ms)
            && (1..=MAX_RANGE_BINS).contains(&self.max_range_bins)
            && self.doppler_span_hz > 0.0
            && self.doppler_span_hz <= MAX_DOPPLER_SPAN_HZ
            && self.eca.valid()
            && self.cfar.valid()
            && self.illuminator.is_none_or(|source| source.valid())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarDetection {
    pub range_bin: u32,
    /// Bistatic range in kilometres: how much further the echo travelled than the direct path.
    pub range_km: f32,
    pub doppler_hz: f32,
    pub snr_db: f32,
}

/// Where the fusion grid says the transmitter is, and how sure of itself it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfEstimate {
    pub lat: f64,
    pub lon: f64,
    pub ellipse_major_m: f64,
    pub ellipse_minor_m: f64,
    pub ellipse_bearing_deg: f64,
    pub converged: bool,
    pub samples: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceMode {
    /// Drive across the bearing. Two bearings from the same place say nothing; two from
    /// different places cross, and crossing them is the whole method.
    Cross,
    /// The estimate has closed up. Drive at it.
    Approach,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavTargetKind {
    Cross,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NavTarget {
    pub lat: f64,
    pub lon: f64,
    pub kind: NavTargetKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfGuidance {
    pub heading_deg: f64,
    pub mode: GuidanceMode,
    pub distance_m: f64,
    pub nav_target: NavTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfStation {
    pub station_id: String,
    pub lat: f64,
    pub lon: f64,
    pub bearings: u32,
    pub last_seen: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfFusionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<DfEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<DfGuidance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<DfStation>,
    pub samples: u32,
}

/// One bearing as an event, so it reaches the map, the log and every event output the same way a
/// decoded packet does — and so a remote station's webhook can post one straight into a
/// central fusion grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfBearing {
    pub bearing_deg: f32,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_id: Option<String>,
}

/// A bearing another station measured, arriving over the same event output any decoder uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BearingReport {
    pub station_id: String,
    pub lat: f64,
    pub lon: f64,
    pub bearing_deg: f64,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

impl BearingReport {
    #[must_use]
    pub fn valid(&self) -> bool {
        !self.station_id.is_empty()
            && self.station_id.len() <= MAX_STATION_ID_LEN
            && (-90.0..=90.0).contains(&self.lat)
            && (-180.0..=180.0).contains(&self.lon)
            && self.bearing_deg.is_finite()
            && (0.0..=1.0).contains(&self.confidence)
    }

    #[must_use]
    pub fn from_fix(
        station_id: String,
        fix: &PositionFix,
        bearing_deg: f64,
        confidence: f32,
    ) -> Self {
        Self {
            station_id,
            lat: fix.latitude,
            lon: fix.longitude,
            bearing_deg,
            confidence,
            time: Some(fix.time.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_array_is_a_usable_one() {
        let params = DfParams::default();
        assert!(params.valid());
        assert_eq!(params.geometry.count(), 4);
    }

    #[test]
    fn an_array_with_too_few_or_too_many_elements_is_refused() {
        for count in [0u32, 1, MAX_ARRAY_ELEMENTS + 1] {
            let geometry = ArrayGeometry::Uca {
                radius_m: 0.35,
                count,
            };
            assert!(!geometry.valid(), "{count} elements must be refused");
        }
    }

    #[test]
    fn an_array_with_no_extent_is_refused() {
        assert!(
            !ArrayGeometry::Uca {
                radius_m: 0.0,
                count: 4
            }
            .valid()
        );
        assert!(
            !ArrayGeometry::Ula {
                spacing_m: -1.0,
                count: 4
            }
            .valid()
        );
    }

    #[test]
    fn explicit_positions_are_counted_and_bounded() {
        let geometry = ArrayGeometry::Explicit {
            positions: vec![
                ArrayElement { x_m: 0.0, y_m: 0.5 },
                ArrayElement { x_m: 0.5, y_m: 0.0 },
            ],
        };
        assert_eq!(geometry.count(), 2);
        assert!(geometry.valid());
        let far = ArrayGeometry::Explicit {
            positions: vec![
                ArrayElement { x_m: 0.0, y_m: 0.0 },
                ArrayElement {
                    x_m: MAX_ARRAY_EXTENT_M * 2.0,
                    y_m: 0.0,
                },
            ],
        };
        assert!(!far.valid());
    }

    #[test]
    fn a_source_count_the_array_cannot_resolve_is_refused() {
        let params = DfParams {
            sources: 4,
            ..DfParams::default()
        };
        assert!(!params.valid(), "four elements cannot resolve four sources");
    }

    #[test]
    fn radar_defaults_are_valid_and_the_edges_are_not() {
        assert!(PassiveRadarParams::default().valid());
        assert!(
            !PassiveRadarParams {
                cpi_ms: 0,
                ..PassiveRadarParams::default()
            }
            .valid()
        );
        assert!(
            !PassiveRadarParams {
                max_range_bins: MAX_RANGE_BINS + 1,
                ..PassiveRadarParams::default()
            }
            .valid()
        );
        assert!(
            !PassiveRadarParams {
                eca: EcaParams {
                    delay_taps: 0,
                    ..EcaParams::default()
                },
                ..PassiveRadarParams::default()
            }
            .valid()
        );
        assert!(
            !PassiveRadarParams {
                cfar: CfarParams {
                    probability_false_alarm: 1.0,
                    ..CfarParams::default()
                },
                ..PassiveRadarParams::default()
            }
            .valid()
        );
    }

    #[test]
    fn an_illuminator_off_the_globe_is_refused() {
        assert!(
            !Illuminator {
                lat: 91.0,
                lon: 0.0,
                freq_hz: 100e6
            }
            .valid()
        );
        assert!(
            Illuminator {
                lat: 51.0,
                lon: 7.0,
                freq_hz: 100e6
            }
            .valid()
        );
    }

    #[test]
    fn a_bearing_report_needs_a_station_and_a_place() {
        let good = BearingReport {
            station_id: "north".to_owned(),
            lat: 51.0,
            lon: 7.0,
            bearing_deg: 137.0,
            confidence: 0.8,
            time: None,
        };
        assert!(good.valid());
        assert!(
            !BearingReport {
                station_id: String::new(),
                ..good.clone()
            }
            .valid()
        );
        assert!(
            !BearingReport {
                confidence: 2.0,
                ..good
            }
            .valid()
        );
    }

    #[test]
    fn a_reading_round_trips_through_json() {
        let reading = DfReading {
            bearing_deg: 137.5,
            confidence: 0.62,
            peak_to_floor_db: 12.5,
            pseudospectrum: vec![7; DF_SPECTRUM_POINTS],
        };
        let json = serde_json::to_string(&reading).expect("serialize");
        let back: DfReading = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, reading);
    }
}
