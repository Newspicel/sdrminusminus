use sdrmm_device::DeviceError;
use sdrmm_wire::{
    ArgumentOption, Capabilities, DcArtifact, ExtraSetting, GainStage, Range, StreamScope,
};

use crate::{
    ffi,
    model::{DuoMode, Model},
};

pub const IF_GAIN_STAGE: &str = "IF";
pub const RF_GAIN_STAGE: &str = "RF";

pub const MIN_GR_DB: i32 = ffi::NORMAL_MIN_GR;
pub const MAX_GR_DB: i32 = ffi::MAX_BB_GR;
pub const IF_GAIN_SPAN_DB: f64 = (MAX_GR_DB - MIN_GR_DB) as f64;

pub const MIN_ADC_RATE_HZ: f64 = 2_000_000.0;
pub const MAX_ADC_RATE_HZ: f64 = 10_660_000.0;
pub const MAX_DECIMATION: u8 = 32;
pub const DUO_LOW_IF_RATE_HZ: f64 = 2_000_000.0;

pub const ANTENNA_A: &str = "Antenna A";
pub const ANTENNA_B: &str = "Antenna B";
pub const ANTENNA_C: &str = "Antenna C";
pub const ANTENNA_HI_Z: &str = "Hi-Z";
pub const ANTENNA_50_OHM: &str = "50 Ohm";

pub const EXTRA_AGC: &str = "agc";
pub const EXTRA_AGC_SETPOINT: &str = "agc_setpoint_dbfs";
pub const EXTRA_BIAS_T: &str = "bias_t";
pub const EXTRA_RF_NOTCH: &str = "rf_notch";
pub const EXTRA_DAB_NOTCH: &str = "dab_notch";
pub const EXTRA_AM_NOTCH: &str = "am_notch";
pub const EXTRA_EXT_REF: &str = "ext_ref_out";
pub const EXTRA_HDR: &str = "hdr";
pub const EXTRA_HDR_BW: &str = "hdr_bandwidth";
pub const EXTRA_DC_CORRECTION: &str = "dc_correction";
pub const EXTRA_IQ_BALANCE: &str = "iq_balance";

pub const AGC_OFF: &str = "off";
pub const AGC_5HZ: &str = "5 Hz";
pub const AGC_50HZ: &str = "50 Hz";
pub const AGC_100HZ: &str = "100 Hz";

const RSP1_0_420: [u8; 4] = [0, 24, 19, 43];
const RSP1_420_1000: [u8; 4] = [0, 7, 19, 26];
const RSP1_1000_2000: [u8; 4] = [0, 5, 19, 24];

const RSP1A_AM: [u8; 7] = [0, 6, 12, 18, 37, 42, 61];
const RSP1A_VHF: [u8; 10] = [0, 6, 12, 18, 20, 26, 32, 38, 57, 62];
const RSP1A_420_1000: [u8; 10] = [0, 7, 13, 19, 20, 27, 33, 39, 45, 64];
const RSP1A_LBAND: [u8; 9] = [0, 6, 12, 20, 26, 32, 38, 43, 62];

const RSP2_0_420: [u8; 9] = [0, 10, 15, 21, 24, 34, 39, 45, 64];
const RSP2_420_1000: [u8; 6] = [0, 7, 10, 17, 22, 41];
const RSP2_1000_2000: [u8; 6] = [0, 5, 21, 15, 15, 34];
const RSP2_HI_Z: [u8; 5] = [0, 6, 12, 18, 37];

const RSPDUO_HI_Z: [u8; 5] = [0, 6, 12, 18, 37];

