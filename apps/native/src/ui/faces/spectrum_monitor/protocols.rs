use std::collections::BTreeSet;

use sdrmm_wire::channel::{ChannelDescriptor, DecoderFamily};

use crate::ui::faces::channel::picker::FAMILIES;

#[derive(Clone, Debug, PartialEq)]
pub struct Protocol {
    pub kind: String,
    pub label: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolGroup {
    pub family: DecoderFamily,
    pub title: &'static str,
    pub protocols: Vec<Protocol>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProtocolChoice {
    pub disabled: Vec<String>,
    pub unidentified: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolPreset {
    pub id: String,
    pub label: &'static str,
    pub choice: ProtocolChoice,
}

fn short_label(name: &str) -> String {
    match name.find(" (") {
        Some(at) if name.ends_with(')') => name[..at].trim_end().to_owned(),
        _ => name.to_owned(),
    }
}

#[must_use]
pub fn protocol_groups(types: &[ChannelDescriptor]) -> Vec<ProtocolGroup> {
    FAMILIES
        .iter()
        .map(|(family, title)| ProtocolGroup {
            family: *family,
            title,
            protocols: types
                .iter()
                .filter(|descriptor| descriptor.identifiable && descriptor.family == *family)
                .map(|descriptor| Protocol {
                    kind: descriptor.type_id.clone(),
                    label: short_label(&descriptor.name),
                    name: descriptor.name.clone(),
                })
                .collect(),
        })
        .filter(|group| !group.protocols.is_empty())
        .collect()
}

fn kinds_of(groups: &[ProtocolGroup]) -> Vec<String> {
    groups
        .iter()
        .flat_map(|group| group.protocols.iter().map(|protocol| protocol.kind.clone()))
        .collect()
}

#[must_use]
pub fn protocol_presets(groups: &[ProtocolGroup]) -> Vec<ProtocolPreset> {
    let all = kinds_of(groups);
    let mut presets = vec![ProtocolPreset {
        id: String::from("all"),
        label: "All",
        choice: ProtocolChoice {
            disabled: Vec::new(),
            unidentified: true,
        },
    }];
    presets.extend(groups.iter().map(|group| {
        let kept = kinds_of(std::slice::from_ref(group));
        ProtocolPreset {
            id: format!("{:?}", group.family),
            label: group.title,
            choice: ProtocolChoice {
                disabled: all
                    .iter()
                    .filter(|kind| !kept.contains(kind))
                    .cloned()
                    .collect(),
                unidentified: false,
            },
        }
    }));
    presets
}

#[must_use]
pub fn enabled_kinds(groups: &[ProtocolGroup], disabled: &[String]) -> Vec<String> {
    kinds_of(groups)
        .into_iter()
        .filter(|kind| !disabled.contains(kind))
        .collect()
}

fn same_choice(groups: &[ProtocolGroup], a: &ProtocolChoice, b: &ProtocolChoice) -> bool {
    a.unidentified == b.unidentified
        && enabled_kinds(groups, &a.disabled) == enabled_kinds(groups, &b.disabled)
}

#[must_use]
pub fn active_preset(groups: &[ProtocolGroup], choice: &ProtocolChoice) -> Option<ProtocolPreset> {
    protocol_presets(groups)
        .into_iter()
        .find(|preset| same_choice(groups, &preset.choice, choice))
}

#[must_use]
pub fn choice_summary(groups: &[ProtocolGroup], choice: &ProtocolChoice) -> String {
    if let Some(preset) = active_preset(groups, choice) {
        return if preset.id == "all" {
            String::from("All protocols")
        } else {
            preset.label.to_owned()
        };
    }
    let on = enabled_kinds(groups, &choice.disabled).len();
    if on == 0 {
        return String::from(if choice.unidentified {
            "Unidentified only"
        } else {
            "Nothing"
        });
    }
    format!("{on} of {}", kinds_of(groups).len())
}

#[must_use]
pub fn set_enabled(disabled: &[String], kinds: &[String], enabled: bool) -> Vec<String> {
    let mut off: BTreeSet<String> = disabled.iter().cloned().collect();
    for kind in kinds {
        if enabled {
            off.remove(kind);
        } else {
            off.insert(kind.clone());
        }
    }
    off.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(
        type_id: &str,
        name: &str,
        family: DecoderFamily,
        identifiable: bool,
    ) -> ChannelDescriptor {
        ChannelDescriptor {
            type_id: type_id.into(),
            name: name.into(),
            family,
            identifiable,
            ..ChannelDescriptor::default()
        }
    }

    fn groups() -> Vec<ProtocolGroup> {
        protocol_groups(&[
            kind("nfm", "NFM", DecoderFamily::AnalogVoice, true),
            kind("wfm", "WFM (broadcast)", DecoderFamily::AnalogVoice, true),
            kind("dmr", "DMR", DecoderFamily::DigitalVoice, true),
            kind("adsb", "ADS-B (1090ES)", DecoderFamily::Aviation, true),
            kind("ident", "Signal identifier", DecoderFamily::Utility, false),
        ])
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn only_identifiable_protocols_are_listed_by_family_with_short_labels() {
        let groups = groups();
        let families: Vec<DecoderFamily> = groups.iter().map(|group| group.family).collect();
        assert_eq!(
            families,
            vec![
                DecoderFamily::AnalogVoice,
                DecoderFamily::DigitalVoice,
                DecoderFamily::Aviation
            ]
        );
        let labels: Vec<&str> = groups[0]
            .protocols
            .iter()
            .map(|p| p.label.as_str())
            .collect();
        assert_eq!(labels, vec!["NFM", "WFM"]);
        assert_eq!(groups[2].protocols[0].name, "ADS-B (1090ES)");
    }

    #[test]
    fn everything_starts_enabled() {
        let groups = groups();
        let choice = ProtocolChoice {
            disabled: Vec::new(),
            unidentified: true,
        };
        assert_eq!(
            enabled_kinds(&groups, &choice.disabled),
            strings(&["nfm", "wfm", "dmr", "adsb"])
        );
        assert_eq!(
            active_preset(&groups, &choice).map(|preset| preset.id),
            Some(String::from("all"))
        );
        assert_eq!(choice_summary(&groups, &choice), "All protocols");
    }

    #[test]
    fn each_family_preset_keeps_only_that_family() {
        let groups = groups();
        let analog = protocol_presets(&groups)
            .into_iter()
            .find(|preset| preset.label == "Analog voice")
            .map(|preset| preset.choice);
        assert_eq!(
            analog,
            Some(ProtocolChoice {
                disabled: strings(&["dmr", "adsb"]),
                unidentified: false
            })
        );
        let choice = ProtocolChoice {
            disabled: strings(&["adsb", "dmr"]),
            unidentified: false,
        };
        assert_eq!(choice_summary(&groups, &choice), "Analog voice");
    }

    #[test]
    fn a_custom_choice_is_counted() {
        let groups = groups();
        let disabled = set_enabled(&[], &strings(&["dmr"]), false);
        assert_eq!(disabled, strings(&["dmr"]));
        let choice = ProtocolChoice {
            disabled: disabled.clone(),
            unidentified: true,
        };
        assert_eq!(active_preset(&groups, &choice), None);
        assert_eq!(choice_summary(&groups, &choice), "3 of 4");
        assert!(set_enabled(&disabled, &strings(&["dmr"]), true).is_empty());
    }

    #[test]
    fn an_empty_choice_is_named() {
        let groups = groups();
        let disabled = strings(&["nfm", "wfm", "dmr", "adsb"]);
        let nothing = ProtocolChoice {
            disabled: disabled.clone(),
            unidentified: false,
        };
        assert_eq!(choice_summary(&groups, &nothing), "Nothing");
        let unidentified = ProtocolChoice {
            disabled,
            unidentified: true,
        };
        assert_eq!(choice_summary(&groups, &unidentified), "Unidentified only");
    }
}
