use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_TLE_LEN: usize = 256;
pub const MAX_TRANSMITTER_ID_LEN: usize = 64;
pub const MAX_SATELLITE_HZ: f64 = 300e9;
pub const MAX_CATALOG_RESULTS: usize = 64;
pub const MAX_SATELLITE_QUERY_LEN: usize = 64;
pub const SATELLITE_CATALOG_SOURCE: &str = "CelesTrak";
pub const SATELLITE_CATALOG_URL: &str = "https://celestrak.org/NORAD/elements/gp.php";
pub const TRANSMITTER_SOURCE: &str = "SatNOGS DB";
pub const TRANSMITTER_URL: &str = "https://db.satnogs.org/api/transmitters/";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SatelliteNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downlink_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uplink_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmitter: Option<String>,
    #[serde(default)]
    pub tuning_locked: bool,
}

impl SatelliteNode {
    #[must_use]
    pub fn valid(&self) -> bool {
        let frequency = |hz: Option<f64>| {
            hz.is_none_or(|hz| hz.is_finite() && hz > 0.0 && hz <= MAX_SATELLITE_HZ)
        };
        self.tle
            .as_ref()
            .is_none_or(|tle| !tle.trim().is_empty() && tle.len() <= MAX_TLE_LEN)
            && frequency(self.downlink_hz)
            && frequency(self.uplink_hz)
            && self
                .transmitter
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= MAX_TRANSMITTER_ID_LEN)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SatelliteLook {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub range_km: f64,
    pub range_rate_km_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SatellitePass {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aos: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub los: Option<i64>,
    pub max_elevation_deg: f64,
    pub max_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SatelliteStatus {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tle_age_days: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<SatelliteLook>,
    #[serde(default)]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doppler_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doppler_rate_hz_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uplink_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_pass: Option<SatellitePass>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub driving: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct SatelliteCatalogQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CatalogSatellite {
    pub name: String,
    pub catalog: String,
    pub tle: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SatelliteCatalogResponse {
    pub satellites: Vec<CatalogSatellite>,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Transmitter {
    pub id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downlink_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uplink_hz: Option<f64>,
    pub alive: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TransmittersResponse {
    pub transmitters: Vec<Transmitter>,
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_node_is_valid_and_waits_for_elements() {
        assert!(SatelliteNode::default().valid());
    }

    #[test]
    fn nonsense_settings_are_refused() {
        let bad = [
            SatelliteNode {
                tle: Some(" ".to_owned()),
                ..SatelliteNode::default()
            },
            SatelliteNode {
                tle: Some("x".repeat(MAX_TLE_LEN + 1)),
                ..SatelliteNode::default()
            },
            SatelliteNode {
                downlink_hz: Some(f64::NAN),
                ..SatelliteNode::default()
            },
            SatelliteNode {
                uplink_hz: Some(-1.0),
                ..SatelliteNode::default()
            },
            SatelliteNode {
                transmitter: Some(String::new()),
                ..SatelliteNode::default()
            },
        ];
        for node in bad {
            assert!(!node.valid(), "{node:?}");
        }
    }
}
