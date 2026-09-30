use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentSource {
    Rust,
    Web,
    Native,
}

impl ComponentSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust crates",
            Self::Web => "Web packages",
            Self::Native => "Hardware libraries",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Attribution {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub license: String,
    pub source: ComponentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub texts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub const API_PROTOCOL: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AboutResponse {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub server_id: String,
    pub server_name: String,
    pub license: String,
    pub license_text: String,
    pub repository: String,
    pub components: Vec<Attribution>,
    #[serde(default)]
    pub reveal: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LicenseTextResponse {
    pub id: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_names_the_protocol_and_the_server() {
        let about = AboutResponse {
            name: "SDR--".to_owned(),
            version: "0.4.0".to_owned(),
            protocol: API_PROTOCOL,
            server_id: "00112233445566778899aabbccddeeff".to_owned(),
            server_name: "pi".to_owned(),
            license: "AGPL-3.0-or-later".to_owned(),
            license_text: String::new(),
            repository: String::new(),
            components: Vec::new(),
            reveal: false,
        };
        let value = serde_json::to_value(&about).unwrap();
        assert_eq!(value["protocol"], API_PROTOCOL);
        assert_eq!(value["server_id"], "00112233445566778899aabbccddeeff");
        assert_eq!(value["server_name"], "pi");
        for gone in ["lan_addresses", "routing", "offline_basemap", "local_only"] {
            assert!(value.get(gone).is_none(), "{gone}");
        }
        assert_eq!(
            serde_json::from_value::<AboutResponse>(value).unwrap(),
            about
        );
    }
}
