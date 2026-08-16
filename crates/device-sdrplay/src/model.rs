use std::ffi::c_int;

use sdrmm_wire::DeviceInfo;

use crate::ffi;

pub const DRIVER_ID: &str = "sdrplay";
pub const DUO_DUAL_TUNER_FS_HZ: f64 = 6_000_000.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Model {
    Rsp1,
    Rsp1a,
    Rsp1b,
    Rsp2,
    RspDuo,
    RspDx,
    RspDxR2,
}

impl Model {
    #[must_use]
    pub fn from_hw_ver(hw_ver: u8) -> Option<Self> {
        match hw_ver {
            ffi::RSP1_ID => Some(Self::Rsp1),
            ffi::RSP1A_ID => Some(Self::Rsp1a),
            ffi::RSP1B_ID => Some(Self::Rsp1b),
            ffi::RSP2_ID => Some(Self::Rsp2),
            ffi::RSPDUO_ID => Some(Self::RspDuo),
            ffi::RSPDX_ID => Some(Self::RspDx),
            ffi::RSPDXR2_ID => Some(Self::RspDxR2),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Rsp1 => "RSP1",
            Self::Rsp1a => "RSP1A",
            Self::Rsp1b => "RSP1B",
            Self::Rsp2 => "RSP2",
            Self::RspDuo => "RSPduo",
            Self::RspDx => "RSPdx",
            Self::RspDxR2 => "RSPdx-R2",
        }
    }

    #[must_use]
    pub fn has_bias_t(self) -> bool {
        !matches!(self, Self::Rsp1)
    }

    #[must_use]
    pub fn has_rf_notch(self) -> bool {
        !matches!(self, Self::Rsp1)
    }

    #[must_use]
    pub fn has_dab_notch(self) -> bool {
        matches!(
            self,
            Self::Rsp1a | Self::Rsp1b | Self::RspDuo | Self::RspDx | Self::RspDxR2
        )
    }

    #[must_use]
    pub fn has_ext_ref(self) -> bool {
        matches!(self, Self::Rsp2 | Self::RspDuo)
    }

    #[must_use]
    pub fn has_hdr(self) -> bool {
        matches!(self, Self::RspDx | Self::RspDxR2)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DuoMode {
    SingleTunerA,
    SingleTunerB,
    DualTuner,
    MasterA,
    MasterB,
    Slave,
}

impl DuoMode {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::SingleTunerA => "ST_A",
            Self::SingleTunerB => "ST_B",
            Self::DualTuner => "DT",
            Self::MasterA => "MST_A",
            Self::MasterB => "MST_B",
            Self::Slave => "SLV",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ST_A" => Some(Self::SingleTunerA),
            "ST_B" => Some(Self::SingleTunerB),
            "DT" => Some(Self::DualTuner),
            "MST_A" => Some(Self::MasterA),
            "MST_B" => Some(Self::MasterB),
            "SLV" => Some(Self::Slave),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::SingleTunerA => "Tuner 1",
            Self::SingleTunerB => "Tuner 2",
            Self::DualTuner => "Dual Tuner",
            Self::MasterA => "Master, Tuner 1",
            Self::MasterB => "Master, Tuner 2",
            Self::Slave => "Slave",
        }
    }

    #[must_use]
    pub fn api_mode(self) -> c_int {
        match self {
            Self::SingleTunerA | Self::SingleTunerB => ffi::DUO_MODE_SINGLE_TUNER,
            Self::DualTuner => ffi::DUO_MODE_DUAL_TUNER,
            Self::MasterA | Self::MasterB => ffi::DUO_MODE_MASTER,
            Self::Slave => ffi::DUO_MODE_SLAVE,
        }
    }

    #[must_use]
    pub fn tuner(self) -> c_int {
        match self {
            Self::SingleTunerA | Self::MasterA => ffi::TUNER_A,
            Self::SingleTunerB | Self::MasterB => ffi::TUNER_B,
            Self::DualTuner => ffi::TUNER_BOTH,
            Self::Slave => ffi::TUNER_NEITHER,
        }
    }

    #[must_use]
    pub fn streams(self) -> u32 {
        if self == Self::DualTuner { 2 } else { 1 }
    }

    #[must_use]
    pub fn is_low_if(self) -> bool {
        !matches!(self, Self::SingleTunerA | Self::SingleTunerB)
    }
}

#[must_use]
pub fn key(serial: &str, mode: Option<DuoMode>) -> String {
    match mode {
        Some(mode) => format!("{serial}@{}", mode.code()),
        None => serial.to_string(),
    }
}

#[must_use]
pub fn split_key(key: &str) -> (String, Option<DuoMode>) {
    match key.split_once('@') {
        Some((serial, code)) => (serial.to_string(), DuoMode::from_code(code)),
        None => (key.to_string(), None),
    }
}

fn info(model: Model, serial: &str, mode: Option<DuoMode>) -> DeviceInfo {
    let label = match mode {
        Some(mode) => format!("{} {serial} ({})", model.name(), mode.label()),
        None => format!("{} {serial}", model.name()),
    };
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: key(serial, mode),
        label,
        serial: Some(serial.to_string()),
        profile: None,
    }
}

