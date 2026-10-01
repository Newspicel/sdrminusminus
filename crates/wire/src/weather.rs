use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AptParams {
    #[serde(default = "default_true")]
    pub keep_partial: bool,
}

impl Default for AptParams {
    fn default() -> Self {
        Self { keep_partial: true }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LrptMode {
    Qpsk72,
    #[default]
    Oqpsk72,
    Oqpsk80,
}

impl LrptMode {
    pub const ALL: [Self; 3] = [Self::Qpsk72, Self::Oqpsk72, Self::Oqpsk80];

    #[must_use]
    pub const fn symbol_rate(self) -> f64 {
        match self {
            Self::Qpsk72 | Self::Oqpsk72 => 72_000.0,
            Self::Oqpsk80 => 80_000.0,
        }
    }

    #[must_use]
    pub const fn offset(self) -> bool {
        matches!(self, Self::Oqpsk72 | Self::Oqpsk80)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Qpsk72 => "QPSK 72k",
            Self::Oqpsk72 => "OQPSK 72k",
            Self::Oqpsk80 => "OQPSK 80k",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LrptParams {
    #[serde(default)]
    pub mode: LrptMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WefaxIoc {
    #[default]
    Ioc576,
    Ioc288,
}

impl WefaxIoc {
    pub const ALL: [Self; 2] = [Self::Ioc576, Self::Ioc288];

    #[must_use]
    pub const fn value(self) -> u16 {
        match self {
            Self::Ioc576 => 576,
            Self::Ioc288 => 288,
        }
    }

    #[must_use]
    pub fn width(self) -> u16 {
        (f64::from(self.value()) * std::f64::consts::PI).round() as u16
    }

    #[must_use]
    pub const fn start_tone_hz(self) -> f64 {
        match self {
            Self::Ioc576 => 300.0,
            Self::Ioc288 => 675.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WefaxLpm {
    Lpm60,
    Lpm90,
    #[default]
    Lpm120,
    Lpm240,
}

impl WefaxLpm {
    pub const ALL: [Self; 4] = [Self::Lpm60, Self::Lpm90, Self::Lpm120, Self::Lpm240];

    #[must_use]
    pub const fn value(self) -> u16 {
        match self {
            Self::Lpm60 => 60,
            Self::Lpm90 => 90,
            Self::Lpm120 => 120,
            Self::Lpm240 => 240,
        }
    }

    #[must_use]
    pub fn line_ms(self) -> f64 {
        60_000.0 / f64::from(self.value())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WefaxParams {
    #[serde(default)]
    pub ioc: WefaxIoc,
    #[serde(default)]
    pub lpm: WefaxLpm,
    #[serde(default = "default_true")]
    pub keep_partial: bool,
}

impl Default for WefaxParams {
    fn default() -> Self {
        Self {
            ioc: WefaxIoc::default(),
            lpm: WefaxLpm::default(),
            keep_partial: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SondeType {
    Rs41,
    Dfm,
    M10,
    M20,
    Imet4,
}

impl SondeType {
    pub const ALL: [Self; 5] = [Self::Rs41, Self::Dfm, Self::M10, Self::M20, Self::Imet4];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rs41 => "RS41",
            Self::Dfm => "DFM",
            Self::M10 => "M10",
            Self::M20 => "M20",
            Self::Imet4 => "iMet-4",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadiosondeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonde: Option<SondeType>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AvhrrChannel {
    Ch1,
    Ch2,
    Ch3a,
    Ch3b,
    Ch4,
    Ch5,
}

impl AvhrrChannel {
    pub const ALL: [Self; 6] = [
        Self::Ch1,
        Self::Ch2,
        Self::Ch3a,
        Self::Ch3b,
        Self::Ch4,
        Self::Ch5,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ch1 => "1",
            Self::Ch2 => "2",
            Self::Ch3a => "3A",
            Self::Ch3b => "3B",
            Self::Ch4 => "4",
            Self::Ch5 => "5",
        }
    }

    #[must_use]
    pub const fn wedge(self) -> u8 {
        match self {
            Self::Ch1 => 1,
            Self::Ch2 => 2,
            Self::Ch3a => 3,
            Self::Ch4 => 4,
            Self::Ch5 => 5,
            Self::Ch3b => 6,
        }
    }

    #[must_use]
    pub fn from_wedge(wedge: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.wedge() == wedge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AptImage {
    pub seq: u32,
    pub lines: u16,
    pub complete: bool,
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_a: Option<AvhrrChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_b: Option<AvhrrChannel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LrptImage {
    pub seq: u32,
    pub mode: LrptMode,
    pub width: u16,
    pub lines: u16,
    pub complete: bool,
    pub duration_ms: u32,
    pub apids: Vec<u16>,
    pub frames: u32,
    pub frames_corrected: u32,
    pub frames_failed: u32,
    pub packets_lost: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WefaxPicture {
    pub seq: u32,
    pub ioc: WefaxIoc,
    pub lpm: WefaxLpm,
    pub width: u16,
    pub lines: u16,
    pub complete: bool,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadiosondeFrame {
    pub sonde: SondeType,
    pub serial: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub climb_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humidity_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_hpa: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satellites: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_v: Option<f32>,
    pub errors_corrected: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AprsWeather {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_dir_deg: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_speed_ms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_gust_ms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain_1h_mm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain_24h_mm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain_midnight_mm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humidity_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_hpa: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub luminosity_wm2: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snow_24h_mm: Option<f32>,
}

impl AprsWeather {
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = self.temperature_c {
            parts.push(format!("{t:.1} °C"));
        }
        if let Some(h) = self.humidity_pct {
            parts.push(format!("{h}%"));
        }
        if let Some(p) = self.pressure_hpa {
            parts.push(format!("{p:.1} hPa"));
        }
        match (self.wind_dir_deg, self.wind_speed_ms) {
            (Some(dir), Some(speed)) => parts.push(format!("{dir:03}° {speed:.1} m/s")),
            (None, Some(speed)) => parts.push(format!("{speed:.1} m/s")),
            _ => {}
        }
        if let Some(rain) = self.rain_1h_mm {
            parts.push(format!("{rain:.1} mm/h"));
        }
        parts.join(" · ")
    }
}

pub(crate) fn apt_summary(p: &AptImage) -> String {
    let mut parts = vec!["APT".to_owned()];
    if let (Some(a), Some(b)) = (p.channel_a, p.channel_b) {
        parts.push(format!("ch {}/{}", a.label(), b.label()));
    }
    parts.push(if p.complete {
        format!("{} lines in {} s", p.lines, p.duration_ms / 1_000)
    } else {
        format!("{} lines, cut short", p.lines)
    });
    parts.join(" · ")
}

pub(crate) fn lrpt_summary(p: &LrptImage) -> String {
    let apids = p
        .apids
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join("/");
    let mut parts = vec![p.mode.label().to_owned()];
    if !apids.is_empty() {
        parts.push(format!("APID {apids}"));
    }
    parts.push(format!("{} lines", p.lines));
    if p.frames_failed > 0 || p.packets_lost > 0 {
        parts.push(format!(
            "{} frames lost · {} packets lost",
            p.frames_failed, p.packets_lost
        ));
    }
    parts.join(" · ")
}

pub(crate) fn wefax_summary(p: &WefaxPicture) -> String {
    let mut parts = vec![format!("IOC {} · {} LPM", p.ioc.value(), p.lpm.value())];
    parts.push(if p.complete {
        format!("{} lines in {} s", p.lines, p.duration_ms / 1_000)
    } else {
        format!("{} lines, cut short", p.lines)
    });
    parts.join(" · ")
}

pub(crate) fn radiosonde_summary(f: &RadiosondeFrame) -> String {
    let mut parts = vec![format!("{} {}", f.sonde.label(), f.serial)];
    if let Some(alt) = f.altitude_m {
        parts.push(format!("{alt:.0} m"));
    }
    if let Some(climb) = f.climb_ms {
        parts.push(format!("{climb:+.1} m/s"));
    }
    if let Some(t) = f.temperature_c {
        parts.push(format!("{t:.1} °C"));
    }
    if let (Some(lat), Some(lon)) = (f.lat, f.lon) {
        parts.push(format!("{lat:.5}, {lon:.5}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wefax_geometry_follows_the_index_of_cooperation() {
        assert_eq!(WefaxIoc::Ioc576.width(), 1810);
        assert_eq!(WefaxIoc::Ioc288.width(), 905);
        assert_eq!(WefaxLpm::Lpm120.line_ms(), 500.0);
        assert_eq!(WefaxLpm::Lpm60.line_ms(), 1_000.0);
    }

    #[test]
    fn avhrr_wedges_name_their_channel() {
        for channel in AvhrrChannel::ALL {
            assert_eq!(AvhrrChannel::from_wedge(channel.wedge()), Some(channel));
        }
        assert_eq!(AvhrrChannel::from_wedge(7), None);
    }

    #[test]
    fn empty_settings_take_the_defaults() {
        assert_eq!(
            serde_json::from_str::<WefaxParams>("{}").unwrap(),
            WefaxParams::default()
        );
        assert_eq!(
            serde_json::from_str::<AptParams>("{}").unwrap(),
            AptParams::default()
        );
        assert_eq!(
            serde_json::from_str::<LrptParams>("{}").unwrap().mode,
            LrptMode::Oqpsk72
        );
        assert_eq!(
            serde_json::from_str::<RadiosondeParams>("{}")
                .unwrap()
                .sonde,
            None
        );
    }

    #[test]
    fn weather_summary_lists_what_was_reported() {
        let weather = AprsWeather {
            wind_dir_deg: Some(220),
            wind_speed_ms: Some(2.2),
            temperature_c: Some(12.5),
            humidity_pct: Some(80),
            pressure_hpa: Some(1013.2),
            ..AprsWeather::default()
        };
        assert_eq!(
            weather.summary(),
            "12.5 °C · 80% · 1013.2 hPa · 220° 2.2 m/s"
        );
    }
}
