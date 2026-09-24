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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AboutResponse {
    pub name: String,
    pub version: String,
    pub license: String,
    pub license_text: String,
    pub repository: String,
    pub components: Vec<Attribution>,
    /// Every address on this machine a phone on the same network can reach the server at. An
    /// operator browsing on localhost has an origin no other device can use, so the field-mode
    /// handoff offers one of these instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lan_addresses: Vec<String>,
    /// Whether a routing backend is configured, so the field client knows whether to ask for a
    /// route at all or go straight to heading guidance.
    #[serde(default)]
    pub routing: bool,
    /// Whether an operator has put a map archive next to the database, so the client can draw a
    /// basemap with no internet at all.
    #[serde(default)]
    pub offline_basemap: bool,
    /// Whether this server can show a file in the machine's own file manager. Only the desktop
    /// app, which runs on the machine holding the recordings, offers it; a browser reaching a
    /// server elsewhere gets download links instead.
    #[serde(default)]
    pub reveal: bool,
    #[serde(default)]
    pub local_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LicenseTextResponse {
    pub id: String,
    pub text: String,
}
