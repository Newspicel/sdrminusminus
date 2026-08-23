use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::codeplug::CodeplugCounts;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Note,
    Adjusted,
    Dropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueScope {
    Settings,
    RadioId,
    Contact,
    GroupList,
    Channel,
    Zone,
    ScanList,
    Extension,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConversionIssue {
    pub severity: IssueSeverity,
    pub scope: IssueScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
}

impl ConversionIssue {
    #[must_use]
    pub fn new(severity: IssueSeverity, scope: IssueScope, message: impl Into<String>) -> Self {
        Self {
            severity,
            scope,
            item: None,
            field: None,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn item(mut self, item: impl Into<String>) -> Self {
        self.item = Some(item.into());
        self
    }

    #[must_use]
    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConversionReport {
    pub target_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model: Option<String>,
    pub before: CodeplugCounts,
    pub after: CodeplugCounts,
    pub issues: Vec<ConversionIssue>,
}

impl ConversionReport {
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == IssueSeverity::Dropped)
            .count()
    }

    #[must_use]
    pub fn adjusted(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == IssueSeverity::Adjusted)
            .count()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}
