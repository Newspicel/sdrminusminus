use std::collections::HashSet;

use sdrmm_wire::cps::{Codeplug, ConversionIssue, IssueScope, IssueSeverity, MergeMode, MergePart};

#[must_use]
pub fn merge(
    target: &Codeplug,
    source: &Codeplug,
    mode: MergeMode,
    parts: &[MergePart],
) -> (Codeplug, Vec<ConversionIssue>) {
    let parts: HashSet<MergePart> = if parts.is_empty() {
        MergePart::ALL.into_iter().collect()
    } else {
        parts.iter().copied().collect()
    };
    let mut out = target.clone();
    let mut issues = Vec::new();

    if parts.contains(&MergePart::RadioIds) {
        take(
            &mut out.radio_ids,
            &source.radio_ids,
            mode,
            |item| item.name.clone(),
            IssueScope::RadioId,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::Contacts) {
        take(
            &mut out.contacts,
            &source.contacts,
            mode,
            |item| item.name.clone(),
            IssueScope::Contact,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::GroupLists) {
        take(
            &mut out.group_lists,
            &source.group_lists,
            mode,
            |item| item.name.clone(),
            IssueScope::GroupList,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::Channels) {
        take(
            &mut out.channels,
            &source.channels,
            mode,
            |item| item.name.clone(),
            IssueScope::Channel,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::Zones) {
        take(
            &mut out.zones,
            &source.zones,
            mode,
            |item| item.name.clone(),
            IssueScope::Zone,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::ScanLists) {
        take(
            &mut out.scan_lists,
            &source.scan_lists,
            mode,
            |item| item.name.clone(),
            IssueScope::ScanList,
            &mut issues,
        );
    }
    if parts.contains(&MergePart::Settings) {
        out.settings = source.settings.clone();
        out.extensions.clone_from(&source.extensions);
    }
    (out, issues)
}

fn take<T: Clone>(
    target: &mut Vec<T>,
    source: &[T],
    mode: MergeMode,
    name_of: impl Fn(&T) -> String,
    scope: IssueScope,
    issues: &mut Vec<ConversionIssue>,
) {
    match mode {
        MergeMode::Replace => {
            target.clear();
            target.extend_from_slice(source);
        }
        MergeMode::Append | MergeMode::Union => {
            let taken: HashSet<String> = target.iter().map(&name_of).collect();
            for item in source {
                let name = name_of(item);
                if taken.contains(&name) {
                    if mode == MergeMode::Append {
                        issues.push(
                            ConversionIssue::new(
                                IssueSeverity::Note,
                                scope,
                                "kept the entry already in the target",
                            )
                            .item(name),
                        );
                    }
                    continue;
                }
                target.push(item.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::{Channel, Contact, ContactKind};

    use super::*;

    fn plug(channels: &[&str], contacts: &[&str]) -> Codeplug {
        let mut codeplug = Codeplug::empty();
        codeplug.channels = channels
            .iter()
            .map(|name| Channel {
                name: (*name).to_owned(),
                rx_hz: 145_000_000,
                tx_hz: 145_000_000,
                ..Channel::default()
            })
            .collect();
        codeplug.contacts = contacts
            .iter()
            .map(|name| Contact {
                name: (*name).to_owned(),
                kind: ContactKind::Group,
                number: 1,
                ring: false,
            })
            .collect();
        codeplug
    }

    #[test]
    fn replacing_takes_only_the_parts_that_were_asked_for() {
        let target = plug(&["Home"], &["Local"]);
        let source = plug(&["Away"], &["Worldwide"]);
        let (merged, _) = merge(&target, &source, MergeMode::Replace, &[MergePart::Contacts]);
        assert_eq!(merged.channels.len(), 1);
        assert_eq!(merged.channels[0].name, "Home");
        assert_eq!(merged.contacts[0].name, "Worldwide");
    }

    #[test]
    fn a_union_adds_what_is_missing_and_leaves_the_rest_alone() {
        let target = plug(&["Home", "Away"], &[]);
        let source = plug(&["Away", "Work"], &[]);
        let (merged, issues) = merge(&target, &source, MergeMode::Union, &[MergePart::Channels]);
        assert_eq!(
            merged
                .channels
                .iter()
                .map(|channel| channel.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Home", "Away", "Work"]
        );
        assert!(issues.is_empty());
    }

    #[test]
    fn appending_says_which_entries_the_target_already_had() {
        let target = plug(&["Home"], &[]);
        let source = plug(&["Home", "Work"], &[]);
        let (merged, issues) = merge(&target, &source, MergeMode::Append, &[MergePart::Channels]);
        assert_eq!(merged.channels.len(), 2);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].item.as_deref(), Some("Home"));
    }

    #[test]
    fn merging_with_no_parts_named_takes_the_whole_codeplug() {
        let target = plug(&["Home"], &["Local"]);
        let source = plug(&["Work"], &["Worldwide"]);
        let (merged, _) = merge(&target, &source, MergeMode::Replace, &[]);
        assert_eq!(merged.channels[0].name, "Work");
        assert_eq!(merged.contacts[0].name, "Worldwide");
    }
}
