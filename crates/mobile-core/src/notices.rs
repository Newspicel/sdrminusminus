use std::collections::BTreeMap;

use serde::Deserialize;

use crate::records::LicenseEntry;

const DOCUMENT: &str = include_str!("../data/notices.json");
const APP_NAME: &str = "SDR--";

#[derive(Deserialize)]
struct Document {
    license: String,
    license_text: String,
    components: Vec<Component>,
    texts: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Component {
    name: String,
    version: Option<String>,
    license: String,
    texts: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NoticesError {
    #[error("notices unreadable: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{component} names a missing license text {text}")]
    MissingText { component: String, text: String },
}

pub(crate) fn entries() -> Result<Vec<LicenseEntry>, NoticesError> {
    parse(DOCUMENT)
}

fn parse(json: &str) -> Result<Vec<LicenseEntry>, NoticesError> {
    let document: Document = serde_json::from_str(json)?;
    let mut entries = vec![LicenseEntry {
        name: APP_NAME.to_owned(),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        license: document.license,
        text: document.license_text,
    }];
    for component in document.components {
        entries.push(LicenseEntry {
            text: component_text(&component, &document.texts)?,
            name: component.name,
            version: component.version,
            license: component.license,
        });
    }
    Ok(entries)
}

fn component_text(
    component: &Component,
    texts: &BTreeMap<String, String>,
) -> Result<String, NoticesError> {
    let parts = component
        .texts
        .iter()
        .map(|id| {
            texts
                .get(id)
                .map(String::as_str)
                .ok_or_else(|| NoticesError::MissingText {
                    component: component.name.clone(),
                    text: id.clone(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if parts.is_empty() {
        component.license.clone()
    } else {
        parts.join("\n\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_name_the_core_and_its_dependencies() {
        let entries = entries().expect("compiled-in notices parse");
        assert_eq!(entries[0].name, APP_NAME);
        assert_eq!(entries[0].license, "AGPL-3.0-or-later");
        assert!(
            entries[0]
                .text
                .contains("GNU AFFERO GENERAL PUBLIC LICENSE")
        );
        for name in ["uniffi", "tokio", "rustls", "serde_json", "ring"] {
            let entry = entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("{name} listed"));
            assert!(!entry.text.is_empty(), "{name} has a text");
            assert!(entry.version.is_some(), "{name} has a version");
        }
    }

    #[test]
    fn notices_leave_out_desktop_only_crates() {
        let entries = entries().expect("compiled-in notices parse");
        for name in ["rusqlite", "axum", "wgpu", "nusb", "sdrmm-wire"] {
            assert!(
                entries.iter().all(|entry| entry.name != name),
                "{name} is not in the phone build"
            );
        }
    }

    #[test]
    fn a_missing_license_text_is_an_error() {
        let json = r#"{"license":"AGPL-3.0-or-later","license_text":"L","components":[{"name":"ring","version":"0.17.14","license":"Apache-2.0 AND ISC","texts":["gone"]}],"texts":{}}"#;
        assert!(matches!(
            parse(json),
            Err(NoticesError::MissingText { component, text }) if component == "ring" && text == "gone"
        ));
    }

    #[test]
    fn a_component_without_texts_shows_its_license() {
        let json = r#"{"license":"AGPL-3.0-or-later","license_text":"L","components":[{"name":"a","version":null,"license":"MIT","texts":[]},{"name":"b","version":"1.0.0","license":"MIT","texts":["m","n"]}],"texts":{"m":"one","n":"two"}}"#;
        let entries = parse(json).expect("parsed");
        assert_eq!(entries[1].text, "MIT");
        assert_eq!(entries[2].text, "one\n\ntwo");
    }
}