const RSPDX_HDR: [u8; 22] = [
    0, 3, 6, 9, 12, 15, 18, 21, 24, 25, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60,
];
const RSPDX_0_12: [u8; 19] = [
    0, 3, 6, 9, 12, 15, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60,
];
const RSPDX_12_50: [u8; 20] = [
    0, 3, 6, 9, 12, 15, 18, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60,
];
const RSPDX_50_60: [u8; 25] = [
    0, 3, 6, 9, 12, 20, 23, 26, 29, 32, 35, 38, 44, 47, 50, 53, 56, 59, 62, 65, 68, 71, 74, 77, 80,
];
const RSPDX_60_250: [u8; 27] = [
    0, 3, 6, 9, 12, 15, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60, 63, 66, 69, 72, 75, 78,
    81, 84,
];
const RSPDX_250_420: [u8; 28] = [
    0, 3, 6, 9, 12, 15, 18, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60, 63, 66, 69, 72, 75,
    78, 81, 84,
];
const RSPDX_420_1000: [u8; 21] = [
    0, 7, 10, 13, 16, 19, 22, 25, 31, 34, 37, 40, 43, 46, 49, 52, 55, 58, 61, 64, 67,
];
const RSPDX_1000_2000: [u8; 19] = [
    0, 5, 8, 11, 14, 17, 20, 32, 35, 38, 41, 44, 47, 50, 53, 56, 59, 62, 65,
];

#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub model: Model,
    pub center_hz: f64,
    pub hi_z: bool,
    pub hdr: bool,
}

fn rsp1a_family(mhz: f64, am_limit: f64) -> &'static [u8] {
    if mhz < am_limit {
        &RSP1A_AM
    } else if mhz < 420.0 {
        &RSP1A_VHF
    } else if mhz < 1000.0 {
        &RSP1A_420_1000
    } else {
        &RSP1A_LBAND
    }
}

fn rspdx_family(mhz: f64, hdr: bool) -> &'static [u8] {
    if hdr && mhz < 2.0 {
        &RSPDX_HDR
    } else if mhz < 12.0 {
        &RSPDX_0_12
    } else if mhz < 50.0 {
        &RSPDX_12_50
    } else if mhz < 60.0 {
        &RSPDX_50_60
    } else if mhz < 250.0 {
        &RSPDX_60_250
    } else if mhz < 420.0 {
        &RSPDX_250_420
    } else if mhz < 1000.0 {
        &RSPDX_420_1000
    } else {
        &RSPDX_1000_2000
    }
}

#[must_use]
pub fn lna_reductions(band: Band) -> &'static [u8] {
    let mhz = band.center_hz / 1e6;
    match band.model {
        Model::Rsp1 => {
            if mhz < 420.0 {
                &RSP1_0_420
            } else if mhz < 1000.0 {
                &RSP1_420_1000
            } else {
                &RSP1_1000_2000
            }
        }
        Model::Rsp1a => rsp1a_family(mhz, 60.0),
        Model::Rsp1b => rsp1a_family(mhz, 50.0),
        Model::Rsp2 => {
            if band.hi_z && mhz < 60.0 {
                &RSP2_HI_Z
            } else if mhz < 420.0 {
                &RSP2_0_420
            } else if mhz < 1000.0 {
                &RSP2_420_1000
            } else {
                &RSP2_1000_2000
            }
        }
        Model::RspDuo => {
            if band.hi_z && mhz < 60.0 {
                &RSPDUO_HI_Z
            } else {
                rsp1a_family(mhz, 60.0)
            }
        }
        Model::RspDx | Model::RspDxR2 => rspdx_family(mhz, band.hdr),
    }
}

#[must_use]
pub fn max_lna_reduction(band: Band) -> f64 {
    f64::from(
        lna_reductions(band)
            .iter()
            .copied()
            .max()
            .unwrap_or_default(),
    )
}

#[must_use]
pub fn rf_gain_db(band: Band, state: u8) -> f64 {
    let reductions = lna_reductions(band);
    let reduction = reductions
        .get(state as usize)
        .copied()
        .unwrap_or_else(|| reductions.last().copied().unwrap_or_default());
    max_lna_reduction(band) - f64::from(reduction)
}

#[must_use]
pub fn lna_state_for_gain(band: Band, gain_db: f64) -> u8 {
    let max = max_lna_reduction(band);
    let wanted = max - gain_db.clamp(0.0, max);
    let mut best = 0_u8;
    let mut best_distance = f64::INFINITY;
    for (state, reduction) in lna_reductions(band).iter().enumerate() {
        let distance = (f64::from(*reduction) - wanted).abs();
        if distance < best_distance {
            best_distance = distance;
            best = state as u8;
        }
    }
    best
}

