use std::{collections::BTreeMap, sync::LazyLock};

use sdrmm_wire::{AboutResponse, Attribution, LicenseTextResponse, about::API_PROTOCOL};
use serde::Deserialize;

use crate::packed::{inflate, packed_data};

#[derive(Debug, Deserialize)]
struct NoticesDocument {
    license: String,
    license_text: String,
    repository: String,
    components: Vec<Attribution>,
    texts: BTreeMap<String, String>,
}

static NOTICES_DOC: &[u8] = packed_data!("notices.json");

#[expect(clippy::expect_used, reason = "compiled-in constant; see above")]
static NOTICES: LazyLock<NoticesDocument> = LazyLock::new(|| {
    let raw = inflate(NOTICES_DOC).expect("notices.json is committed and packed");
    serde_json::from_slice(&raw).expect("notices.json is committed and valid")
});

#[must_use]
pub fn about(server_id: &str, server_name: &str) -> AboutResponse {
    AboutResponse {
        name: "SDR--".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: API_PROTOCOL,
        server_id: server_id.to_owned(),
        server_name: server_name.to_owned(),
        license: NOTICES.license.clone(),
        license_text: NOTICES.license_text.clone(),
        repository: NOTICES.repository.clone(),
        components: NOTICES.components.clone(),
        reveal: false,
    }
}

#[must_use]
pub fn lan_addresses() -> Vec<String> {
    let mut found: Vec<String> = local_ip_address::list_afinet_netifas()
        .map(|interfaces| {
            interfaces
                .into_iter()
                .map(|(_, address)| address)
                .filter(|address| !address.is_loopback() && !address.is_unspecified())
                .filter(|address| match address {
                    std::net::IpAddr::V4(v4) => !v4.is_link_local(),
                    std::net::IpAddr::V6(_) => false,
                })
                .map(|address| address.to_string())
                .collect()
        })
        .unwrap_or_default();
    found.sort_unstable();
    found.dedup();
    found
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
        let about = about("id", "host");
        assert_eq!(about.license, "AGPL-3.0-or-later");
        assert!(
            about
                .license_text
                .contains("GNU AFFERO GENERAL PUBLIC LICENSE"),
            "the project's own license text is missing from the notices"
        );
        assert!(
            about.components.len() > 100,
            "harvested only {} components: the generator produced a stub",
            about.components.len()
        );
    }

    #[test]
    fn every_referenced_text_resolves() {
        for component in &about("id", "host").components {
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
        let about = about("id", "host");
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
        let about = about("id", "host");
        for name in ["codec2", "rtl-sdr (librtlsdr)"] {
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
