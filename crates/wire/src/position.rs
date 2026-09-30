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
pub const MAX_YAW_RATE_DPS: f64 = 1_000.0;
pub const MAX_HEADING_ACCURACY_DEG: f64 = 180.0;

const fn default_nmea_update_interval_ms() -> u32 {
    DEFAULT_NMEA_UPDATE_INTERVAL_MS
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PositionSource {
    Phone {
        phone: String,
    },
    Fixed {
        lat: f64,
        lon: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        altitude_m: Option<f64>,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadingSource {
    Compass,
    Course,
    Fused,
    Gnss,
    Sensor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Attitude {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_accuracy_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_source: Option<HeadingSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw_rate_dps: Option<f64>,
}

impl Attitude {
    pub fn validate(&self) -> Result<(), &'static str> {
        let values = [
            self.heading_deg,
            self.heading_accuracy_deg,
            self.pitch_deg,
            self.roll_deg,
            self.yaw_rate_dps,
        ];
        if values.into_iter().flatten().any(|value| !value.is_finite()) {
            return Err("attitude values must be finite");
        }
        if outside(self.heading_deg, |deg| (0.0..360.0).contains(&deg)) {
            return Err("heading must be within 0°..360°");
        }
        if outside(self.heading_accuracy_deg, |deg| {
            (0.0..=MAX_HEADING_ACCURACY_DEG).contains(&deg)
        }) {
            return Err("heading accuracy must be within 0°..180°");
        }
        if outside(self.pitch_deg, |deg| (-90.0..=90.0).contains(&deg)) {
            return Err("pitch must be within ±90°");
        }
        if outside(self.roll_deg, |deg| (-180.0..=180.0).contains(&deg)) {
            return Err("roll must be within ±180°");
        }
        if outside(self.yaw_rate_dps, |dps| {
            (-MAX_YAW_RATE_DPS..=MAX_YAW_RATE_DPS).contains(&dps)
        }) {
            return Err("yaw rate must be within ±1000°/s");
        }
        let details = self.heading_accuracy_deg.is_some()
            || self.heading_source.is_some()
            || self.yaw_rate_dps.is_some();
        if details && self.heading_deg.is_none() {
            return Err("heading details need a heading");
        }
        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn outside(value: Option<f64>, inside: impl Fn(f64) -> bool) -> bool {
    value.is_some_and(|value| !inside(value))
}

#[must_use]
pub fn normalize_heading(deg: f64) -> f64 {
    crate::geo::wrap_360(deg)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GpsNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PositionSource>,
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
    #[serde(flatten)]
    pub attitude: Attitude,
}

impl PositionFix {
    #[must_use]
    pub fn at(&self) -> crate::geo::LatLon {
        crate::geo::LatLon {
            lat: self.latitude,
            lon: self.longitude,
        }
    }

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
        self.attitude.validate()
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
            attitude: Attitude::default(),
        }
    }

    fn attitude() -> Attitude {
        Attitude {
            heading_deg: Some(87.5),
            heading_accuracy_deg: Some(4.0),
            heading_source: Some(HeadingSource::Fused),
            pitch_deg: Some(-3.0),
            roll_deg: Some(1.5),
            yaw_rate_dps: Some(-12.0),
        }
    }

    fn refused(bad: Attitude) -> &'static str {
        match bad.validate() {
            Err(problem) => problem,
            Ok(()) => panic!("accepted invalid attitude {bad:?}"),
        }
    }

    #[test]
    fn attitude_rules_accept_a_full_and_an_empty_attitude() {
        assert_eq!(attitude().validate(), Ok(()));
        assert_eq!(Attitude::default().validate(), Ok(()));
        assert!(Attitude::default().is_empty());
        assert!(!attitude().is_empty());
        for edge in [0.0, 359.999] {
            let ok = Attitude {
                heading_deg: Some(edge),
                ..attitude()
            };
            assert_eq!(ok.validate(), Ok(()));
        }
    }

    #[test]
    fn attitude_rules_refuse_out_of_range_values() {
        let cases = [
            (
                Attitude {
                    heading_deg: Some(360.0),
                    ..attitude()
                },
                "heading must be within 0°..360°",
            ),
            (
                Attitude {
                    heading_deg: Some(-0.5),
                    ..attitude()
                },
                "heading must be within 0°..360°",
            ),
            (
                Attitude {
                    heading_accuracy_deg: Some(-1.0),
                    ..attitude()
                },
                "heading accuracy must be within 0°..180°",
            ),
            (
                Attitude {
                    heading_accuracy_deg: Some(181.0),
                    ..attitude()
                },
                "heading accuracy must be within 0°..180°",
            ),
            (
                Attitude {
                    pitch_deg: Some(91.0),
                    ..attitude()
                },
                "pitch must be within ±90°",
            ),
            (
                Attitude {
                    roll_deg: Some(181.0),
                    ..attitude()
                },
                "roll must be within ±180°",
            ),
            (
                Attitude {
                    yaw_rate_dps: Some(MAX_YAW_RATE_DPS + 1.0),
                    ..attitude()
                },
                "yaw rate must be within ±1000°/s",
            ),
            (
                Attitude {
                    pitch_deg: Some(f64::NAN),
                    ..attitude()
                },
                "attitude values must be finite",
            ),
        ];
        for (bad, problem) in cases {
            assert_eq!(refused(bad), problem, "{bad:?}");
        }
    }

    #[test]
    fn attitude_rules_need_a_heading_for_its_details() {
        let details = [
            Attitude {
                heading_accuracy_deg: Some(5.0),
                ..Attitude::default()
            },
            Attitude {
                heading_source: Some(HeadingSource::Compass),
                ..Attitude::default()
            },
            Attitude {
                yaw_rate_dps: Some(3.0),
                ..Attitude::default()
            },
        ];
        for bad in details {
            assert_eq!(refused(bad), "heading details need a heading");
        }
        let tilt_only = Attitude {
            pitch_deg: Some(10.0),
            roll_deg: Some(-10.0),
            ..Attitude::default()
        };
        assert_eq!(tilt_only.validate(), Ok(()));
    }

    #[test]
    fn attitude_rules_run_last_in_a_fix() {
        let bad_both = PositionFix {
            latitude: 91.0,
            attitude: Attitude {
                heading_deg: Some(400.0),
                ..attitude()
            },
            ..fix()
        };
        assert_eq!(bad_both.validate(), Err("latitude must be within ±90°"));
        let bad_heading = PositionFix {
            attitude: Attitude {
                heading_deg: Some(400.0),
                ..attitude()
            },
            ..fix()
        };
        assert_eq!(
            bad_heading.validate(),
            Err("heading must be within 0°..360°")
        );
    }

    #[test]
    fn yaw_rate_must_be_finite() {
        for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let bad = Attitude {
                yaw_rate_dps: Some(rate),
                ..attitude()
            };
            assert_eq!(refused(bad), "attitude values must be finite");
        }
        let edge = Attitude {
            yaw_rate_dps: Some(-MAX_YAW_RATE_DPS),
            ..attitude()
        };
        assert_eq!(edge.validate(), Ok(()));
    }

    #[test]
    fn pose_json_is_flat() {
        let posed = PositionFix {
            attitude: Attitude {
                heading_deg: Some(87.5),
                heading_source: Some(HeadingSource::Fused),
                ..Attitude::default()
            },
            ..fix()
        };
        let json = serde_json::to_value(&posed).unwrap();
        assert_eq!(json["heading_deg"], 87.5);
        assert_eq!(json["heading_source"], "fused");
        assert!(json.get("attitude").is_none());
        assert!(json.get("pitch_deg").is_none());
        assert_eq!(serde_json::from_value::<PositionFix>(json).unwrap(), posed);

        let plain: PositionFix = serde_json::from_str(
            r#"{"latitude":52.52,"longitude":13.405,"time":"2026-08-14T12:00:00Z"}"#,
        )
        .unwrap();
        assert!(plain.attitude.is_empty());
        let bare = serde_json::to_value(&plain).unwrap();
        assert_eq!(
            bare,
            serde_json::json!({"latitude":52.52,"longitude":13.405,"time":"2026-08-14T12:00:00Z"})
        );
    }

    #[test]
    fn heading_sources_are_snake_case() {
        for (source, text) in [
            (HeadingSource::Compass, "compass"),
            (HeadingSource::Course, "course"),
            (HeadingSource::Fused, "fused"),
            (HeadingSource::Gnss, "gnss"),
            (HeadingSource::Sensor, "sensor"),
        ] {
            assert_eq!(serde_json::to_value(source).unwrap(), text);
        }
    }

    #[test]
    fn normalize_heading_wraps() {
        assert_eq!(normalize_heading(-10.0), 350.0);
        assert_eq!(normalize_heading(720.0), 0.0);
        assert_eq!(normalize_heading(359.5), 359.5);
        assert!(normalize_heading(-0.0).is_sign_positive());
        assert_eq!(normalize_heading(-0.0), 0.0);
    }

    #[test]
    fn a_fix_is_at_its_coordinates() {
        assert_eq!(
            fix().at(),
            crate::geo::LatLon {
                lat: 52.52,
                lon: 13.405
            }
        );
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
            PositionSource::Fixed {
                lat: 52.52,
                lon: 13.405,
                altitude_m: Some(40.0),
            },
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
    fn phone_source_roundtrips() {
        let json = serde_json::json!({"type": "phone", "phone": "p0123456789abcdef"});
        let source = PositionSource::Phone {
            phone: "p0123456789abcdef".to_owned(),
        };
        assert_eq!(
            serde_json::from_value::<PositionSource>(json.clone()).unwrap(),
            source
        );
        assert_eq!(serde_json::to_value(&source).unwrap(), json);
        assert!(serde_json::from_str::<PositionSource>(r#"{"type":"phone"}"#).is_err());
    }

    #[test]
    fn the_device_source_is_gone() {
        assert!(serde_json::from_str::<PositionSource>(r#"{"type":"device"}"#).is_err());
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