#[must_use]
pub fn if_gain_db(gr_db: i32) -> f64 {
    f64::from(MAX_GR_DB - gr_db.clamp(MIN_GR_DB, MAX_GR_DB))
}

#[must_use]
pub fn gr_db_for_gain(gain_db: f64) -> i32 {
    let gain = gain_db.clamp(0.0, IF_GAIN_SPAN_DB);
    MAX_GR_DB - (gain.round() as i32)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RatePlan {
    pub adc_hz: f64,
    pub decimation: u8,
    pub output_hz: f64,
    pub if_khz: i32,
}

fn decimation_for(requested: f64, base: f64) -> Option<u8> {
    let mut factor = 1_u8;
    while factor <= MAX_DECIMATION {
        if (base / f64::from(factor) - requested).abs() <= requested * 1e-6 {
            return Some(factor);
        }
        factor = factor.checked_mul(2)?;
    }
    None
}

pub fn plan_rate(mode: Option<DuoMode>, requested: f64) -> Result<RatePlan, DeviceError> {
    if !requested.is_finite() || requested <= 0.0 {
        return Err(DeviceError::Unsupported(format!(
            "sample rate {requested} is not a rate"
        )));
    }
    if mode.is_some_and(DuoMode::is_low_if) {
        let decimation = decimation_for(requested, DUO_LOW_IF_RATE_HZ).ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "this RSPduo mode runs the ADC at a fixed rate, so it offers {} down to {} Hz \
                 halving each step, not {requested} Hz",
                DUO_LOW_IF_RATE_HZ,
                DUO_LOW_IF_RATE_HZ / f64::from(MAX_DECIMATION)
            ))
        })?;
        return Ok(RatePlan {
            adc_hz: crate::model::DUO_DUAL_TUNER_FS_HZ,
            decimation,
            output_hz: DUO_LOW_IF_RATE_HZ / f64::from(decimation),
            if_khz: ffi::IF_1_620,
        });
    }
    if requested > MAX_ADC_RATE_HZ {
        return Err(DeviceError::Unsupported(format!(
            "sample rate {requested} Hz is above the {MAX_ADC_RATE_HZ} Hz this receiver samples at"
        )));
    }
    let mut decimation = 1_u8;
    while f64::from(decimation) * requested < MIN_ADC_RATE_HZ {
        decimation = decimation.checked_mul(2).ok_or_else(|| {
            DeviceError::Unsupported(format!("sample rate {requested} Hz is too low"))
        })?;
        if decimation > MAX_DECIMATION {
            return Err(DeviceError::Unsupported(format!(
                "sample rate {requested} Hz is below the {} Hz this receiver reaches by \
                 decimating its {MIN_ADC_RATE_HZ} Hz minimum",
                MIN_ADC_RATE_HZ / f64::from(MAX_DECIMATION)
            )));
        }
    }
    Ok(RatePlan {
        adc_hz: requested * f64::from(decimation),
        decimation,
        output_hz: requested,
        if_khz: ffi::IF_ZERO,
    })
}

const BANDWIDTHS_KHZ: [i32; 8] = [
    ffi::BW_0_200,
    ffi::BW_0_300,
    ffi::BW_0_600,
    ffi::BW_1_536,
    ffi::BW_5_000,
    ffi::BW_6_000,
    ffi::BW_7_000,
    ffi::BW_8_000,
];

#[must_use]
pub fn bandwidths(mode: Option<DuoMode>) -> Vec<f64> {
    let limit = if mode.is_some_and(DuoMode::is_low_if) {
        ffi::BW_1_536
    } else {
        ffi::BW_8_000
    };
    BANDWIDTHS_KHZ
        .iter()
        .filter(|khz| **khz <= limit)
        .map(|khz| f64::from(*khz) * 1000.0)
        .collect()
}

#[must_use]
pub fn bandwidth_khz(bandwidth_hz: f64) -> i32 {
    let wanted = bandwidth_hz / 1000.0;
    let mut chosen = BANDWIDTHS_KHZ[0];
    for candidate in BANDWIDTHS_KHZ {
        if f64::from(candidate) <= wanted {
            chosen = candidate;
        }
    }
    chosen
}

