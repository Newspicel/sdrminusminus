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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LicenseTextResponse {
    pub id: String,
    pub text: String,
}
