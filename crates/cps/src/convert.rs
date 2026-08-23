use std::collections::{HashMap, HashSet};

use sdrmm_wire::cps::{
    Channel, ChannelMode, Codeplug, ConversionIssue, ConversionReport, IssueScope, IssueSeverity,
    RadioLimits, ScanTarget, Tone,
};

use crate::catalog::UniqueNames;

#[derive(Default)]
struct Issues(Vec<ConversionIssue>);

impl Issues {
    fn push(
        &mut self,
        severity: IssueSeverity,
        scope: IssueScope,
        item: &str,
        field: Option<&str>,
        message: String,
    ) {
        let mut issue = ConversionIssue::new(severity, scope, message).item(item);
        if let Some(field) = field {
            issue = issue.field(field);
        }
        self.0.push(issue);
    }
}

fn truncate(name: &str, max: u32) -> String {
    let max = max.max(1) as usize;
    if name.chars().count() <= max {
        return name.to_owned();
    }
    name.chars().take(max).collect()
}

struct Renames(HashMap<String, String>);

impl Renames {
    fn resolve(&self, name: &str) -> Option<&String> {
        self.0.get(name)
    }
}

fn rename_all(
    names: &[String],
    max_len: u32,
    scope: IssueScope,
    issues: &mut Issues,
) -> (Vec<String>, Renames) {
    let mut unique = UniqueNames::default();
    let mut mapped = Vec::with_capacity(names.len());
    let mut renames = HashMap::with_capacity(names.len());
    for name in names {
        let shortened = truncate(name, max_len);
        let claimed = unique.claim(&shortened, "Item");
        if &claimed != name {
            issues.push(
                IssueSeverity::Adjusted,
                scope,
                name,
                Some("name"),
                format!("renamed to {claimed:?} to fit this radio"),
            );
        }
        renames.insert(name.clone(), claimed.clone());
        mapped.push(claimed);
    }
    (mapped, Renames(renames))
}

fn keep<T>(items: &mut Vec<T>, limit: u32, scope: IssueScope, issues: &mut Issues, label: &str)
where
    T: Clone,
{
    let limit = limit as usize;
    if items.len() <= limit {
        return;
    }
    let dropped = items.len() - limit;
    items.truncate(limit);
    issues.push(
        IssueSeverity::Dropped,
        scope,
        label,
        None,
        format!("{dropped} of them do not fit; this radio holds {limit}"),
    );
}

fn round_to_step(hz: u64, step: u64) -> u64 {
    if step <= 1 {
        return hz;
    }
    (hz + step / 2) / step * step
}