#[must_use]
pub fn duo_modes(available_modes: c_int, tuners: c_int) -> Vec<DuoMode> {
    let has_a = tuners & ffi::TUNER_A != 0;
    let has_b = tuners & ffi::TUNER_B != 0;
    let mut modes = Vec::new();
    if available_modes & ffi::DUO_MODE_SINGLE_TUNER != 0 {
        if has_a {
            modes.push(DuoMode::SingleTunerA);
        }
        if has_b {
            modes.push(DuoMode::SingleTunerB);
        }
    }
    if available_modes & ffi::DUO_MODE_DUAL_TUNER != 0 && has_a && has_b {
        modes.push(DuoMode::DualTuner);
    }
    if available_modes & ffi::DUO_MODE_MASTER != 0 {
        if has_a {
            modes.push(DuoMode::MasterA);
        }
        if has_b {
            modes.push(DuoMode::MasterB);
        }
    }
    if available_modes & ffi::DUO_MODE_SLAVE != 0 {
        modes.push(DuoMode::Slave);
    }
    modes
}

#[must_use]
pub fn describe(device: &ffi::DeviceT) -> Vec<DeviceInfo> {
    let Some(model) = Model::from_hw_ver(device.hw_ver) else {
        tracing::warn!(
            hw_ver = device.hw_ver,
            "ignoring an SDRplay device this build does not know"
        );
        return Vec::new();
    };
    let serial = device.serial();
    if serial.is_empty() {
        return Vec::new();
    }
    if model != Model::RspDuo {
        return vec![info(model, &serial, None)];
    }
    duo_modes(device.rsp_duo_mode, device.tuner)
        .into_iter()
        .map(|mode| info(model, &serial, Some(mode)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duo(modes: c_int, tuners: c_int) -> ffi::DeviceT {
        let mut device = ffi::DeviceT {
            hw_ver: ffi::RSPDUO_ID,
            tuner: tuners,
            rsp_duo_mode: modes,
            valid: 1,
            ..ffi::DeviceT::default()
        };
        for (slot, byte) in device.ser_no.iter_mut().zip(b"1809001DDD") {
            *slot = *byte as std::ffi::c_char;
        }
        device
    }

    #[test]
    fn an_idle_duo_offers_every_mode_its_tuners_allow() {
        let device = duo(
            ffi::DUO_MODE_SINGLE_TUNER | ffi::DUO_MODE_DUAL_TUNER | ffi::DUO_MODE_MASTER,
            ffi::TUNER_BOTH,
        );
        let keys: Vec<String> = describe(&device).into_iter().map(|info| info.key).collect();
        assert_eq!(
            keys,
            [
                "1809001DDD@ST_A",
                "1809001DDD@ST_B",
                "1809001DDD@DT",
                "1809001DDD@MST_A",
                "1809001DDD@MST_B",
            ]
        );
    }

    #[test]
    fn a_duo_held_by_a_master_offers_only_the_slave() {
        let device = duo(ffi::DUO_MODE_SLAVE, ffi::TUNER_B);
        let keys: Vec<String> = describe(&device).into_iter().map(|info| info.key).collect();
        assert_eq!(keys, ["1809001DDD@SLV"]);
    }

    #[test]
    fn one_tuner_in_use_hides_the_dual_tuner_mode() {
        let device = duo(
            ffi::DUO_MODE_SINGLE_TUNER | ffi::DUO_MODE_DUAL_TUNER,
            ffi::TUNER_A,
        );
        let keys: Vec<String> = describe(&device).into_iter().map(|info| info.key).collect();
        assert_eq!(keys, ["1809001DDD@ST_A"]);
    }

    #[test]
    fn every_duo_mode_survives_a_key_round_trip() {
        for mode in [
            DuoMode::SingleTunerA,
            DuoMode::SingleTunerB,
            DuoMode::DualTuner,
            DuoMode::MasterA,
            DuoMode::MasterB,
            DuoMode::Slave,
        ] {
            let (serial, parsed) = split_key(&key("123", Some(mode)));
            assert_eq!(serial, "123");
            assert_eq!(parsed, Some(mode));
        }
    }

    #[test]
    fn a_plain_serial_key_carries_no_mode() {
        assert_eq!(split_key("1234567890"), ("1234567890".to_string(), None));
    }

    #[test]
    fn a_single_tuner_device_is_listed_once_without_a_mode() {
        let mut device = ffi::DeviceT {
            hw_ver: ffi::RSP1A_ID,
            valid: 1,
            ..ffi::DeviceT::default()
        };
        for (slot, byte) in device.ser_no.iter_mut().zip(b"1234567890") {
            *slot = *byte as std::ffi::c_char;
        }
        let listed = describe(&device);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "1234567890");
        assert_eq!(listed[0].label, "RSP1A 1234567890");
    }

    #[test]
    fn an_unknown_hardware_version_is_ignored() {
        let device = ffi::DeviceT {
            hw_ver: 99,
            valid: 1,
            ..ffi::DeviceT::default()
        };
        assert!(describe(&device).is_empty());
    }

    #[test]
    fn only_the_dual_tuner_mode_carries_two_streams() {
        assert_eq!(DuoMode::DualTuner.streams(), 2);
        assert_eq!(DuoMode::SingleTunerA.streams(), 1);
        assert_eq!(DuoMode::Slave.streams(), 1);
    }
}
