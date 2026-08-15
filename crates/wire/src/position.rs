use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const DEFAULT_GPSD_ADDRESS: &str = "127.0.0.1:2947";
pub const DEFAULT_NMEA_BAUD: u32 = 9_600;
pub const MIN_NMEA_BAUD: u32 = 1_200;
pub const MAX_NMEA_BAUD: u32 = 4_000_000;
pub const DEFAULT_NMEA_UPDATE_INTERVAL_MS: u32 = 1_000;
pub const MIN_NMEA_UPDATE_INTERVAL_MS: u32 = 50;
pub const MAX_NMEA_UPDATE_INTERVAL_MS: u32 = 60_000;
pub const MAX_POSITION_ENDPOINT_LEN: usize = 256;
pub const MAX_POSITION_TIME_LEN: usize = 64;

const fn default_nmea_update_interval_ms() -> u32 {
    DEFAULT_NMEA_UPDATE_INTERVAL_MS
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PositionSource {
    #[default]
    Device,
    Gpsd {
        address: String,
    },
    Nmea {
        device: String,
        baud: u32,
        #[serde(default = "default_nmea_update_interval_ms")]
        update_interval_ms: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GpsNode {
    #[serde(default)]
    pub source: PositionSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NmeaDeviceInfo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_vid: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_pid: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NmeaDevicesResponse {
    pub devices: Vec<NmeaDeviceInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PositionFix {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_deg: Option<f64>,
    pub time: String,
}

impl PositionFix {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.latitude.is_finite() || !(-90.0..=90.0).contains(&self.latitude) {
            return Err("latitude must be within ±90°");
        }
        if !self.longitude.is_finite() || !(-180.0..=180.0).contains(&self.longitude) {
            return Err("longitude must be within ±180°");
        }
        for value in [
            self.altitude_m,
            self.accuracy_m,
            self.speed_mps,
            self.track_deg,
        ]
        .into_iter()
        .flatten()
        {
            if !value.is_finite() {
                return Err("position measurements must be finite");
            }
        }
        if self.accuracy_m.is_some_and(|value| value < 0.0) {
            return Err("accuracy must not be negative");
        }
        if self.speed_mps.is_some_and(|value| value < 0.0) {
            return Err("speed must not be negative");
        }
        if self
            .track_deg
            .is_some_and(|value| !(0.0..=360.0).contains(&value))
        {
            return Err("track must be within 0°..=360°");
        }
        if self.time.is_empty()
            || self.time.len() > MAX_POSITION_TIME_LEN
            || self.time.parse::<jiff::Timestamp>().is_err()
        {
            return Err("position time must be an RFC3339 timestamp");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> PositionFix {
        PositionFix {
            latitude: 52.52,
            longitude: 13.405,
            altitude_m: Some(40.0),
            accuracy_m: Some(3.0),
            speed_mps: Some(12.0),
            track_deg: Some(180.0),
            time: "2026-08-14T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn validates_complete_finite_fixes() {
        assert_eq!(fix().validate(), Ok(()));
        let invalid = [
            PositionFix {
                latitude: -91.0,
                ..fix()
            },
            PositionFix {
                longitude: 181.0,
                ..fix()
            },
            PositionFix {
                altitude_m: Some(f64::NAN),
                ..fix()
            },
            PositionFix {
                accuracy_m: Some(f64::INFINITY),
                ..fix()
            },
            PositionFix {
                accuracy_m: Some(-1.0),
                ..fix()
            },
            PositionFix {
                speed_mps: Some(-1.0),
                ..fix()
            },
            PositionFix {
                track_deg: Some(361.0),
                ..fix()
            },
            PositionFix {
                time: "not a timestamp".to_owned(),
                ..fix()
            },
        ];
        for bad in invalid {
            assert!(bad.validate().is_err(), "accepted invalid fix {bad:?}");
        }
    }

    #[test]
    fn position_sources_roundtrip_with_explicit_variants() {
        for source in [
            PositionSource::Device,
            PositionSource::Gpsd {
                address: DEFAULT_GPSD_ADDRESS.to_owned(),
            },
            PositionSource::Nmea {
                device: "/dev/ttyUSB0".to_owned(),
                baud: DEFAULT_NMEA_BAUD,
                update_interval_ms: DEFAULT_NMEA_UPDATE_INTERVAL_MS,
            },
        ] {
            let json = serde_json::to_value(&source).unwrap();
            assert_eq!(
                serde_json::from_value::<PositionSource>(json).unwrap(),
                source
            );
        }
    }

    #[test]
    fn older_nmea_sources_default_the_update_interval() {
        let source: PositionSource =
            serde_json::from_str(r#"{"type":"nmea","device":"/dev/ttyUSB0","baud":9600}"#).unwrap();
        assert_eq!(
            source,
            PositionSource::Nmea {
                device: "/dev/ttyUSB0".to_owned(),
                baud: DEFAULT_NMEA_BAUD,
                update_interval_ms: DEFAULT_NMEA_UPDATE_INTERVAL_MS,
            }
        );
    }
}