#[must_use]
pub fn fit(
    source: &Codeplug,
    target_model: &str,
    limits: &RadioLimits,
) -> (Codeplug, ConversionReport) {
    let before = source.counts();
    let mut issues = Issues::default();
    let mut out = source.clone();
    out.version = sdrmm_wire::cps::CODEPLUG_VERSION;

    keep(
        &mut out.radio_ids,
        limits.radio_ids,
        IssueScope::RadioId,
        &mut issues,
        "radio IDs",
    );
    keep(
        &mut out.contacts,
        limits.contacts,
        IssueScope::Contact,
        &mut issues,
        "contacts",
    );
    keep(
        &mut out.group_lists,
        limits.group_lists,
        IssueScope::GroupList,
        &mut issues,
        "group lists",
    );
    keep(
        &mut out.zones,
        limits.zones,
        IssueScope::Zone,
        &mut issues,
        "zones",
    );
    keep(
        &mut out.scan_lists,
        limits.scan_lists,
        IssueScope::ScanList,
        &mut issues,
        "scan lists",
    );

    out.channels
        .retain(|channel| retain_channel(channel, limits, &mut issues));
    keep(
        &mut out.channels,
        limits.channels,
        IssueScope::Channel,
        &mut issues,
        "channels",
    );

    let (radio_id_names, radio_ids) = rename_all(
        &names_of(out.radio_ids.iter().map(|item| &item.name)),
        limits.radio_id_name_len,
        IssueScope::RadioId,
        &mut issues,
    );
    let (contact_names, contacts) = rename_all(
        &names_of(out.contacts.iter().map(|item| &item.name)),
        limits.contact_name_len,
        IssueScope::Contact,
        &mut issues,
    );
    let (group_names, group_lists) = rename_all(
        &names_of(out.group_lists.iter().map(|item| &item.name)),
        limits.group_list_name_len,
        IssueScope::GroupList,
        &mut issues,
    );
    let (zone_names, _zones) = rename_all(
        &names_of(out.zones.iter().map(|item| &item.name)),
        limits.zone_name_len,
        IssueScope::Zone,
        &mut issues,
    );
    let (scan_names, scan_lists) = rename_all(
        &names_of(out.scan_lists.iter().map(|item| &item.name)),
        limits.scan_list_name_len,
        IssueScope::ScanList,
        &mut issues,
    );
    let (channel_names, channels) = rename_all(
        &names_of(out.channels.iter().map(|item| &item.name)),
        limits.channel_name_len,
        IssueScope::Channel,
        &mut issues,
    );

    apply_names(
        out.radio_ids.iter_mut().map(|item| &mut item.name),
        &radio_id_names,
    );
    apply_names(
        out.contacts.iter_mut().map(|item| &mut item.name),
        &contact_names,
    );
    apply_names(
        out.group_lists.iter_mut().map(|item| &mut item.name),
        &group_names,
    );
    apply_names(out.zones.iter_mut().map(|item| &mut item.name), &zone_names);
    apply_names(
        out.scan_lists.iter_mut().map(|item| &mut item.name),
        &scan_names,
    );
    apply_names(
        out.channels.iter_mut().map(|item| &mut item.name),
        &channel_names,
    );

    out.settings.default_radio_id = out
        .settings
        .default_radio_id
        .as_deref()
        .and_then(|name| radio_ids.resolve(name).cloned());

    for list in &mut out.group_lists {
        let before = list.contacts.len();
        list.contacts = list
            .contacts
            .iter()
            .filter_map(|name| contacts.resolve(name).cloned())
            .take(limits.group_list_members as usize)
            .collect();
        if list.contacts.len() != before {
            issues.push(
                IssueSeverity::Dropped,
                IssueScope::GroupList,
                &list.name,
                Some("contacts"),
                format!(
                    "{} of {before} contacts were dropped",
                    before - list.contacts.len()
                ),
            );
        }
    }

    for zone in &mut out.zones {
        if !limits.features.dual_zone_lists && !zone.channels_b.is_empty() {
            issues.push(
                IssueSeverity::Adjusted,
                IssueScope::Zone,
                &zone.name,
                Some("channels_b"),
                "this radio keeps one channel list per zone; the B list was appended".to_owned(),
            );
            let folded = std::mem::take(&mut zone.channels_b);
            zone.channels_a.extend(folded);
        }
        let before = zone.channels_a.len();
        zone.channels_a = zone
            .channels_a
            .iter()
            .filter_map(|name| channels.resolve(name).cloned())
            .take(limits.zone_channels as usize)
            .collect();
        zone.channels_b = zone
            .channels_b
            .iter()
            .filter_map(|name| channels.resolve(name).cloned())
            .take(limits.zone_channels as usize)
            .collect();
        if zone.channels_a.len() != before {
            issues.push(
                IssueSeverity::Dropped,
                IssueScope::Zone,
                &zone.name,
                Some("channels"),
                format!(
                    "{} of {before} channels were dropped",
                    before - zone.channels_a.len()
                ),
            );
        }
    }

    if !limits.features.scan_lists && !out.scan_lists.is_empty() {
        issues.push(
            IssueSeverity::Dropped,
            IssueScope::ScanList,
            "scan lists",
            None,
            "this radio has no scan lists".to_owned(),
        );
        out.scan_lists.clear();
    }

    for list in &mut out.scan_lists {
        let before = list.channels.len();
        list.channels = list
            .channels
            .iter()
            .filter_map(|name| channels.resolve(name).cloned())
            .take(limits.scan_list_members as usize)
            .collect();
        if list.channels.len() != before {
            issues.push(
                IssueSeverity::Dropped,
                IssueScope::ScanList,
                &list.name,
                Some("channels"),
                format!(
                    "{} of {before} channels were dropped",
                    before - list.channels.len()
                ),
            );
        }
        list.primary = resolve_target(list.primary.take(), &channels);
        list.secondary = resolve_target(list.secondary.take(), &channels);
    }

    for channel in &mut out.channels {
        channel.rx_hz = round_to_step(channel.rx_hz, limits.frequency_step_hz);
        channel.tx_hz = round_to_step(channel.tx_hz, limits.frequency_step_hz);
        let wanted = channel.power;
        channel.power = limits.nearest_power(wanted);
        if channel.power != wanted {
            issues.push(
                IssueSeverity::Adjusted,
                IssueScope::Channel,
                &channel.name,
                Some("power"),
                format!("{wanted:?} is not offered; using {:?}", channel.power),
            );
        }
        channel.scan_list = channel
            .scan_list
            .as_deref()
            .and_then(|name| scan_lists.resolve(name).cloned());
        match &mut channel.mode {
            ChannelMode::Fm(fm) => {
                if !limits.features.dcs_tones {
                    for (tone, field) in
                        [(&mut fm.rx_tone, "rx_tone"), (&mut fm.tx_tone, "tx_tone")]
                    {
                        if matches!(tone, Some(Tone::Dcs { .. })) {
                            issues.push(
                                IssueSeverity::Dropped,
                                IssueScope::Channel,
                                &channel.name,
                                Some(field),
                                "this radio has no DCS codes".to_owned(),
                            );
                            *tone = None;
                        }
                    }
                }
            }
            ChannelMode::Dmr(dmr) => {
                dmr.contact = dmr
                    .contact
                    .as_deref()
                    .and_then(|name| contacts.resolve(name).cloned());
                dmr.group_list = dmr
                    .group_list
                    .as_deref()
                    .and_then(|name| group_lists.resolve(name).cloned());
                dmr.radio_id = if limits.features.per_channel_radio_id {
                    dmr.radio_id
                        .as_deref()
                        .and_then(|name| radio_ids.resolve(name).cloned())
                } else {
                    None
                };
                if !limits.features.group_lists {
                    dmr.group_list = None;
                }
            }
        }
    }

    let report = ConversionReport {
        target_model: target_model.to_owned(),
        source_model: source.meta.source_model.clone(),
        before,
        after: out.counts(),
        issues: coalesce(issues.0),
    };
    (out, report)
}

