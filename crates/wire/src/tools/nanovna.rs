use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const NANOVNA_TOOL_ID: &str = "nanovna";
pub const MIN_NANOVNA_FREQ_HZ: u64 = 10_000;
pub const MAX_NANOVNA_FREQ_HZ: u64 = 6_300_000_000;
pub const MIN_NANOVNA_POINTS: u32 = 11;
pub const MAX_NANOVNA_POINTS: u32 = 10_001;
pub const MAX_NANOVNA_AVERAGES: u16 = 16;
pub const MAX_NANOVNA_PORT_LEN: usize = 1024;
pub const MAX_NANOVNA_CAL_SLOT: u8 = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NanoVnaRequest {
    ListDevices,
    Describe(NanoVnaPortRequest),
    Sweep(NanoVnaSweepRequest),
    Calibrate(NanoVnaCalibrateRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaPortRequest {
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaSweepRequest {
    pub port: String,
    pub start_hz: u64,
    pub stop_hz: u64,
    pub points: u32,
    #[serde(default = "default_averages")]
    pub averages: u16,
}

fn default_averages() -> u16 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaCalibrateRequest {
    pub port: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<NanoVnaSweepState>,
    #[serde(flatten)]
    pub step: NanoVnaCalStep,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum NanoVnaCalStep {
    Status,
    Reset,
    Open,
    Short,
    Load,
    Thru,
    Isolation,
    Finish,
    Enable,
    Disable,
    Save { slot: u8 },
    Recall { slot: u8 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NanoVnaResult {
    Devices {
        devices: Vec<NanoVnaDevice>,
        ignored_ports: Vec<String>,
    },
    Device(NanoVnaDeviceReport),
    Sweep(NanoVnaSweep),
    Calibration(NanoVnaCalibration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NanoVnaMatch {
    Confirmed,
    Probable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaDevice {
    pub port: String,
    pub label: String,
    pub match_kind: NanoVnaMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_vid: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_pid: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaDeviceReport {
    pub port: String,
    pub firmware: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    pub info: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_mv: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcxo_hz: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harmonic_threshold_hz: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub electrical_delay_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s21_offset_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<NanoVnaSweepState>,
    pub calibration: NanoVnaCalibration,
    pub commands: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaSweepState {
    pub start_hz: u64,
    pub stop_hz: u64,
    pub points: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaCalibration {
    pub port: String,
    pub standards: Vec<NanoVnaStandard>,
    pub error_terms: Vec<String>,
    pub applied: bool,
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NanoVnaStandard {
    Load,
    Open,
    Short,
    Thru,
    Isolation,
}

impl NanoVnaStandard {
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "load" => Some(Self::Load),
            "open" => Some(Self::Open),
            "short" => Some(Self::Short),
            "thru" => Some(Self::Thru),
            "isoln" => Some(Self::Isolation),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaComplex {
    pub re: f64,
    pub im: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaPoint {
    pub frequency_hz: u64,
    pub s11: NanoVnaComplex,
    pub s21: NanoVnaComplex,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaSweep {
    pub device: NanoVnaDeviceReport,
    pub requested_points: u32,
    pub averages: u16,
    pub elapsed_ms: u64,
    pub points: Vec<NanoVnaPoint>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_result_tags_are_stable() {
        let request = NanoVnaRequest::Sweep(NanoVnaSweepRequest {
            port: "/dev/ttyACM0".to_owned(),
            start_hz: 1_000_000,
            stop_hz: 30_000_000,
            points: 101,
            averages: 2,
        });
        let json = serde_json::to_value(request).expect("serialize request");
        assert_eq!(json["action"], "sweep");
        assert_eq!(json["points"], 101);

        let result = NanoVnaResult::Devices {
            devices: Vec::new(),
            ignored_ports: Vec::new(),
        };
        let json = serde_json::to_value(result).expect("serialize result");
        assert_eq!(json["kind"], "devices");
    }

    #[test]
    fn omitted_averages_defaults_to_one() {
        let request: NanoVnaRequest = serde_json::from_str(
            r#"{"action":"sweep","port":"COM3","start_hz":10000,"stop_hz":30000000,"points":101}"#,
        )
        .expect("deserialize request");
        let NanoVnaRequest::Sweep(request) = request else {
            panic!("expected sweep");
        };
        assert_eq!(request.averages, 1);
    }

    #[test]
    fn a_calibration_step_carries_its_slot_beside_the_port() {
        let request: NanoVnaRequest =
            serde_json::from_str(r#"{"action":"calibrate","port":"COM3","step":"save","slot":2}"#)
                .expect("deserialize calibration");
        let NanoVnaRequest::Calibrate(request) = request else {
            panic!("expected calibrate");
        };
        assert_eq!(request.port, "COM3");
        assert_eq!(request.step, NanoVnaCalStep::Save { slot: 2 });

        let json = serde_json::to_value(NanoVnaRequest::Calibrate(NanoVnaCalibrateRequest {
            port: "COM3".to_owned(),
            range: None,
            step: NanoVnaCalStep::Open,
        }))
        .expect("serialize calibration");
        assert_eq!(json["action"], "calibrate");
        assert_eq!(json["step"], "open");
    }

    #[test]
    fn calibration_standards_parse_from_the_shell_tokens() {
        assert_eq!(
            NanoVnaStandard::from_token("isoln"),
            Some(NanoVnaStandard::Isolation)
        );
        assert_eq!(NanoVnaStandard::from_token("Es"), None);
    }
}
