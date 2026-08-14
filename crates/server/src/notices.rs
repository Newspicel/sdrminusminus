//! The attribution the running build owes, compiled into it.
use std::{collections::BTreeMap, sync::LazyLock};

use sdrmm_wire::{AboutResponse, Attribution, LicenseTextResponse};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct NoticesDocument {
    license: String,
    license_text: String,
    repository: String,
    components: Vec<Attribution>,
    /// License texts by content id, shared across every component with an identical copy.
    texts: BTreeMap<String, String>,
}

static NOTICES_DOC: &str = include_str!("../data/notices.json");

/// `expect` is load-of-a-compiled-in-constant, not I/O: the document is `include_str!`d, so a
/// malformed one cannot appear at runtime — it fails [`tests::notices_document_parses`] in CI,
/// before it can reach anybody (CLAUDE.md's startup exception).
#[expect(clippy::expect_used, reason = "compiled-in constant; see above")]
static NOTICES: LazyLock<NoticesDocument> = LazyLock::new(|| {
    serde_json::from_str(NOTICES_DOC).expect("notices.json is committed and valid")
});

/// `GET /api/about`.
///
/// The version comes from the running binary, not from the document: the notices are committed
/// and the version is stamped at release time by `cargo xtask set-version`, so only the binary
/// can say which build is answering.
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

/// One license text by its content id, or `None` if no component ships that text.
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

    /// The `expect` in [`NOTICES`] is only legitimate because this test exists.
    #[test]
    fn notices_document_parses() {
        let about = about();
        assert_eq!(about.license, "MIT");
        assert!(
            about.license_text.contains("MIT License"),
            "the project's own license text is missing from the notices"
        );
        assert!(
            about.components.len() > 100,
            "harvested only {} components — the generator produced a stub",
            about.components.len()
        );
    }

    /// A component pointing at a text that is not in the document is a broken link in the one
    /// place where a broken link means an undelivered copyright notice.
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

    /// Deduplication is by content, so an id that nothing references is a text nobody is owed —
    /// dead weight in a binary, and a sign the harvest and the pool disagree.
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