const REPEAT_LIMIT: usize = 5;

fn coalesce(issues: Vec<ConversionIssue>) -> Vec<ConversionIssue> {
    let mut counts: HashMap<(IssueSeverity, IssueScope, Option<String>, String), usize> =
        HashMap::new();
    for issue in &issues {
        *counts
            .entry((
                issue.severity,
                issue.scope,
                issue.field.clone(),
                issue.message.clone(),
            ))
            .or_default() += 1;
    }
    let mut seen: HashSet<(IssueSeverity, IssueScope, Option<String>, String)> = HashSet::new();
    let mut out = Vec::with_capacity(issues.len());
    for issue in issues {
        let key = (
            issue.severity,
            issue.scope,
            issue.field.clone(),
            issue.message.clone(),
        );
        let repeats = counts.get(&key).copied().unwrap_or(1);
        if repeats <= REPEAT_LIMIT {
            out.push(issue);
            continue;
        }
        if seen.insert(key) {
            out.push(ConversionIssue {
                item: Some(format!("{repeats} entries")),
                ..issue
            });
        }
    }
    out
}

fn names_of<'a>(names: impl Iterator<Item = &'a String>) -> Vec<String> {
    names.cloned().collect()
}

fn apply_names<'a>(targets: impl Iterator<Item = &'a mut String>, names: &[String]) {
    for (target, name) in targets.zip(names) {
        target.clone_from(name);
    }
}

