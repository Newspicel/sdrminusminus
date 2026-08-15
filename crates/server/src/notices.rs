use std::{collections::BTreeMap, sync::LazyLock};

use sdrmm_wire::{AboutResponse, Attribution, LicenseTextResponse};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct NoticesDocument {
    license: String,
    license_text: String,
    repository: String,
    components: Vec<Attribution>,
    texts: BTreeMap<String, String>,
}

static NOTICES_DOC: &str = include_str!("../data/notices.json");

#[expect(clippy::expect_used, reason = "compiled-in constant; see above")]
static NOTICES: LazyLock<NoticesDocument> = LazyLock::new(|| {
    serde_json::from_str(NOTICES_DOC).expect("notices.json is committed and valid")
});

#[must_use]
pub fn about() -> AboutResponse {
    AboutResponse {
        name: "sdr--".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        license: NOTICES.license.clone(),
        license_text: NOTICES.license_text.clone(),
        repository: NOTICES.repository.clone(),
        components: NOTICES.components.clone(),
    }
}

#[must_use]
pub fn license_text(id: &str) -> Option<LicenseTextResponse> {
    NOTICES.texts.get(id).map(|text| LicenseTextResponse {
        id: id.to_string(),
        text: text.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn notices_document_parses() {
        let about = about();
        assert_eq!(about.license, "GPL-3.0-or-later");
        assert!(
            about.license_text.contains("GNU GENERAL PUBLIC LICENSE"),
            "the project's own license text is missing from the notices"
        );
        assert!(
            about.components.len() > 100,
            "harvested only {} components — the generator produced a stub",
            about.components.len()
        );
    }

    #[test]
    fn every_referenced_text_resolves() {
        for component in &about().components {
            for id in &component.texts {
                assert!(
                    license_text(id).is_some(),
                    "{} references license text {id}, which the document does not carry",
                    component.name
                );
            }
        }
    }

    #[test]
    fn every_text_is_referenced() {
        let about = about();
        let referenced: BTreeSet<&str> = about
            .components
            .iter()
            .flat_map(|component| component.texts.iter().map(String::as_str))
            .collect();
        let orphans: Vec<&str> = NOTICES
            .texts
            .keys()
            .map(String::as_str)
            .filter(|id| !referenced.contains(id))
            .collect();
        assert!(
            orphans.is_empty(),
            "unreferenced license texts: {orphans:?}"
        );
    }

    #[test]
    fn copyleft_components_are_annotated() {
        let about = about();
        for name in ["codec2", "rtl-sdr (librtlsdr)", "hackrf (libhackrf)"] {
            let component = about
                .components
                .iter()
                .find(|component| component.name == name)
                .unwrap_or_else(|| panic!("{name} is missing from the notices"));
            assert!(
                component.note.is_some(),
                "{name} carries a copyleft license with no note explaining how it applies"
            );
        }
    }

    #[test]
    fn unknown_license_text_is_none() {
        assert!(license_text("deadbeefdeadbeef").is_none());
    }
}
