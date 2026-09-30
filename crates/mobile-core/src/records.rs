use std::collections::HashMap;

use sdrmm_wire::geo;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CoreConfig {
    pub app_version: String,
    pub platform: Platform,
    pub device_model: String,
    pub data_dir: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Platform {
    Ios,
    Android,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CoreAbout {
    pub core_version: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LicenseEntry {
    pub name: String,
    pub version: Option<String>,
    pub license: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl From<LatLon> for geo::LatLon {
    fn from(value: LatLon) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
        }
    }
}

impl From<geo::LatLon> for LatLon {
    fn from(value: geo::LatLon) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NavApp {
    GoogleMaps,
    Chooser,
    Car,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SavedServer {
    pub id: String,
    pub name: String,
    pub hosts: Vec<String>,
    pub fingerprint_short: String,
    pub phone_id: String,
    pub paired_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PairOffer {
    pub hosts: Vec<String>,
    pub code: String,
    pub fingerprint: Option<String>,
    pub fingerprint_short: Option<String>,
    pub protocol: u32,
    pub server_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DiscoveredServer {
    pub name: String,
    pub hosts: Vec<String>,
    pub txt: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LinkState {
    Offline,
    Connecting { attempt: u32, host: String },
    Online { server: String },
    Refused { reason: RefusalKind, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RefusalKind {
    ServerTooOld,
    AppTooOld,
    Revoked,
    KeyMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct LocationSample {
    pub t_unix_ms: i64,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f64>,
    pub h_acc_m: f64,
    pub v_acc_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub speed_acc_mps: Option<f64>,
    pub course_deg: Option<f64>,
    pub course_acc_deg: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct HeadingSample {
    pub t_unix_ms: i64,
    pub true_deg: Option<f64>,
    pub magnetic_deg: f64,
    pub accuracy_deg: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MotionFrame {
    TrueNorth,
    Arbitrary,
    EnuMagnetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MagAccuracy {
    Uncalibrated,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct MotionSample {
    pub t_unix_ms: i64,
    pub frame: MotionFrame,
    pub qw: f64,
    pub qx: f64,
    pub qy: f64,
    pub qz: f64,
    pub rot_x: f64,
    pub rot_y: f64,
    pub rot_z: f64,
    pub grav_x: f64,
    pub grav_y: f64,
    pub grav_z: f64,
    pub heading_deg: Option<f64>,
    pub mag_accuracy: MagAccuracy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HeadingMode {
    Auto,
    Compass,
    Course,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Mount {
    Flat,
    Upright,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct PoseSettings {
    pub heading_mode: HeadingMode,
    pub mount: Mount,
    pub mount_offset_deg: f64,
    pub share_pose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HeadingSourceKind {
    None,
    Compass,
    Course,
    Fused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AlignHint {
    DriveFaster,
    DriveStraight,
    Hold,
}

#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum AlignState {
    Idle,
    Collecting { progress: f32, hint: AlignHint },
    Done { offset_deg: f64 },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PoseView {
    pub heading_deg: Option<f64>,
    pub accuracy_deg: Option<f64>,
    pub source: HeadingSourceKind,
    pub align: AlignState,
    pub sending: bool,
    pub fix_age_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Notice {
    pub level: NoticeLevel,
    pub text: String,
}

impl Notice {
    pub(crate) fn warn(text: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Warn,
            text: text.into(),
        }
    }

    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Error,
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lat_lon_converts_both_ways() {
        let at = LatLon {
            lat: 52.5,
            lon: -13.25,
        };
        let wire = geo::LatLon::from(at);
        assert_eq!((wire.lat, wire.lon), (52.5, -13.25));
        assert_eq!(LatLon::from(wire), at);
    }

    #[test]
    fn notices_carry_their_level() {
        assert_eq!(Notice::warn("a").level, NoticeLevel::Warn);
        assert_eq!(Notice::error("b").level, NoticeLevel::Error);
    }
}
