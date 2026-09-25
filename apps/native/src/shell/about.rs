use sdrmm_wire::about::{Attribution, ComponentSource};

const SOURCE_ORDER: [ComponentSource; 3] = [
    ComponentSource::Rust,
    ComponentSource::Web,
    ComponentSource::Native,
];

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub source: ComponentSource,
    pub label: &'static str,
    pub components: Vec<Attribution>,
}

#[must_use]
pub fn matches_query(component: &Attribution, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || component.name.to_lowercase().contains(&needle)
        || component.license.to_lowercase().contains(&needle)
        || component
            .note
            .as_deref()
            .is_some_and(|note| note.to_lowercase().contains(&needle))
}

#[must_use]
pub fn group_components(components: &[Attribution], query: &str) -> Vec<Group> {
    SOURCE_ORDER
        .iter()
        .map(|source| Group {
            source: *source,
            label: source.label(),
            components: components
                .iter()
                .filter(|component| component.source == *source && matches_query(component, query))
                .cloned()
                .collect(),
        })
        .filter(|group| !group.components.is_empty())
        .collect()
}

#[must_use]
pub fn license_summary(components: &[Attribution]) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for component in components {
        match counts
            .iter_mut()
            .find(|(license, _)| *license == component.license)
        {
            Some((_, count)) => *count += 1,
            None => counts.push((component.license.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

#[must_use]
pub fn summary_line(components: &[Attribution]) -> String {
    license_summary(components)
        .into_iter()
        .take(4)
        .map(|(license, count)| format!("{count} \u{d7} {license}"))
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

#[must_use]
pub fn noted_components(components: &[Attribution]) -> Vec<Attribution> {
    components
        .iter()
        .filter(|component| component.note.is_some())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(
        name: &str,
        license: &str,
        source: ComponentSource,
        note: Option<&str>,
    ) -> Attribution {
        Attribution {
            name: name.to_owned(),
            version: None,
            license: license.to_owned(),
            source,
            url: None,
            texts: Vec::new(),
            note: note.map(str::to_owned),
        }
    }

    fn serde() -> Attribution {
        component("serde", "MIT OR Apache-2.0", ComponentSource::Rust, None)
    }

    fn codec2() -> Attribution {
        component(
            "codec2",
            "LGPL-2.1-only AND MIT",
            ComponentSource::Rust,
            Some("LGPL-2.1-only, statically linked into the binary."),
        )
    }

    fn react() -> Attribution {
        component("react", "MIT", ComponentSource::Web, None)
    }

    fn rtlsdr() -> Attribution {
        component(
            "rtl-sdr (librtlsdr)",
            "GPL-2.0-or-later",
            ComponentSource::Native,
            Some("Loaded at runtime as a SoapySDR module."),
        )
    }

    fn all() -> Vec<Attribution> {
        vec![serde(), codec2(), react(), rtlsdr()]
    }

    #[test]
    fn matches_every_component_on_a_blank_query() {
        assert!(matches_query(&serde(), ""));
        assert!(matches_query(&serde(), "   "));
    }

    #[test]
    fn matches_on_name_license_and_note() {
        assert!(matches_query(&codec2(), "CODEC"));
        assert!(!matches_query(&codec2(), "serde"));
        assert!(matches_query(&rtlsdr(), "gpl"));
        assert!(matches_query(&codec2(), "lgpl"));
        assert!(!matches_query(&react(), "gpl"));
        assert!(matches_query(&codec2(), "statically linked"));
    }

    #[test]
    fn orders_groups_rust_web_native_and_labels_them() {
        let groups = group_components(&[rtlsdr(), react(), serde()], "");
        let sources: Vec<ComponentSource> = groups.iter().map(|group| group.source).collect();
        assert_eq!(
            sources,
            [
                ComponentSource::Rust,
                ComponentSource::Web,
                ComponentSource::Native
            ]
        );
        assert_eq!(group_components(&[serde()], "")[0].label, "Rust crates");
    }

    #[test]
    fn drops_groups_a_search_empties() {
        let groups = group_components(&all(), "gpl");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].components, [codec2()]);
        assert_eq!(groups[1].components, [rtlsdr()]);
        assert!(group_components(&all(), "nosuchpackage").is_empty());
    }

    #[test]
    fn counts_licenses_most_common_first_and_breaks_ties_by_name() {
        let mut json = serde();
        json.name = "serde_json".to_owned();
        assert_eq!(
            license_summary(&[serde(), react(), json]),
            [("MIT OR Apache-2.0".to_owned(), 2), ("MIT".to_owned(), 1)]
        );
        let tied: Vec<String> = license_summary(&[react(), rtlsdr()])
            .into_iter()
            .map(|(license, _)| license)
            .collect();
        assert_eq!(tied, ["GPL-2.0-or-later", "MIT"]);
        assert_eq!(summary_line(&[react()]), "1 \u{d7} MIT");
    }

    #[test]
    fn keeps_only_the_components_whose_license_needs_explaining() {
        assert_eq!(noted_components(&all()), [codec2(), rtlsdr()]);
    }
}
