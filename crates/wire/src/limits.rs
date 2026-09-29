use serde::Serialize;

use crate::{
    array::{
        DEFAULT_CAL_BANDWIDTH_HZ, LIGHT_SPEED_M_S, MAX_ARRAY_EXTENT_M, MAX_ARRAY_GAIN_DB,
        MAX_ARRAY_LANES, MAX_CAL_BANDWIDTH_HZ, MAX_CAL_OFFSET_HZ, MIN_ARRAY_GAIN_DB,
        MIN_ARRAY_LANES, MIN_CAL_BANDWIDTH_HZ,
    },
    fusion::{
        DEFAULT_FUSION_HALF_LIFE_S, FUSION_FIXED_HALF_LIFE_S, FUSION_MOVING_HALF_LIFE_S,
        MAX_FUSION_EMITTERS, MAX_FUSION_HALF_LIFE_S, MIN_FUSION_HALF_LIFE_S,
    },
    processor::{
        DEFAULT_BAND_HZ, MIN_BAND_HZ,
        beamformer::{MAX_BEAM_NULLS, MAX_BEAM_TAPS},
        df::{
            MAX_DF_BANDWIDTH_HZ, MAX_DF_OFFSET_HZ, MAX_DF_PEAKS, MAX_DF_REPORT_MS,
            MAX_DF_SMOOTHING, MAX_DF_SOURCES, MAX_STATION_ID_LEN, MIN_DF_BANDWIDTH_HZ,
            MIN_DF_REPORT_MS,
        },
        stitch::STITCH_REPORT_MS,
    },
    radar::{
        DEFAULT_CMA_STEP, DEFAULT_CMA_TAPS, DEFAULT_CUSTOM_BANDWIDTH_HZ, DEFAULT_OS_RANK,
        DVBT_BANDWIDTH_HZ, MAX_CFAR_GUARD, MAX_CFAR_TRAIN_DOPPLER, MAX_CFAR_TRAIN_RANGE,
        MAX_CMA_TAPS, MAX_ECA_DOPPLER_TAPS, MAX_ECA_LEAD, MAX_RADAR_ALTITUDE_M, MAX_TRACK_WINDOW,
        RADAR_MAX_BANDWIDTH_HZ, RADAR_MAX_CPI_MS, RADAR_MAX_OFFSET_HZ, RADAR_MAX_OVERLAP,
        RADAR_MAX_RANGE_KM, RADAR_MAX_SPEED_MPS, RADAR_MIN_BANDWIDTH_HZ, RADAR_MIN_CPI_MS,
        RADAR_MIN_SPEED_MPS, SURFACE_DB_MAX, SURFACE_DB_MIN,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Bounds<T> {
    pub min: T,
    pub max: T,
    #[serde(skip_serializing_if = "includes_min")]
    pub above: bool,
}

const fn includes_min(above: &bool) -> bool {
    !*above
}

impl<T: Copy + PartialOrd> Bounds<T> {
    #[must_use]
    pub const fn new(min: T, max: T) -> Self {
        Self {
            min,
            max,
            above: false,
        }
    }

    #[must_use]
    pub const fn above(min: T, max: T) -> Self {
        Self {
            min,
            max,
            above: true,
        }
    }

    #[must_use]
    pub fn contains(&self, value: T) -> bool {
        let low = if self.above {
            self.min < value
        } else {
            self.min <= value
        };
        low && value <= self.max
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Limits {
    pub light_speed_m_s: f64,
    pub band_seed_hz: f64,
    pub array: ArrayLimits,
    pub fusion: FusionLimits,
    pub radar: RadarLimits,
    pub stitch: StitchLimits,
    pub df: DfLimits,
    pub beamformer: BeamformerLimits,
    pub spatial: SpatialLimits,
    pub correlator: CorrelatorLimits,
    pub polarimeter: PolarimeterLimits,
    pub hunt: HuntLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ArrayLimits {
    pub lanes: Bounds<u32>,
    pub extent_m: f64,
    pub gain_db: Bounds<f64>,
    pub cal_offset_hz: Bounds<f64>,
    pub cal_bandwidth_hz: Bounds<f64>,
    pub cal_bandwidth_seed_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FusionLimits {
    pub fixed_half_life_s: u32,
    pub moving_half_life_s: u32,
    pub half_life_s: Bounds<u32>,
    pub half_life_seed_s: u32,
    pub extent_km: Bounds<f64>,
    pub probe_km: Bounds<f64>,
    pub min_confidence: Bounds<f32>,
    pub emitters: Bounds<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RadarLimits {
    pub surface_db: Bounds<f32>,
    pub offset_hz: Bounds<f64>,
    pub bandwidth_hz: Bounds<f64>,
    pub max_range_km: Bounds<f32>,
    pub max_speed_mps: Bounds<f32>,
    pub cpi_ms: Bounds<u32>,
    pub overlap: Bounds<f32>,
    pub reach_km: Bounds<f32>,
    pub lead: Bounds<u32>,
    pub doppler_taps: Bounds<u32>,
    pub batch_ms: Bounds<f32>,
    pub extension_ms: Bounds<f32>,
    pub clutter_step: Bounds<f32>,
    pub loading: Bounds<f32>,
    pub cma_taps: Bounds<u32>,
    pub cma_step: Bounds<f32>,
    pub os_rank: Bounds<f32>,
    pub pfa: Bounds<f64>,
    pub guard: Bounds<u32>,
    pub train_range: Bounds<u32>,
    pub train_doppler: Bounds<u32>,
    pub min_doppler_hz: Bounds<f32>,
    pub min_range_km: Bounds<f32>,
    pub min_snr_db: Bounds<f32>,
    pub track_window: Bounds<u32>,
    pub coast_looks: Bounds<u32>,
    pub max_accel_mps2: Bounds<f32>,
    pub gate: Bounds<f32>,
    pub jerk: Bounds<f32>,
    pub altitude_m: Bounds<f32>,
    pub seed: RadarSeeds,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RadarSeeds {
    pub dvbt_bandwidth_hz: f64,
    pub custom_bandwidth_hz: f64,
    pub cma_taps: u32,
    pub cma_step: f32,
    pub os_rank: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StitchLimits {
    pub report_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BandLimits {
    pub offset_hz: Bounds<f64>,
    pub bandwidth_hz: Bounds<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DfLimits {
    pub sources: Bounds<u32>,
    pub peaks: Bounds<u8>,
    pub smoothing: Bounds<u8>,
    pub band: BandLimits,
    pub report_ms: Bounds<u32>,
    pub carry_over: Bounds<f32>,
    pub squelch_db: Bounds<f32>,
    pub loading: Bounds<f32>,
    pub azimuth_step_deg: Bounds<f64>,
    pub yaw_gate_dps: Bounds<f32>,
    pub station_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BeamformerLimits {
    pub nulls: usize,
    pub elevation_deg: Bounds<f64>,
    pub taps: Bounds<u32>,
    pub step: Bounds<f32>,
    pub forget: Bounds<f32>,
    pub crossfade_ms: Bounds<u32>,
    pub update_ms: Bounds<u32>,
    pub carry_over: Bounds<f32>,
    pub loading: Bounds<f32>,
    pub band: BandLimits,
    pub steer_timeout_ms: Bounds<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpatialLimits {
    pub bins: Bounds<u32>,
    pub columns: Bounds<u32>,
    pub average_ms: Bounds<u32>,
    pub report_ms: Bounds<u32>,
    pub azimuth_step_deg: Bounds<f64>,
    pub span_db: Bounds<f32>,
    pub band: BandLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CorrelatorLimits {
    pub bins: Bounds<u32>,
    pub channels: Bounds<u32>,
    pub integrate_s: Bounds<f32>,
    pub band: BandLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PolarimeterLimits {
    pub band: BandLimits,
    pub report_ms: Bounds<u32>,
    pub average_ms: Bounds<u32>,
    pub crossfade_ms: Bounds<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct HuntLimits {
    pub beamwidth_deg: Bounds<f64>,
    pub front_back_db: Bounds<f64>,
    pub min_span_deg: Bounds<f64>,
    pub min_contrast_db: Bounds<f32>,
    pub mount_offset_deg: Bounds<f64>,
}

pub const ARRAY_LIMITS: ArrayLimits = ArrayLimits {
    lanes: Bounds::new(MIN_ARRAY_LANES, MAX_ARRAY_LANES),
    extent_m: MAX_ARRAY_EXTENT_M,
    gain_db: Bounds::new(MIN_ARRAY_GAIN_DB, MAX_ARRAY_GAIN_DB),
    cal_offset_hz: Bounds::new(-MAX_CAL_OFFSET_HZ, MAX_CAL_OFFSET_HZ),
    cal_bandwidth_hz: Bounds::new(MIN_CAL_BANDWIDTH_HZ, MAX_CAL_BANDWIDTH_HZ),
    cal_bandwidth_seed_hz: DEFAULT_CAL_BANDWIDTH_HZ,
};

pub const FUSION_LIMITS: FusionLimits = FusionLimits {
    fixed_half_life_s: FUSION_FIXED_HALF_LIFE_S,
    moving_half_life_s: FUSION_MOVING_HALF_LIFE_S,
    half_life_s: Bounds::new(MIN_FUSION_HALF_LIFE_S, MAX_FUSION_HALF_LIFE_S),
    half_life_seed_s: DEFAULT_FUSION_HALF_LIFE_S,
    extent_km: Bounds::new(1.0, 100.0),
    probe_km: Bounds::new(0.5, 50.0),
    min_confidence: Bounds::new(0.0, 1.0),
    emitters: Bounds::new(1, MAX_FUSION_EMITTERS),
};

pub const RADAR_LIMITS: RadarLimits = RadarLimits {
    surface_db: Bounds::new(SURFACE_DB_MIN, SURFACE_DB_MAX),
    offset_hz: Bounds::new(-RADAR_MAX_OFFSET_HZ, RADAR_MAX_OFFSET_HZ),
    bandwidth_hz: Bounds::new(RADAR_MIN_BANDWIDTH_HZ, RADAR_MAX_BANDWIDTH_HZ),
    max_range_km: Bounds::above(0.0, RADAR_MAX_RANGE_KM),
    max_speed_mps: Bounds::new(RADAR_MIN_SPEED_MPS, RADAR_MAX_SPEED_MPS),
    cpi_ms: Bounds::new(RADAR_MIN_CPI_MS, RADAR_MAX_CPI_MS),
    overlap: Bounds::new(0.0, RADAR_MAX_OVERLAP),
    reach_km: Bounds::new(0.1, 100.0),
    lead: Bounds::new(0, MAX_ECA_LEAD),
    doppler_taps: Bounds::new(0, MAX_ECA_DOPPLER_TAPS),
    batch_ms: Bounds::new(1.0, 1_000.0),
    extension_ms: Bounds::new(0.0, 500.0),
    clutter_step: Bounds::above(0.0, 1.0),
    loading: Bounds::new(0.0, 1.0),
    cma_taps: Bounds::new(1, MAX_CMA_TAPS),
    cma_step: Bounds::above(0.0, 0.1),
    os_rank: Bounds::new(0.5, 0.95),
    pfa: Bounds::new(1e-9, 1e-2),
    guard: Bounds::new(0, MAX_CFAR_GUARD),
    train_range: Bounds::new(1, MAX_CFAR_TRAIN_RANGE),
    train_doppler: Bounds::new(1, MAX_CFAR_TRAIN_DOPPLER),
    min_doppler_hz: Bounds::new(0.0, 100.0),
    min_range_km: Bounds::new(0.0, 50.0),
    min_snr_db: Bounds::new(0.0, 40.0),
    track_window: Bounds::new(1, MAX_TRACK_WINDOW),
    coast_looks: Bounds::new(0, 100),
    max_accel_mps2: Bounds::new(1.0, 200.0),
    gate: Bounds::new(4.0, 30.0),
    jerk: Bounds::above(0.0, 1_000.0),
    altitude_m: Bounds::new(0.0, MAX_RADAR_ALTITUDE_M),
    seed: RadarSeeds {
        dvbt_bandwidth_hz: DVBT_BANDWIDTH_HZ,
        custom_bandwidth_hz: DEFAULT_CUSTOM_BANDWIDTH_HZ,
        cma_taps: DEFAULT_CMA_TAPS,
        cma_step: DEFAULT_CMA_STEP,
        os_rank: DEFAULT_OS_RANK,
    },
};

const OFFSET_HZ: Bounds<f64> = Bounds::new(-MAX_DF_OFFSET_HZ, MAX_DF_OFFSET_HZ);

const NARROW_BAND: BandLimits = BandLimits {
    offset_hz: OFFSET_HZ,
    bandwidth_hz: Bounds::new(MIN_DF_BANDWIDTH_HZ, MAX_DF_BANDWIDTH_HZ),
};

const WIDE_BAND: BandLimits = BandLimits {
    offset_hz: OFFSET_HZ,
    bandwidth_hz: Bounds::new(MIN_BAND_HZ, MAX_DF_BANDWIDTH_HZ),
};

pub const DF_LIMITS: DfLimits = DfLimits {
    sources: Bounds::new(1, MAX_DF_SOURCES),
    peaks: Bounds::new(1, MAX_DF_PEAKS),
    smoothing: Bounds::new(0, MAX_DF_SMOOTHING),
    band: NARROW_BAND,
    report_ms: Bounds::new(MIN_DF_REPORT_MS, MAX_DF_REPORT_MS),
    carry_over: Bounds::new(0.0, 0.99),
    squelch_db: Bounds::new(0.0, 40.0),
    loading: Bounds::new(0.0, 1.0),
    azimuth_step_deg: Bounds::new(0.25, 10.0),
    yaw_gate_dps: Bounds::new(1.0, 360.0),
    station_len: MAX_STATION_ID_LEN,
};

pub const BEAMFORMER_LIMITS: BeamformerLimits = BeamformerLimits {
    nulls: MAX_BEAM_NULLS,
    elevation_deg: Bounds::new(0.0, 90.0),
    taps: Bounds::new(1, MAX_BEAM_TAPS),
    step: Bounds::new(1e-5, 1.9),
    forget: Bounds::new(0.9, 0.99999),
    crossfade_ms: Bounds::new(0, 500),
    update_ms: Bounds::new(50, 10_000),
    carry_over: Bounds::new(0.0, 0.99),
    loading: Bounds::new(0.0, 10.0),
    band: WIDE_BAND,
    steer_timeout_ms: Bounds::new(100, 60_000),
};

pub const SPATIAL_LIMITS: SpatialLimits = SpatialLimits {
    bins: Bounds::new(256, 4_096),
    columns: Bounds::new(64, 1_024),
    average_ms: Bounds::new(50, 10_000),
    report_ms: Bounds::new(50, 2_000),
    azimuth_step_deg: Bounds::new(1.0, 10.0),
    span_db: Bounds::new(10.0, 80.0),
    band: WIDE_BAND,
};

pub const CORRELATOR_LIMITS: CorrelatorLimits = CorrelatorLimits {
    bins: Bounds::new(64, 8_192),
    channels: Bounds::new(16, 1_024),
    integrate_s: Bounds::new(0.05, 600.0),
    band: WIDE_BAND,
};

pub const POLARIMETER_LIMITS: PolarimeterLimits = PolarimeterLimits {
    band: NARROW_BAND,
    report_ms: Bounds::new(50, 10_000),
    average_ms: Bounds::new(50, 10_000),
    crossfade_ms: Bounds::new(0, 500),
};

pub const HUNT_LIMITS: HuntLimits = HuntLimits {
    beamwidth_deg: Bounds::new(10.0, 180.0),
    front_back_db: Bounds::new(0.0, 40.0),
    min_span_deg: Bounds::new(60.0, 720.0),
    min_contrast_db: Bounds::new(1.0, 40.0),
    mount_offset_deg: Bounds::new(-180.0, 180.0),
};

pub const LIMITS: Limits = Limits {
    light_speed_m_s: LIGHT_SPEED_M_S,
    band_seed_hz: DEFAULT_BAND_HZ,
    array: ARRAY_LIMITS,
    fusion: FUSION_LIMITS,
    radar: RADAR_LIMITS,
    stitch: StitchLimits {
        report_ms: STITCH_REPORT_MS,
    },
    df: DF_LIMITS,
    beamformer: BEAMFORMER_LIMITS,
    spatial: SPATIAL_LIMITS,
    correlator: CORRELATOR_LIMITS,
    polarimeter: POLARIMETER_LIMITS,
    hunt: HUNT_LIMITS,
};

pub fn generated() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&LIMITS)
}

#[cfg(test)]
mod tests;