#[must_use]
pub fn default_bandwidth_khz(output_hz: f64, mode: Option<DuoMode>) -> i32 {
    let limit = if mode.is_some_and(DuoMode::is_low_if) {
        ffi::BW_1_536
    } else {
        ffi::BW_8_000
    };
    bandwidth_khz(output_hz).min(limit)
}

const SINGLE_TUNER_RATES: [f64; 16] = [
    62_500.0,
    96_000.0,
    125_000.0,
    192_000.0,
    250_000.0,
    384_000.0,
    500_000.0,
    768_000.0,
    1_000_000.0,
    1_536_000.0,
    2_000_000.0,
    2_048_000.0,
    3_000_000.0,
    4_000_000.0,
    6_000_000.0,
    8_000_000.0,
];

#[must_use]
pub fn sample_rates(mode: Option<DuoMode>) -> Vec<f64> {
    if mode.is_some_and(DuoMode::is_low_if) {
        let mut rates = Vec::new();
        let mut factor = 1_u8;
        while factor <= MAX_DECIMATION {
            rates.push(DUO_LOW_IF_RATE_HZ / f64::from(factor));
            factor *= 2;
        }
        rates.reverse();
        return rates;
    }
    SINGLE_TUNER_RATES.to_vec()
}

#[must_use]
pub fn frequency_range(model: Model) -> Range {
    Range {
        min: if model == Model::Rsp1 {
            10_000.0
        } else {
            1_000.0
        },
        max: 2_000_000_000.0,
        step: None,
    }
}

#[must_use]
pub fn antennas(model: Model, mode: Option<DuoMode>) -> Vec<String> {
    match model {
        Model::Rsp1 | Model::Rsp1a | Model::Rsp1b => Vec::new(),
        Model::Rsp2 => vec![
            ANTENNA_A.to_string(),
            ANTENNA_B.to_string(),
            ANTENNA_HI_Z.to_string(),
        ],
        Model::RspDuo => match mode {
            Some(DuoMode::SingleTunerA | DuoMode::MasterA) => {
                vec![ANTENNA_50_OHM.to_string(), ANTENNA_HI_Z.to_string()]
            }
            _ => Vec::new(),
        },
        Model::RspDx | Model::RspDxR2 => vec![
            ANTENNA_A.to_string(),
            ANTENNA_B.to_string(),
            ANTENNA_C.to_string(),
        ],
    }
}

fn agc_setting() -> ExtraSetting {
    ExtraSetting::Enum {
        name: EXTRA_AGC.to_string(),
        options: vec![
            ArgumentOption::plain(AGC_OFF),
            ArgumentOption::plain(AGC_5HZ),
            ArgumentOption::plain(AGC_50HZ),
            ArgumentOption::plain(AGC_100HZ),
        ],
        default: AGC_50HZ.to_string(),
    }
}

fn boolean(name: &str, default: bool) -> ExtraSetting {
    ExtraSetting::Bool {
        name: name.to_string(),
        default,
    }
}

#[must_use]
pub fn extras(model: Model, mode: Option<DuoMode>) -> Vec<ExtraSetting> {
    let mut extras = vec![
        agc_setting(),
        ExtraSetting::Range {
            name: EXTRA_AGC_SETPOINT.to_string(),
            range: Range {
                min: -72.0,
                max: -20.0,
                step: Some(1.0),
            },
            unit: "dBFS".to_string(),
        },
        boolean(EXTRA_DC_CORRECTION, true),
        boolean(EXTRA_IQ_BALANCE, true),
    ];
    if model.has_bias_t() {
        extras.push(boolean(EXTRA_BIAS_T, false));
    }
    if model.has_rf_notch() {
        extras.push(boolean(EXTRA_RF_NOTCH, false));
    }
    if model.has_dab_notch() {
        extras.push(boolean(EXTRA_DAB_NOTCH, false));
    }
    if model == Model::RspDuo && matches!(mode, Some(DuoMode::SingleTunerA | DuoMode::MasterA)) {
        extras.push(boolean(EXTRA_AM_NOTCH, false));
    }
    if model.has_ext_ref() {
        extras.push(boolean(EXTRA_EXT_REF, false));
    }
    if model.has_hdr() {
        extras.push(boolean(EXTRA_HDR, false));
        extras.push(ExtraSetting::Enum {
            name: EXTRA_HDR_BW.to_string(),
            options: vec![
                ArgumentOption::plain("200 kHz"),
                ArgumentOption::plain("500 kHz"),
                ArgumentOption::plain("1.2 MHz"),
                ArgumentOption::plain("1.7 MHz"),
            ],
            default: "1.7 MHz".to_string(),
        });
    }
    extras
}

