use std::ffi::{c_int, c_uint};

use sdrmm_device::DeviceError;
use sdrmm_wire::{
    AgcSetting, BandwidthSetting, Capabilities, DeviceSettings, ExtraValue, GainKind, GainValue,
};

use crate::{
    caps::{self, Band, RatePlan},
    ffi,
    model::{DuoMode, Model},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reasons {
    pub reason: c_uint,
    pub ext1: c_uint,
}

impl Reasons {
    fn set<T: PartialEq>(&mut self, slot: &mut T, value: T, bit: c_uint) {
        if *slot != value {
            *slot = value;
            self.reason |= bit;
        }
    }

    fn set_ext1<T: PartialEq>(&mut self, slot: &mut T, value: T, bit: c_uint) {
        if *slot != value {
            *slot = value;
            self.ext1 |= bit;
        }
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.reason == ffi::UPDATE_NONE && self.ext1 == ffi::UPDATE_EXT1_NONE
    }
}

#[derive(Debug)]
pub struct Applied {
    pub reasons: Reasons,
    pub bandwidth: Option<BandwidthSetting>,
}

pub struct Target<'a> {
    pub model: Model,
    pub mode: Option<DuoMode>,
    pub dev: &'a mut ffi::DevParamsT,
    pub channel: &'a mut ffi::RxChannelParamsT,
}

