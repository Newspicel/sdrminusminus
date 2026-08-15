use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const NANOVNA_TOOL_ID: &str = "nanovna";
pub const MIN_NANOVNA_FREQ_HZ: u64 = 10_000;
pub const MAX_NANOVNA_FREQ_HZ: u64 = 6_300_000_000;
pub const MIN_NANOVNA_POINTS: u32 = 11;
pub const MAX_NANOVNA_POINTS: u32 = 10_001;
pub const MAX_NANOVNA_AVERAGES: u16 = 16;
pub const MAX_NANOVNA_PORT_LEN: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NanoVnaRequest {
    ListDevices,
    Sweep(NanoVnaSweepRequest),
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NanoVnaResult {
    Devices { devices: Vec<NanoVnaDevice> },
    Sweep(NanoVnaSweep),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NanoVnaDevice {
    pub port: String,
    pub label: String,
    pub likely_nanovna: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_vid: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_pid: Option<u16>,
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
    pub port: String,
    pub firmware: String,
    pub requested_points: u32,
    pub averages: u16,
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
}
