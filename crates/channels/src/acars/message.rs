use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcarsCore {
    pub mode: char,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_num: Option<String>,
    pub text: String,
    pub more_to_come: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reassembled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assstat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<serde_json::Value>,
    #[serde(skip)]
    pub vdl2_link: Option<serde_json::Value>,
}