impl Target<'_> {
    fn is_slave(&self) -> bool {
        self.mode == Some(DuoMode::Slave)
    }

    fn hi_z(&self) -> bool {
        match self.model {
            Model::Rsp2 => self.channel.rsp2_tuner_params.am_port_sel == ffi::RSP2_AMPORT_1,
            Model::RspDuo => {
                self.channel.rsp_duo_tuner_params.tuner1_am_port_sel == ffi::DUO_AMPORT_1
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn band(&self) -> Band {
        Band {
            model: self.model,
            center_hz: self.channel.tuner_params.rf_freq.rf_hz,
            hi_z: self.hi_z(),
            hdr: self.dev.rsp_dx_params.hdr_enable != 0,
        }
    }

    fn output_rate_hz(&self) -> f64 {
        let decimation = if self.channel.ctrl_params.decimation.enable == 0 {
            1
        } else {
            self.channel.ctrl_params.decimation.decimation_factor.max(1)
        };
        let base = if self.mode.is_some_and(DuoMode::is_low_if) {
            caps::DUO_LOW_IF_RATE_HZ
        } else {
            self.dev.fs_freq.fs_hz
        };
        base / f64::from(decimation)
    }
}

fn flag(value: bool) -> u8 {
    u8::from(value)
}

fn as_bool(extra: &ExtraValue) -> Result<bool, DeviceError> {
    extra
        .value
        .as_bool()
        .ok_or_else(|| DeviceError::Unsupported(format!("{}: expected true or false", extra.name)))
}

fn as_f64(extra: &ExtraValue) -> Result<f64, DeviceError> {
    extra
        .value
        .as_f64()
        .ok_or_else(|| DeviceError::Unsupported(format!("{}: expected a number", extra.name)))
}

fn as_str(extra: &ExtraValue) -> Result<&str, DeviceError> {
    extra
        .value
        .as_str()
        .ok_or_else(|| DeviceError::Unsupported(format!("{}: expected a name", extra.name)))
}

fn agc_mode(setting: &AgcSetting, current: c_int) -> Result<c_int, DeviceError> {
    if !setting.on {
        return Ok(ffi::AGC_DISABLE);
    }
    match setting.mode.as_deref() {
        Some(caps::AGC_5HZ) => Ok(ffi::AGC_5HZ),
        Some(caps::AGC_50HZ) => Ok(ffi::AGC_50HZ),
        Some(caps::AGC_100HZ) => Ok(ffi::AGC_100HZ),
        None if current != ffi::AGC_DISABLE => Ok(current),
        None => Ok(ffi::AGC_50HZ),
        Some(other) => Err(DeviceError::Unsupported(format!(
            "agc: {other} is not one of {}, {}, {}",
            caps::AGC_5HZ,
            caps::AGC_50HZ,
            caps::AGC_100HZ
        ))),
    }
}

fn agc_setting(mode: c_int) -> AgcSetting {
    match mode {
        ffi::AGC_5HZ => AgcSetting::in_mode(true, caps::AGC_5HZ),
        ffi::AGC_50HZ => AgcSetting::in_mode(true, caps::AGC_50HZ),
        ffi::AGC_100HZ => AgcSetting::in_mode(true, caps::AGC_100HZ),
        _ => AgcSetting::off(),
    }
}

fn hdr_bandwidth(name: &str) -> Result<c_int, DeviceError> {
    match name {
        "200 kHz" => Ok(0),
        "500 kHz" => Ok(1),
        "1.2 MHz" => Ok(2),
        "1.7 MHz" => Ok(3),
        other => Err(DeviceError::Unsupported(format!(
            "{}: {other} is not a bandwidth this receiver offers in HDR mode",
            caps::EXTRA_HDR_BW
        ))),
    }
}

fn hdr_bandwidth_name(value: c_int) -> &'static str {
    match value {
        0 => "200 kHz",
        1 => "500 kHz",
        2 => "1.2 MHz",
        _ => "1.7 MHz",
    }
}

fn apply_rate(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    requested: f64,
) -> Result<RatePlan, DeviceError> {
    let plan = caps::plan_rate(target.mode, requested)?;
    if !target.is_slave() {
        reasons.set(
            &mut target.dev.fs_freq.fs_hz,
            plan.adc_hz,
            ffi::UPDATE_DEV_FS,
        );
        reasons.set(
            &mut target.channel.tuner_params.if_type,
            plan.if_khz,
            ffi::UPDATE_TUNER_IF_TYPE,
        );
    }
    reasons.set(
        &mut target.channel.ctrl_params.decimation.enable,
        flag(plan.decimation > 1),
        ffi::UPDATE_CTRL_DECIMATION,
    );
    reasons.set(
        &mut target.channel.ctrl_params.decimation.decimation_factor,
        plan.decimation,
        ffi::UPDATE_CTRL_DECIMATION,
    );
    Ok(plan)
}

fn apply_antenna(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    antenna: &str,
    capabilities: &Capabilities,
) -> Result<(), DeviceError> {
    if !capabilities.antennas.iter().any(|known| known == antenna) {
        return Err(DeviceError::Unsupported(format!(
            "antenna {antenna}: this receiver offers {}",
            capabilities.antennas.join(", ")
        )));
    }
    match target.model {
        Model::Rsp2 => {
            let (port, selection) = match antenna {
                caps::ANTENNA_HI_Z => (ffi::RSP2_AMPORT_1, ffi::RSP2_ANTENNA_A),
                caps::ANTENNA_B => (ffi::RSP2_AMPORT_2, ffi::RSP2_ANTENNA_B),
                _ => (ffi::RSP2_AMPORT_2, ffi::RSP2_ANTENNA_A),
            };
            reasons.set(
                &mut target.channel.rsp2_tuner_params.am_port_sel,
                port,
                ffi::UPDATE_RSP2_AM_PORT,
            );
            reasons.set(
                &mut target.channel.rsp2_tuner_params.antenna_sel,
                selection,
                ffi::UPDATE_RSP2_ANTENNA,
            );
        }
        Model::RspDuo => {
            let port = if antenna == caps::ANTENNA_HI_Z {
                ffi::DUO_AMPORT_1
            } else {
                ffi::DUO_AMPORT_2
            };
            reasons.set(
                &mut target.channel.rsp_duo_tuner_params.tuner1_am_port_sel,
                port,
                ffi::UPDATE_RSPDUO_AM_PORT,
            );
        }
        Model::RspDx | Model::RspDxR2 => {
            let selection = match antenna {
                caps::ANTENNA_B => ffi::RSPDX_ANTENNA_B,
                caps::ANTENNA_C => ffi::RSPDX_ANTENNA_C,
                _ => ffi::RSPDX_ANTENNA_A,
            };
            reasons.set_ext1(
                &mut target.dev.rsp_dx_params.antenna_sel,
                selection,
                ffi::UPDATE_EXT1_RSPDX_ANTENNA,
            );
        }
        Model::Rsp1 | Model::Rsp1a | Model::Rsp1b => {}
    }
    Ok(())
}

fn apply_gain(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    gain: &GainValue,
) -> Result<(), DeviceError> {
    let stage = gain.stage.as_str();
    if stage == GainKind::If.name() {
        reasons.set(
            &mut target.channel.tuner_params.gain.gr_db,
            caps::gr_db_for_gain(gain.value_db),
            ffi::UPDATE_TUNER_GR,
        );
        reasons.set(
            &mut target.channel.tuner_params.gain.min_gr,
            ffi::NORMAL_MIN_GR,
            ffi::UPDATE_TUNER_GR,
        );
        Ok(())
    } else if stage == GainKind::Rf.name() {
        let state = caps::lna_state_for_gain(target.band(), gain.value_db);
        reasons.set(
            &mut target.channel.tuner_params.gain.lna_state,
            state,
            ffi::UPDATE_TUNER_GR,
        );
        Ok(())
    } else {
        Err(DeviceError::Unsupported(format!(
            "gain stage {stage}: this receiver has {} and {}",
            GainKind::Rf.name(),
            GainKind::If.name()
        )))
    }
}

fn apply_bias_t(target: &mut Target<'_>, reasons: &mut Reasons, on: bool) {
    match target.model {
        Model::Rsp1a | Model::Rsp1b => reasons.set(
            &mut target.channel.rsp1a_tuner_params.bias_t_enable,
            flag(on),
            ffi::UPDATE_RSP1A_BIAS_T,
        ),
        Model::Rsp2 => reasons.set(
            &mut target.channel.rsp2_tuner_params.bias_t_enable,
            flag(on),
            ffi::UPDATE_RSP2_BIAS_T,
        ),
        Model::RspDuo => reasons.set(
            &mut target.channel.rsp_duo_tuner_params.bias_t_enable,
            flag(on),
            ffi::UPDATE_RSPDUO_BIAS_T,
        ),
        Model::RspDx | Model::RspDxR2 => reasons.set_ext1(
            &mut target.dev.rsp_dx_params.bias_t_enable,
            flag(on),
            ffi::UPDATE_EXT1_RSPDX_BIAS_T,
        ),
        Model::Rsp1 => {}
    }
}

fn apply_rf_notch(target: &mut Target<'_>, reasons: &mut Reasons, on: bool) {
    match target.model {
        Model::Rsp1a | Model::Rsp1b => reasons.set(
            &mut target.dev.rsp1a_params.rf_notch_enable,
            flag(on),
            ffi::UPDATE_RSP1A_RF_NOTCH,
        ),
        Model::Rsp2 => reasons.set(
            &mut target.channel.rsp2_tuner_params.rf_notch_enable,
            flag(on),
            ffi::UPDATE_RSP2_RF_NOTCH,
        ),
        Model::RspDuo => reasons.set(
            &mut target.channel.rsp_duo_tuner_params.rf_notch_enable,
            flag(on),
            ffi::UPDATE_RSPDUO_RF_NOTCH,
        ),
        Model::RspDx | Model::RspDxR2 => reasons.set_ext1(
            &mut target.dev.rsp_dx_params.rf_notch_enable,
            flag(on),
            ffi::UPDATE_EXT1_RSPDX_RF_NOTCH,
        ),
        Model::Rsp1 => {}
    }
}

fn apply_dab_notch(target: &mut Target<'_>, reasons: &mut Reasons, on: bool) {
    match target.model {
        Model::Rsp1a | Model::Rsp1b => reasons.set(
            &mut target.dev.rsp1a_params.rf_dab_notch_enable,
            flag(on),
            ffi::UPDATE_RSP1A_RF_DAB_NOTCH,
        ),
        Model::RspDuo => reasons.set(
            &mut target.channel.rsp_duo_tuner_params.rf_dab_notch_enable,
            flag(on),
            ffi::UPDATE_RSPDUO_RF_DAB_NOTCH,
        ),
        Model::RspDx | Model::RspDxR2 => reasons.set_ext1(
            &mut target.dev.rsp_dx_params.rf_dab_notch_enable,
            flag(on),
            ffi::UPDATE_EXT1_RSPDX_RF_DAB_NOTCH,
        ),
        Model::Rsp1 | Model::Rsp2 => {}
    }
}

fn apply_ext_ref(target: &mut Target<'_>, reasons: &mut Reasons, on: bool) {
    match target.model {
        Model::Rsp2 => reasons.set(
            &mut target.dev.rsp2_params.ext_ref_output_en,
            flag(on),
            ffi::UPDATE_RSP2_EXT_REF,
        ),
        Model::RspDuo => reasons.set(
            &mut target.dev.rsp_duo_params.ext_ref_output_en,
            c_int::from(on),
            ffi::UPDATE_RSPDUO_EXT_REF,
        ),
        _ => {}
    }
}

fn apply_extra(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    extra: &ExtraValue,
    capabilities: &Capabilities,
) -> Result<(), DeviceError> {
    if !capabilities
        .extra
        .iter()
        .any(|known| known.name() == extra.name)
    {
        return Err(DeviceError::Unsupported(format!(
            "{}: this receiver has no such setting",
            extra.name
        )));
    }
    match extra.name.as_str() {
        caps::EXTRA_AGC_SETPOINT => {
            let setpoint = as_f64(extra)?.clamp(-72.0, -20.0).round() as c_int;
            reasons.set(
                &mut target.channel.ctrl_params.agc.set_point_dbfs,
                setpoint,
                ffi::UPDATE_CTRL_AGC,
            );
        }
        caps::EXTRA_DC_CORRECTION => reasons.set(
            &mut target.channel.ctrl_params.dc_offset.dc_enable,
            flag(as_bool(extra)?),
            ffi::UPDATE_CTRL_DC_OFFSET_IQ_IMBALANCE,
        ),
        caps::EXTRA_IQ_BALANCE => reasons.set(
            &mut target.channel.ctrl_params.dc_offset.iq_enable,
            flag(as_bool(extra)?),
            ffi::UPDATE_CTRL_DC_OFFSET_IQ_IMBALANCE,
        ),
        caps::EXTRA_RF_NOTCH => apply_rf_notch(target, reasons, as_bool(extra)?),
        caps::EXTRA_DAB_NOTCH => apply_dab_notch(target, reasons, as_bool(extra)?),
        caps::EXTRA_AM_NOTCH => reasons.set(
            &mut target.channel.rsp_duo_tuner_params.tuner1_am_notch_enable,
            flag(as_bool(extra)?),
            ffi::UPDATE_RSPDUO_TUNER1_AM_NOTCH,
        ),
        caps::EXTRA_EXT_REF => apply_ext_ref(target, reasons, as_bool(extra)?),
        caps::EXTRA_HDR => reasons.set_ext1(
            &mut target.dev.rsp_dx_params.hdr_enable,
            flag(as_bool(extra)?),
            ffi::UPDATE_EXT1_RSPDX_HDR_ENABLE,
        ),
        caps::EXTRA_HDR_BW => {
            let bandwidth = hdr_bandwidth(as_str(extra)?)?;
            reasons.set_ext1(
                &mut target.channel.rsp_dx_tuner_params.hdr_bw,
                bandwidth,
                ffi::UPDATE_EXT1_RSPDX_HDR_BW,
            );
        }
        other => {
            return Err(DeviceError::Unsupported(format!(
                "{other}: this receiver has no such setting"
            )));
        }
    }
    Ok(())
}

fn apply_agc(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    agc: &AgcSetting,
    capabilities: &Capabilities,
) -> Result<(), DeviceError> {
    if !capabilities.agc.admits(agc) {
        return Err(DeviceError::Unsupported(format!(
            "agc: {:?} is not a loop rate this receiver offers",
            agc.mode
        )));
    }
    let mode = agc_mode(agc, target.channel.ctrl_params.agc.enable)?;
    reasons.set(
        &mut target.channel.ctrl_params.agc.enable,
        mode,
        ffi::UPDATE_CTRL_AGC,
    );
    Ok(())
}

fn apply_bandwidth(
    target: &mut Target<'_>,
    reasons: &mut Reasons,
    asked: Option<BandwidthSetting>,
    rate: Option<RatePlan>,
    capabilities: &Capabilities,
) -> Result<Option<BandwidthSetting>, DeviceError> {
    let (khz, chosen) = match asked {
        Some(BandwidthSetting::Manual { hz }) => {
            if !capabilities.admits_bandwidth(BandwidthSetting::Manual { hz }) {
                return Err(DeviceError::Unsupported(format!(
                    "bandwidth {hz} Hz: this receiver offers {:?}",
                    capabilities.bandwidths
                )));
            }
            (caps::bandwidth_khz(hz), BandwidthSetting::Manual { hz })
        }
        Some(BandwidthSetting::Auto) => {
            let output_hz = rate.map_or_else(|| target.output_rate_hz(), |plan| plan.output_hz);
            (
                caps::default_bandwidth_khz(output_hz, target.mode),
                BandwidthSetting::Auto,
            )
        }
        None => match rate {
            Some(plan) => (
                caps::default_bandwidth_khz(plan.output_hz, target.mode),
                BandwidthSetting::Auto,
            ),
            None => return Ok(None),
        },
    };
    reasons.set(
        &mut target.channel.tuner_params.bw_type,
        khz,
        ffi::UPDATE_TUNER_BW_TYPE,
    );
    Ok(Some(chosen))
}

pub fn apply(
    target: &mut Target<'_>,
    delta: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<Applied, DeviceError> {
    let mut reasons = Reasons::default();
    let mut rate = None;

    for extra in &delta.extra {
        apply_extra(target, &mut reasons, extra, capabilities)?;
    }
    if let Some(agc) = &delta.agc {
        apply_agc(target, &mut reasons, agc, capabilities)?;
    }
    if let Some(on) = delta.bias_tee {
        if !capabilities.bias_tee {
            return Err(DeviceError::Unsupported(
                "bias tee: this receiver has none".to_string(),
            ));
        }
        apply_bias_t(target, &mut reasons, on);
    }
    if let Some(requested) = delta.sample_rate {
        rate = Some(apply_rate(target, &mut reasons, requested)?);
    }
    if let Some(center_hz) = delta.center_hz {
        let range = capabilities
            .freq_ranges
            .first()
            .copied()
            .unwrap_or(sdrmm_wire::Range {
                min: 0.0,
                max: f64::MAX,
                step: None,
            });
        if center_hz < range.min || center_hz > range.max {
            return Err(DeviceError::Unsupported(format!(
                "centre {center_hz} Hz is outside the {} Hz to {} Hz this receiver tunes",
                range.min, range.max
            )));
        }
        reasons.set(
            &mut target.channel.tuner_params.rf_freq.rf_hz,
            center_hz,
            ffi::UPDATE_TUNER_FRF,
        );
    }
    if let Some(antenna) = &delta.antenna {
        apply_antenna(target, &mut reasons, antenna, capabilities)?;
    }
    if let Some(ppm) = delta.ppm {
        if target.is_slave() {
            return Err(DeviceError::Unsupported(
                "an RSPduo slave cannot correct the clock — the master owns it".to_string(),
            ));
        }
        reasons.set(&mut target.dev.ppm, ppm, ffi::UPDATE_DEV_PPM);
    }
    let bandwidth = apply_bandwidth(target, &mut reasons, delta.bandwidth, rate, capabilities)?;
    for gain in &delta.gains {
        apply_gain(target, &mut reasons, gain)?;
    }
    Ok(Applied { reasons, bandwidth })
}

#[must_use]
pub fn read(target: &Target<'_>, bandwidth_auto: bool) -> DeviceSettings {
    let band = target.band();
    let mut extra = vec![
        ExtraValue {
            name: caps::EXTRA_AGC_SETPOINT.to_string(),
            value: target.channel.ctrl_params.agc.set_point_dbfs.into(),
        },
        ExtraValue {
            name: caps::EXTRA_DC_CORRECTION.to_string(),
            value: (target.channel.ctrl_params.dc_offset.dc_enable != 0).into(),
        },
        ExtraValue {
            name: caps::EXTRA_IQ_BALANCE.to_string(),
            value: (target.channel.ctrl_params.dc_offset.iq_enable != 0).into(),
        },
    ];
    if target.model.has_hdr() {
        extra.push(ExtraValue {
            name: caps::EXTRA_HDR.to_string(),
            value: (target.dev.rsp_dx_params.hdr_enable != 0).into(),
        });
        extra.push(ExtraValue {
            name: caps::EXTRA_HDR_BW.to_string(),
            value: hdr_bandwidth_name(target.channel.rsp_dx_tuner_params.hdr_bw).into(),
        });
    }
    DeviceSettings {
        center_hz: Some(target.channel.tuner_params.rf_freq.rf_hz),
        tuning: None,
        sample_rate: Some(target.output_rate_hz()),
        ppm: Some(target.dev.ppm),
        antenna: read_antenna(target),
        bandwidth: Some(if bandwidth_auto {
            BandwidthSetting::Auto
        } else {
            BandwidthSetting::Manual {
                hz: f64::from(target.channel.tuner_params.bw_type) * 1000.0,
            }
        }),
        dc_block: None,
        bias_tee: target.model.has_bias_t().then(|| read_bias_t(target)),
        agc: Some(agc_setting(target.channel.ctrl_params.agc.enable)),
        gains: vec![
            GainValue::new(
                GainKind::Rf,
                caps::rf_gain_db(band, target.channel.tuner_params.gain.lna_state),
            ),
            GainValue::new(
                GainKind::If,
                caps::if_gain_db(target.channel.tuner_params.gain.gr_db),
            ),
        ],
        extra,
        streams: Vec::new(),
    }
}

fn read_bias_t(target: &Target<'_>) -> bool {
    match target.model {
        Model::Rsp1a | Model::Rsp1b => target.channel.rsp1a_tuner_params.bias_t_enable != 0,
        Model::Rsp2 => target.channel.rsp2_tuner_params.bias_t_enable != 0,
        Model::RspDuo => target.channel.rsp_duo_tuner_params.bias_t_enable != 0,
        Model::RspDx | Model::RspDxR2 => target.dev.rsp_dx_params.bias_t_enable != 0,
        Model::Rsp1 => false,
    }
}

fn read_antenna(target: &Target<'_>) -> Option<String> {
    match target.model {
        Model::Rsp2 => Some(
            if target.channel.rsp2_tuner_params.am_port_sel == ffi::RSP2_AMPORT_1 {
                caps::ANTENNA_HI_Z
            } else if target.channel.rsp2_tuner_params.antenna_sel == ffi::RSP2_ANTENNA_B {
                caps::ANTENNA_B
            } else {
                caps::ANTENNA_A
            }
            .to_string(),
        ),
        Model::RspDuo => matches!(target.mode, Some(DuoMode::SingleTunerA | DuoMode::MasterA))
            .then(|| {
                if target.channel.rsp_duo_tuner_params.tuner1_am_port_sel == ffi::DUO_AMPORT_1 {
                    caps::ANTENNA_HI_Z.to_string()
                } else {
                    caps::ANTENNA_50_OHM.to_string()
                }
            }),
        Model::RspDx | Model::RspDxR2 => Some(
            match target.dev.rsp_dx_params.antenna_sel {
                ffi::RSPDX_ANTENNA_B => caps::ANTENNA_B,
                ffi::RSPDX_ANTENNA_C => caps::ANTENNA_C,
                _ => caps::ANTENNA_A,
            }
            .to_string(),
        ),
        Model::Rsp1 | Model::Rsp1a | Model::Rsp1b => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Params {
        dev: ffi::DevParamsT,
        channel: ffi::RxChannelParamsT,
    }

    impl Params {
        fn new() -> Self {
            let mut channel = ffi::RxChannelParamsT::default();
            channel.tuner_params.rf_freq.rf_hz = 200_000_000.0;
            channel.tuner_params.gain.gr_db = 50;
            channel.tuner_params.gain.min_gr = ffi::NORMAL_MIN_GR;
            channel.ctrl_params.agc.enable = ffi::AGC_50HZ;
            channel.ctrl_params.agc.set_point_dbfs = -60;
            channel.ctrl_params.dc_offset.dc_enable = 1;
            channel.ctrl_params.dc_offset.iq_enable = 1;
            channel.ctrl_params.decimation.decimation_factor = 1;
            let mut dev = ffi::DevParamsT::default();
            dev.fs_freq.fs_hz = 2_000_000.0;
            Self { dev, channel }
        }

        fn target(&mut self, model: Model, mode: Option<DuoMode>) -> Target<'_> {
            Target {
                model,
                mode,
                dev: &mut self.dev,
                channel: &mut self.channel,
            }
        }
    }

    fn capabilities(model: Model, mode: Option<DuoMode>) -> Capabilities {
        caps::capabilities(
            model,
            mode,
            Band {
                model,
                center_hz: 200e6,
                hi_z: false,
                hdr: false,
            },
        )
    }

    fn extra(name: &str, value: serde_json::Value) -> DeviceSettings {
        DeviceSettings {
            extra: vec![ExtraValue {
                name: name.to_string(),
                value,
            }],
            ..DeviceSettings::default()
        }
    }

    fn bias_tee(on: bool) -> DeviceSettings {
        DeviceSettings {
            bias_tee: Some(on),
            ..DeviceSettings::default()
        }
    }

    fn agc(setting: AgcSetting) -> DeviceSettings {
        DeviceSettings {
            agc: Some(setting),
            ..DeviceSettings::default()
        }
    }

    fn manual(hz: f64) -> Option<BandwidthSetting> {
        Some(BandwidthSetting::Manual { hz })
    }

    #[test]
    fn tuning_asks_the_api_only_for_the_frequency() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let applied = apply(
            &mut target,
            &DeviceSettings {
                center_hz: Some(100_000_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("tune");
        assert_eq!(applied.reasons.reason, ffi::UPDATE_TUNER_FRF);
        assert_eq!(applied.reasons.ext1, ffi::UPDATE_EXT1_NONE);
        assert_eq!(params.channel.tuner_params.rf_freq.rf_hz, 100_000_000.0);
    }

    #[test]
    fn writing_a_value_that_is_already_set_asks_the_api_for_nothing() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let applied = apply(
            &mut target,
            &DeviceSettings {
                center_hz: Some(200_000_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("tune");
        assert!(applied.reasons.is_empty());
    }

    #[test]
    fn a_low_rate_sets_the_clock_the_decimation_and_a_matching_filter() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let applied = apply(
            &mut target,
            &DeviceSettings {
                sample_rate: Some(250_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("rate");
        assert!(applied.reasons.reason & ffi::UPDATE_CTRL_DECIMATION != 0);
        assert!(applied.reasons.reason & ffi::UPDATE_TUNER_BW_TYPE != 0);
        assert_eq!(applied.bandwidth, Some(BandwidthSetting::Auto));
        assert_eq!(params.channel.ctrl_params.decimation.decimation_factor, 8);
        assert_eq!(params.channel.ctrl_params.decimation.enable, 1);
        assert_eq!(params.dev.fs_freq.fs_hz, 2_000_000.0);
        assert_eq!(params.channel.tuner_params.bw_type, ffi::BW_0_200);
    }

    #[test]
    fn an_explicit_bandwidth_wins_over_the_one_the_rate_would_pick() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let applied = apply(
            &mut target,
            &DeviceSettings {
                sample_rate: Some(2_000_000.0),
                bandwidth: manual(600_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("rate");
        assert_eq!(applied.bandwidth, manual(600_000.0));
        assert_eq!(
            read(&target, false).bandwidth,
            manual(600_000.0),
            "a width the operator picked reads back as that width"
        );
        assert_eq!(params.channel.tuner_params.bw_type, ffi::BW_0_600);
    }

    #[test]
    fn auto_picks_the_filter_matching_the_rate_the_receiver_already_runs() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        apply(
            &mut target,
            &DeviceSettings {
                sample_rate: Some(2_000_000.0),
                bandwidth: manual(300_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("rate");
        let applied = apply(
            &mut target,
            &DeviceSettings {
                bandwidth: Some(BandwidthSetting::Auto),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("auto");
        assert_eq!(applied.bandwidth, Some(BandwidthSetting::Auto));
        assert_eq!(params.channel.tuner_params.bw_type, ffi::BW_1_536);
        let target = params.target(Model::Rsp1a, None);
        assert_eq!(read(&target, true).bandwidth, Some(BandwidthSetting::Auto));
    }

    #[test]
    fn a_width_the_hardware_has_no_filter_for_is_refused() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let error = apply(
            &mut target,
            &DeviceSettings {
                bandwidth: manual(1_000_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect_err("not on the menu");
        assert!(matches!(error, DeviceError::Unsupported(_)));
        assert_eq!(params.channel.tuner_params.bw_type, 0);
    }

    #[test]
    fn a_low_if_mode_never_writes_a_filter_wider_than_the_api_allows() {
        let mut params = Params::new();
        let mut target = params.target(Model::RspDuo, Some(DuoMode::DualTuner));
        assert!(
            apply(
                &mut target,
                &DeviceSettings {
                    bandwidth: manual(8_000_000.0),
                    ..DeviceSettings::default()
                },
                &capabilities(Model::RspDuo, Some(DuoMode::DualTuner)),
            )
            .is_err(),
            "8 MHz is not on this mode's menu"
        );
        apply(
            &mut target,
            &DeviceSettings {
                sample_rate: Some(2_000_000.0),
                bandwidth: Some(BandwidthSetting::Auto),
                ..DeviceSettings::default()
            },
            &capabilities(Model::RspDuo, Some(DuoMode::DualTuner)),
        )
        .expect("rate");
        assert_eq!(params.channel.tuner_params.bw_type, ffi::BW_1_536);
        assert_eq!(params.channel.tuner_params.if_type, ffi::IF_1_620);
        assert_eq!(params.dev.fs_freq.fs_hz, crate::model::DUO_DUAL_TUNER_FS_HZ);
    }

    #[test]
    fn a_slave_decimates_without_touching_the_clock_the_master_owns() {
        let mut params = Params::new();
        let mut target = params.target(Model::RspDuo, Some(DuoMode::Slave));
        let applied = apply(
            &mut target,
            &DeviceSettings {
                sample_rate: Some(500_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::RspDuo, Some(DuoMode::Slave)),
        )
        .expect("rate");
        assert_eq!(applied.reasons.reason & ffi::UPDATE_DEV_FS, 0);
        assert_eq!(params.channel.ctrl_params.decimation.decimation_factor, 4);
        assert_eq!(params.dev.fs_freq.fs_hz, 2_000_000.0);
    }

    #[test]
    fn a_slave_cannot_correct_the_clock() {
        let mut params = Params::new();
        let mut target = params.target(Model::RspDuo, Some(DuoMode::Slave));
        let error = apply(
            &mut target,
            &DeviceSettings {
                ppm: Some(1.5),
                ..DeviceSettings::default()
            },
            &capabilities(Model::RspDuo, Some(DuoMode::Slave)),
        )
        .expect_err("master owns the clock");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn gains_become_the_reduction_pair_the_api_takes() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let applied = apply(
            &mut target,
            &DeviceSettings {
                gains: vec![
                    GainValue::new(GainKind::If, 39.0),
                    GainValue::new(GainKind::Rf, 62.0),
                ],
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect("gain");
        assert_eq!(applied.reasons.reason, ffi::UPDATE_TUNER_GR);
        assert_eq!(params.channel.tuner_params.gain.gr_db, 20);
        assert_eq!(params.channel.tuner_params.gain.lna_state, 0);
    }

    #[test]
    fn an_unknown_gain_stage_is_refused() {
        let mut params = Params::new();
        let mut target = params.target(Model::Rsp1a, None);
        let error = apply(
            &mut target,
            &DeviceSettings {
                gains: vec![GainValue::new(GainKind::Tuner, 10.0)],
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect_err("no such stage");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn the_hi_z_port_moves_the_gain_table_it_is_read_against() {
        let mut params = Params::new();
        params.channel.tuner_params.rf_freq.rf_hz = 5_000_000.0;
        let mut target = params.target(Model::Rsp2, None);
        apply(
            &mut target,
            &DeviceSettings {
                antenna: Some(caps::ANTENNA_HI_Z.to_string()),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp2, None),
        )
        .expect("antenna");
        assert_eq!(
            params.channel.rsp2_tuner_params.am_port_sel,
            ffi::RSP2_AMPORT_1
        );
        let target = params.target(Model::Rsp2, None);
        assert_eq!(caps::lna_reductions(target.band()).len(), 5);
    }

    #[test]
    fn each_receiver_writes_its_bias_t_where_its_own_parameters_live() {
        let mut params = Params::new();
        apply(
            &mut params.target(Model::Rsp1a, None),
            &bias_tee(true),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("bias t");
        assert_eq!(params.channel.rsp1a_tuner_params.bias_t_enable, 1);
        assert_eq!(
            read(&params.target(Model::Rsp1a, None), true).bias_tee,
            Some(true)
        );

        let mut params = Params::new();
        let applied = apply(
            &mut params.target(Model::RspDx, None),
            &bias_tee(true),
            &capabilities(Model::RspDx, None),
        )
        .expect("bias t");
        assert_eq!(params.dev.rsp_dx_params.bias_t_enable, 1);
        assert_eq!(applied.reasons.ext1, ffi::UPDATE_EXT1_RSPDX_BIAS_T);
        assert_eq!(applied.reasons.reason, ffi::UPDATE_NONE);
    }

    #[test]
    fn a_setting_this_receiver_does_not_have_is_refused() {
        let mut params = Params::new();
        let error = apply(
            &mut params.target(Model::Rsp1, None),
            &bias_tee(true),
            &capabilities(Model::Rsp1, None),
        )
        .expect_err("no bias t on an rsp1");
        assert!(matches!(error, DeviceError::Unsupported(_)));
        assert_eq!(read(&params.target(Model::Rsp1, None), true).bias_tee, None);
        let error = apply(
            &mut params.target(Model::Rsp1, None),
            &extra(caps::EXTRA_HDR, true.into()),
            &capabilities(Model::Rsp1, None),
        )
        .expect_err("no hdr on an rsp1");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn an_extra_of_the_wrong_shape_is_refused() {
        let mut params = Params::new();
        assert!(matches!(
            apply(
                &mut params.target(Model::Rsp1a, None),
                &extra(caps::EXTRA_DC_CORRECTION, "yes".into()),
                &capabilities(Model::Rsp1a, None),
            ),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            apply(
                &mut params.target(Model::Rsp1a, None),
                &extra(caps::EXTRA_AGC_SETPOINT, "loud".into()),
                &capabilities(Model::Rsp1a, None),
            ),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn an_unknown_agc_rate_is_refused_and_a_known_one_is_written() {
        let mut params = Params::new();
        assert!(
            apply(
                &mut params.target(Model::Rsp1a, None),
                &agc(AgcSetting::in_mode(true, "1 kHz")),
                &capabilities(Model::Rsp1a, None),
            )
            .is_err()
        );
        apply(
            &mut params.target(Model::Rsp1a, None),
            &agc(AgcSetting::off()),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("agc off");
        assert_eq!(params.channel.ctrl_params.agc.enable, ffi::AGC_DISABLE);
        assert_eq!(
            read(&params.target(Model::Rsp1a, None), true).agc,
            Some(AgcSetting::off())
        );
        apply(
            &mut params.target(Model::Rsp1a, None),
            &agc(AgcSetting::in_mode(true, caps::AGC_100HZ)),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("agc 100 Hz");
        assert_eq!(params.channel.ctrl_params.agc.enable, ffi::AGC_100HZ);
        assert_eq!(
            read(&params.target(Model::Rsp1a, None), true).agc,
            Some(AgcSetting::in_mode(true, caps::AGC_100HZ))
        );
    }

    #[test]
    fn switching_the_agc_on_without_a_rate_keeps_or_restores_a_loop_rate() {
        let mut params = Params::new();
        params.channel.ctrl_params.agc.enable = ffi::AGC_5HZ;
        apply(
            &mut params.target(Model::Rsp1a, None),
            &agc(AgcSetting::switched(true)),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("agc on");
        assert_eq!(params.channel.ctrl_params.agc.enable, ffi::AGC_5HZ);
        params.channel.ctrl_params.agc.enable = ffi::AGC_DISABLE;
        apply(
            &mut params.target(Model::Rsp1a, None),
            &agc(AgcSetting::switched(true)),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("agc on");
        assert_eq!(params.channel.ctrl_params.agc.enable, ffi::AGC_50HZ);
    }

    #[test]
    fn an_agc_setpoint_outside_the_documented_window_is_clamped() {
        let mut params = Params::new();
        apply(
            &mut params.target(Model::Rsp1a, None),
            &extra(caps::EXTRA_AGC_SETPOINT, (-200).into()),
            &capabilities(Model::Rsp1a, None),
        )
        .expect("setpoint");
        assert_eq!(params.channel.ctrl_params.agc.set_point_dbfs, -72);
    }

    #[test]
    fn a_frequency_outside_the_tuning_range_is_refused() {
        let mut params = Params::new();
        let error = apply(
            &mut params.target(Model::Rsp1a, None),
            &DeviceSettings {
                center_hz: Some(3_000_000_000.0),
                ..DeviceSettings::default()
            },
            &capabilities(Model::Rsp1a, None),
        )
        .expect_err("out of range");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn an_antenna_this_receiver_does_not_have_is_refused() {
        let mut params = Params::new();
        assert!(matches!(
            apply(
                &mut params.target(Model::Rsp2, None),
                &DeviceSettings {
                    antenna: Some(caps::ANTENNA_C.to_string()),
                    ..DeviceSettings::default()
                },
                &capabilities(Model::Rsp2, None),
            ),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn what_was_written_reads_back_the_same() {
        let mut params = Params::new();
        apply(
            &mut params.target(Model::RspDx, None),
            &DeviceSettings {
                center_hz: Some(14_200_000.0),
                sample_rate: Some(500_000.0),
                antenna: Some(caps::ANTENNA_B.to_string()),
                gains: vec![GainValue::new(GainKind::If, 30.0)],
                extra: vec![ExtraValue {
                    name: caps::EXTRA_HDR.to_string(),
                    value: true.into(),
                }],
                ..DeviceSettings::default()
            },
            &capabilities(Model::RspDx, None),
        )
        .expect("apply");
        let target = params.target(Model::RspDx, None);
        let read = read(&target, true);
        assert_eq!(read.center_hz, Some(14_200_000.0));
        assert_eq!(read.sample_rate, Some(500_000.0));
        assert_eq!(read.antenna.as_deref(), Some(caps::ANTENNA_B));
        assert_eq!(read.bandwidth, Some(BandwidthSetting::Auto));
        assert_eq!(read.gain(GainKind::If.name()), Some(30.0));
        assert_eq!(
            read.extra
                .iter()
                .find(|extra| extra.name == caps::EXTRA_HDR)
                .map(|extra| extra.value.as_bool()),
            Some(Some(true))
        );
    }

    #[test]
    fn a_dual_tuner_slave_reads_its_rate_from_the_fixed_clock() {
        let mut params = Params::new();
        params.dev.fs_freq.fs_hz = crate::model::DUO_DUAL_TUNER_FS_HZ;
        params.channel.ctrl_params.decimation.enable = 1;
        params.channel.ctrl_params.decimation.decimation_factor = 4;
        let target = params.target(Model::RspDuo, Some(DuoMode::Slave));
        assert_eq!(read(&target, true).sample_rate, Some(500_000.0));
    }
}
