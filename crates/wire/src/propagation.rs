use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const DEFAULT_PROPAGATION_HALF_LIFE_MIN: u32 = 30;
pub const MIN_PROPAGATION_HALF_LIFE_MIN: u32 = 5;
pub const MAX_PROPAGATION_HALF_LIFE_MIN: u32 = 720;

pub const DEFAULT_REFLECTION_HEIGHT_KM: u32 = 300;
pub const MIN_REFLECTION_HEIGHT_KM: u32 = 90;
pub const MAX_REFLECTION_HEIGHT_KM: u32 = 500;

pub const IONOSONDE_SOURCE: &str = "GIRO and INGV ionosondes, aggregated by prop.kc2g.com";
pub const IONOSONDE_URL: &str = "https://prop.kc2g.com/";
pub const IONOSONDE_MAX_STATIONS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct PropagationNode {
    pub half_life_minutes: u32,
    pub reflection_height_km: u32,
    pub show_paths: bool,
    pub compare_forecast: bool,
}

impl Default for PropagationNode {
    fn default() -> Self {
        Self {
            half_life_minutes: DEFAULT_PROPAGATION_HALF_LIFE_MIN,
            reflection_height_km: DEFAULT_REFLECTION_HEIGHT_KM,
            show_paths: false,
            compare_forecast: true,
        }
    }
}

impl PropagationNode {
    #[must_use]
    pub const fn valid(&self) -> bool {
        MIN_PROPAGATION_HALF_LIFE_MIN <= self.half_life_minutes
            && self.half_life_minutes <= MAX_PROPAGATION_HALF_LIFE_MIN
            && MIN_REFLECTION_HEIGHT_KM <= self.reflection_height_km
            && self.reflection_height_km <= MAX_REFLECTION_HEIGHT_KM
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IonosondeStation {
    pub code: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub muf3000_mhz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fof2_mhz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m3000: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub measured_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IonosondeReport {
    pub source: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    pub stations: Vec<IonosondeStation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IonosondeReport {
    #[must_use]
    pub fn empty(error: Option<String>) -> Self {
        Self {
            source: IONOSONDE_SOURCE.to_owned(),
            url: IONOSONDE_URL.to_owned(),
            fetched_at: None,
            stations: Vec::new(),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_node_is_within_its_own_bounds() {
        assert!(PropagationNode::default().valid());
    }

    #[test]
    fn settings_outside_the_bounds_are_refused() {
        let invalid = [
            PropagationNode {
                half_life_minutes: MIN_PROPAGATION_HALF_LIFE_MIN - 1,
                ..PropagationNode::default()
            },
            PropagationNode {
                half_life_minutes: MAX_PROPAGATION_HALF_LIFE_MIN + 1,
                ..PropagationNode::default()
            },
            PropagationNode {
                reflection_height_km: MIN_REFLECTION_HEIGHT_KM - 1,
                ..PropagationNode::default()
            },
            PropagationNode {
                reflection_height_km: MAX_REFLECTION_HEIGHT_KM + 1,
                ..PropagationNode::default()
            },
        ];
        for settings in invalid {
            assert!(!settings.valid(), "accepted {settings:?}");
        }
    }

    #[test]
    fn an_older_snapshot_without_the_fields_reads_as_the_default() {
        let node: PropagationNode = serde_json::from_str("{}").unwrap();
        assert_eq!(node, PropagationNode::default());
    }

    #[test]
    fn an_empty_report_still_names_where_its_data_comes_from() {
        let report = IonosondeReport::empty(Some("offline".to_owned()));
        assert_eq!(report.source, IONOSONDE_SOURCE);
        assert_eq!(report.url, IONOSONDE_URL);
        assert!(report.stations.is_empty());
        assert_eq!(report.error.as_deref(), Some("offline"));
    }
}