fn resolve_target(target: Option<ScanTarget>, channels: &Renames) -> Option<ScanTarget> {
    match target {
        Some(ScanTarget::Channel { name }) => channels
            .resolve(&name)
            .cloned()
            .map(|name| ScanTarget::Channel { name }),
        other => other,
    }
}

fn retain_channel(channel: &Channel, limits: &RadioLimits, issues: &mut Issues) -> bool {
    if !limits.supports(channel.mode.kind()) {
        issues.push(
            IssueSeverity::Dropped,
            IssueScope::Channel,
            &channel.name,
            Some("mode"),
            format!("this radio has no {:?} channels", channel.mode.kind()),
        );
        return false;
    }
    if !limits.can_receive(channel.rx_hz) {
        issues.push(
            IssueSeverity::Dropped,
            IssueScope::Channel,
            &channel.name,
            Some("rx_hz"),
            format!("{} Hz is outside every receive band", channel.rx_hz),
        );
        return false;
    }
    if !channel.rx_only && !limits.can_transmit(channel.tx_hz) {
        issues.push(
            IssueSeverity::Dropped,
            IssueScope::Channel,
            &channel.name,
            Some("tx_hz"),
            format!("{} Hz is outside every transmit band", channel.tx_hz),
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::{
        Bandwidth, ChannelKind, Contact, ContactKind, DmrChannel, FmChannel, FrequencyRange,
        GroupList, Power, RadioFeatures, Zone,
    };

    use super::*;

    fn narrow_limits() -> RadioLimits {
        RadioLimits {
            channels: 32,
            contacts: 8,
            group_lists: 2,
            group_list_members: 2,
            zones: 2,
            zone_channels: 2,
            scan_lists: 1,
            scan_list_members: 4,
            radio_ids: 1,
            channel_name_len: 6,
            contact_name_len: 6,
            group_list_name_len: 8,
            zone_name_len: 8,
            scan_list_name_len: 8,
            radio_id_name_len: 8,
            rx_ranges: vec![FrequencyRange::new(144_000_000, 148_000_000)],
            tx_ranges: vec![FrequencyRange::new(144_000_000, 148_000_000)],
            powers: vec![Power::Low, Power::High],
            modes: vec![ChannelKind::Fm],
            frequency_step_hz: 1_000,
            features: RadioFeatures {
                dual_zone_lists: false,
                per_channel_radio_id: false,
                scan_lists: true,
                group_lists: true,
                dcs_tones: false,
                talkaround: false,
                named_radio_ids: true,
            },
        }
    }

    fn fm(name: &str, rx: u64) -> Channel {
        Channel {
            name: name.to_owned(),
            rx_hz: rx,
            tx_hz: rx,
            power: Power::Max,
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                tx_tone: Some(Tone::Dcs {
                    code: 23,
                    inverted: false,
                }),
                ..FmChannel::default()
            }),
            ..Channel::default()
        }
    }

    #[test]
    fn a_channel_outside_the_radios_bands_is_dropped_and_reported() {
        let mut source = Codeplug::empty();
        source.channels = vec![fm("Two metre", 145_500_000), fm("Seventy cm", 438_500_000)];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert_eq!(fitted.channels.len(), 1);
        assert_eq!(report.before.channels, 2);
        assert_eq!(report.after.channels, 1);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("rx_hz")
                    && issue.severity == IssueSeverity::Dropped)
        );
    }

    #[test]
    fn a_dmr_channel_is_dropped_when_the_radio_is_analogue_only() {
        let mut source = Codeplug::empty();
        source.channels = vec![Channel {
            name: "TG9".to_owned(),
            rx_hz: 145_000_000,
            tx_hz: 145_000_000,
            mode: ChannelMode::Dmr(DmrChannel::default()),
            ..Channel::default()
        }];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert!(fitted.channels.is_empty());
        assert_eq!(report.dropped(), 1);
    }

    #[test]
    fn names_are_shortened_and_every_reference_follows_the_new_name() {
        let mut source = Codeplug::empty();
        source.channels = vec![
            fm("Vienna repeater", 145_600_000),
            fm("Vienna simplex", 145_500_000),
        ];
        source.zones = vec![Zone {
            name: "Vienna".to_owned(),
            channels_a: vec!["Vienna repeater".to_owned()],
            channels_b: vec!["Vienna simplex".to_owned()],
        }];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert_eq!(fitted.channels[0].name, "Vienna");
        assert_eq!(fitted.channels[1].name, "Vienna (2)");
        assert_eq!(
            fitted.zones[0].channels_a,
            vec!["Vienna".to_owned(), "Vienna (2)".to_owned()]
        );
        assert!(fitted.zones[0].channels_b.is_empty());
        assert!(report.adjusted() >= 2);
    }

    #[test]
    fn power_snaps_to_the_nearest_setting_the_radio_offers() {
        let mut source = Codeplug::empty();
        source.channels = vec![fm("Local", 145_000_000)];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert_eq!(fitted.channels[0].power, Power::High);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("power"))
        );
    }

    #[test]
    fn a_dcs_tone_is_dropped_on_a_radio_without_dcs() {
        let mut source = Codeplug::empty();
        source.channels = vec![fm("Local", 145_000_000)];
        let (fitted, _) = fit(&source, "target", &narrow_limits());
        let ChannelMode::Fm(fm) = &fitted.channels[0].mode else {
            panic!("still an FM channel");
        };
        assert!(fm.tx_tone.is_none());
    }

    #[test]
    fn group_list_members_are_trimmed_and_dangling_references_disappear() {
        let mut source = Codeplug::empty();
        source.contacts = vec![
            Contact {
                name: "One".to_owned(),
                kind: ContactKind::Group,
                number: 1,
                ring: false,
            },
            Contact {
                name: "Two".to_owned(),
                kind: ContactKind::Group,
                number: 2,
                ring: false,
            },
        ];
        source.group_lists = vec![GroupList {
            name: "All".to_owned(),
            contacts: vec!["One".to_owned(), "Two".to_owned(), "Missing".to_owned()],
        }];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert_eq!(fitted.group_lists[0].contacts.len(), 2);
        assert!(report.dropped() >= 1);
    }

    #[test]
    fn one_change_repeated_across_many_entries_is_reported_once_with_a_count() {
        let mut source = Codeplug::empty();
        source.channels = (0..12)
            .map(|index| fm(&format!("Ch{index}"), 145_000_000))
            .collect();
        let (_, report) = fit(&source, "target", &narrow_limits());
        let power: Vec<_> = report
            .issues
            .iter()
            .filter(|issue| issue.field.as_deref() == Some("power"))
            .collect();
        assert_eq!(power.len(), 1);
        assert_eq!(power[0].item.as_deref(), Some("12 entries"));
    }

    #[test]
    fn a_handful_of_changes_still_name_each_entry() {
        let mut source = Codeplug::empty();
        source.channels = (0..3)
            .map(|index| fm(&format!("Ch{index}"), 145_000_000))
            .collect();
        let (_, report) = fit(&source, "target", &narrow_limits());
        let power: Vec<_> = report
            .issues
            .iter()
            .filter(|issue| issue.field.as_deref() == Some("power"))
            .collect();
        assert_eq!(power.len(), 3);
        assert_eq!(power[0].item.as_deref(), Some("Ch0"));
    }

    #[test]
    fn a_codeplug_that_already_fits_reports_nothing() {
        let mut source = Codeplug::empty();
        source.channels = vec![Channel {
            name: "Local".to_owned(),
            rx_hz: 145_000_000,
            tx_hz: 145_000_000,
            power: Power::High,
            mode: ChannelMode::Fm(FmChannel::default()),
            ..Channel::default()
        }];
        let (fitted, report) = fit(&source, "target", &narrow_limits());
        assert_eq!(fitted.channels, source.channels);
        assert!(report.is_clean(), "{:?}", report.issues);
    }
}