#[must_use]
pub fn capabilities(model: Model, mode: Option<DuoMode>, band: Band) -> Capabilities {
    let streams = mode.map_or(1, DuoMode::streams);
    let rates = sample_rates(mode);
    let sample_rate_ranges = if mode.is_some_and(DuoMode::is_low_if) {
        Vec::new()
    } else {
        vec![Range {
            min: MIN_ADC_RATE_HZ / f64::from(MAX_DECIMATION),
            max: MAX_ADC_RATE_HZ,
            step: None,
        }]
    };
    Capabilities {
        freq_ranges: vec![frequency_range(model)],
        sample_rates: rates,
        sample_rate_ranges,
        gains: vec![
            GainStage {
                name: RF_GAIN_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: max_lna_reduction(band),
                    step: None,
                },
                values: Vec::new(),
            },
            GainStage {
                name: IF_GAIN_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: IF_GAIN_SPAN_DB,
                    step: Some(1.0),
                },
                values: Vec::new(),
            },
        ],
        antennas: antennas(model, mode),
        bandwidths: bandwidths(mode),
        bandwidth_ranges: Vec::new(),
        extra: extras(model, mode),
        ppm: true,
        duplex: sdrmm_wire::Duplex::RxOnly,
        rx_streams: streams,
        tx_streams: 0,
        per_stream: if streams > 1 {
            StreamScope {
                tuning: true,
                gain: true,
                antenna: false,
            }
        } else {
            StreamScope::default()
        },
        directional: None,
        dc_artifact: DcArtifact::Operator,
        hardware_sweep: false,
        coherence: sdrmm_wire::Coherence::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(model: Model, center_hz: f64) -> Band {
        Band {
            model,
            center_hz,
            hi_z: false,
            hdr: false,
        }
    }

    #[test]
    fn lna_tables_match_the_state_counts_the_api_documents() {
        assert_eq!(lna_reductions(band(Model::Rsp1a, 10e6)).len(), 7);
        assert_eq!(lna_reductions(band(Model::Rsp1a, 100e6)).len(), 10);
        assert_eq!(lna_reductions(band(Model::Rsp1a, 1500e6)).len(), 9);
        assert_eq!(lna_reductions(band(Model::Rsp2, 100e6)).len(), 9);
        assert_eq!(lna_reductions(band(Model::Rsp2, 500e6)).len(), 6);
        assert_eq!(lna_reductions(band(Model::RspDx, 300e6)).len(), 28);
        assert_eq!(lna_reductions(band(Model::RspDx, 100e6)).len(), 27);
        assert_eq!(lna_reductions(band(Model::RspDx, 500e6)).len(), 21);
        assert_eq!(lna_reductions(band(Model::RspDx, 1500e6)).len(), 19);
        assert_eq!(lna_reductions(band(Model::RspDxR2, 5e6)).len(), 19);
        assert_eq!(lna_reductions(band(Model::RspDxR2, 20e6)).len(), 20);
        assert_eq!(lna_reductions(band(Model::RspDxR2, 55e6)).len(), 25);
    }

    #[test]
    fn the_hi_z_port_has_its_own_table_below_sixty_megahertz() {
        let hi_z = Band {
            hi_z: true,
            ..band(Model::Rsp2, 5e6)
        };
        assert_eq!(lna_reductions(hi_z).len(), 5);
        assert_eq!(lna_reductions(band(Model::Rsp2, 5e6)).len(), 9);
    }

    #[test]
    fn hdr_mode_has_its_own_table_below_two_megahertz() {
        let hdr = Band {
            hdr: true,
            ..band(Model::RspDx, 500e3)
        };
        assert_eq!(lna_reductions(hdr).len(), 22);
        assert_eq!(lna_reductions(band(Model::RspDx, 500e3)).len(), 19);
    }

    #[test]
    fn state_zero_is_the_most_rf_gain_and_the_last_state_is_the_least() {
        let band = band(Model::Rsp1a, 100e6);
        assert_eq!(rf_gain_db(band, 0), 62.0);
        assert_eq!(rf_gain_db(band, 9), 0.0);
    }

    #[test]
    fn an_rf_gain_request_snaps_to_the_nearest_state() {
        let band = band(Model::Rsp1a, 100e6);
        assert_eq!(lna_state_for_gain(band, 62.0), 0);
        assert_eq!(lna_state_for_gain(band, 0.0), 9);
        assert_eq!(lna_state_for_gain(band, 56.0), 1);
    }

    #[test]
    fn an_out_of_range_rf_gain_is_clamped_to_the_table() {
        let band = band(Model::Rsp1a, 100e6);
        assert_eq!(lna_state_for_gain(band, 1000.0), 0);
        assert_eq!(lna_state_for_gain(band, -10.0), 9);
    }

    #[test]
    fn a_state_beyond_the_table_reports_the_last_entry() {
        let band = band(Model::Rsp1, 100e6);
        assert_eq!(rf_gain_db(band, 200), rf_gain_db(band, 3));
    }

    #[test]
    fn if_gain_is_the_inverse_of_the_reduction_the_api_takes() {
        assert_eq!(if_gain_db(59), 0.0);
        assert_eq!(if_gain_db(20), 39.0);
        assert_eq!(gr_db_for_gain(0.0), 59);
        assert_eq!(gr_db_for_gain(39.0), 20);
        assert_eq!(gr_db_for_gain(1000.0), 20);
        assert_eq!(gr_db_for_gain(-5.0), 59);
    }

    #[test]
    fn rates_at_or_above_the_adc_minimum_run_undecimated() {
        let plan = plan_rate(None, 2_048_000.0).expect("plan");
        assert_eq!(
            plan,
            RatePlan {
                adc_hz: 2_048_000.0,
                decimation: 1,
                output_hz: 2_048_000.0,
                if_khz: ffi::IF_ZERO,
            }
        );
    }

    #[test]
    fn low_rates_decimate_from_a_legal_adc_rate() {
        let plan = plan_rate(None, 250_000.0).expect("plan");
        assert_eq!(plan.decimation, 8);
        assert_eq!(plan.adc_hz, 2_000_000.0);
        assert_eq!(plan.output_hz, 250_000.0);
        assert!(plan.adc_hz >= MIN_ADC_RATE_HZ);
    }

    #[test]
    fn every_offered_single_tuner_rate_can_be_planned() {
        for rate in sample_rates(None) {
            let plan = plan_rate(None, rate).expect("plan");
            assert!((plan.output_hz - rate).abs() < 1e-6);
            assert!((MIN_ADC_RATE_HZ..=MAX_ADC_RATE_HZ).contains(&plan.adc_hz));
        }
    }

    #[test]
    fn every_offered_dual_tuner_rate_can_be_planned() {
        for rate in sample_rates(Some(DuoMode::DualTuner)) {
            let plan = plan_rate(Some(DuoMode::DualTuner), rate).expect("plan");
            assert!((plan.output_hz - rate).abs() < 1e-6);
            assert_eq!(plan.adc_hz, crate::model::DUO_DUAL_TUNER_FS_HZ);
            assert_eq!(plan.if_khz, ffi::IF_1_620);
        }
    }

    #[test]
    fn a_dual_tuner_rate_the_fixed_clock_cannot_reach_is_refused() {
        let error = plan_rate(Some(DuoMode::DualTuner), 48_000.0).expect_err("unsupported");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn a_rate_above_the_adc_is_refused() {
        assert!(matches!(
            plan_rate(None, 20_000_000.0),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn a_rate_below_the_deepest_decimation_is_refused() {
        assert!(matches!(
            plan_rate(None, 1_000.0),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn a_nonsense_rate_is_refused() {
        assert!(plan_rate(None, 0.0).is_err());
        assert!(plan_rate(None, f64::NAN).is_err());
    }

    #[test]
    fn low_if_modes_cap_the_bandwidth_at_the_widest_the_api_allows() {
        let widest = bandwidths(Some(DuoMode::DualTuner))
            .into_iter()
            .fold(0.0_f64, f64::max);
        assert_eq!(widest, 1_536_000.0);
        assert_eq!(
            bandwidths(None).into_iter().fold(0.0_f64, f64::max),
            8_000_000.0
        );
    }

    #[test]
    fn bandwidth_snaps_down_to_a_filter_the_hardware_has() {
        assert_eq!(bandwidth_khz(1_000_000.0), 600);
        assert_eq!(bandwidth_khz(1_536_000.0), 1536);
        assert_eq!(bandwidth_khz(10.0), 200);
    }

    #[test]
    fn the_default_bandwidth_never_exceeds_a_low_if_mode_limit() {
        assert_eq!(default_bandwidth_khz(8_000_000.0, None), 8000);
        assert_eq!(
            default_bandwidth_khz(2_000_000.0, Some(DuoMode::DualTuner)),
            1536
        );
    }

    #[test]
    fn dual_tuner_capabilities_carry_two_independently_tuned_streams() {
        let caps = capabilities(
            Model::RspDuo,
            Some(DuoMode::DualTuner),
            band(Model::RspDuo, 100e6),
        );
        assert_eq!(caps.rx_streams, 2);
        assert!(caps.per_stream.tuning);
        assert!(caps.per_stream.gain);
        assert!(!caps.per_stream.antenna);
    }

    #[test]
    fn a_single_tuner_device_declares_no_per_stream_settings() {
        let caps = capabilities(Model::Rsp1a, None, band(Model::Rsp1a, 100e6));
        assert_eq!(caps.rx_streams, 1);
        assert_eq!(caps.per_stream, StreamScope::default());
        assert_eq!(caps.tx_streams, 0);
    }

    #[test]
    fn only_the_models_with_the_hardware_offer_its_setting() {
        let names = |model, mode| -> Vec<String> {
            extras(model, mode)
                .iter()
                .map(|extra| extra.name().to_string())
                .collect()
        };
        let rsp1 = names(Model::Rsp1, None);
        assert!(!rsp1.contains(&EXTRA_BIAS_T.to_string()));
        assert!(!rsp1.contains(&EXTRA_RF_NOTCH.to_string()));
        let dx = names(Model::RspDx, None);
        assert!(dx.contains(&EXTRA_HDR.to_string()));
        assert!(dx.contains(&EXTRA_DAB_NOTCH.to_string()));
        assert!(!names(Model::Rsp2, None).contains(&EXTRA_HDR.to_string()));
        assert!(names(Model::Rsp2, None).contains(&EXTRA_EXT_REF.to_string()));
    }

    #[test]
    fn the_am_notch_belongs_to_the_duo_tuner_that_has_the_hi_z_port() {
        let has_am_notch = |mode| {
            extras(Model::RspDuo, Some(mode))
                .iter()
                .any(|extra| extra.name() == EXTRA_AM_NOTCH)
        };
        assert!(has_am_notch(DuoMode::SingleTunerA));
        assert!(!has_am_notch(DuoMode::SingleTunerB));
    }

    #[test]
    fn only_the_first_duo_tuner_offers_the_hi_z_port() {
        assert_eq!(
            antennas(Model::RspDuo, Some(DuoMode::SingleTunerA)),
            [ANTENNA_50_OHM, ANTENNA_HI_Z]
        );
        assert!(antennas(Model::RspDuo, Some(DuoMode::SingleTunerB)).is_empty());
        assert_eq!(antennas(Model::Rsp1a, None), Vec::<String>::new());
    }

    #[test]
    fn the_rsp1_tunes_from_ten_kilohertz_and_the_others_from_one() {
        assert_eq!(frequency_range(Model::Rsp1).min, 10_000.0);
        assert_eq!(frequency_range(Model::Rsp1a).min, 1_000.0);
        assert_eq!(frequency_range(Model::RspDx).max, 2_000_000_000.0);
    }
}
