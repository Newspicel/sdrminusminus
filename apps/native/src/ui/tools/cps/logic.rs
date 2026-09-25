use sdrmm_wire::cps::{
    Bandwidth, Channel, ChannelKind, ChannelMode, ContactKind, ConversionIssue, ConversionReport,
    CpsJob, CpsJobKind, CpsJobState, CpsPort, IssueSeverity, Power, RadioModelDescriptor, TimeSlot,
    Tone, Zone,
};

#[must_use]
pub fn zone_channels(zone: &Zone) -> Vec<String> {
    zone.channels_a
        .iter()
        .chain(&zone.channels_b)
        .cloned()
        .collect()
}

#[must_use]
pub fn format_mhz(hz: u64) -> String {
    format!("{:.5}", hz as f64 / 1_000_000.0)
}

#[must_use]
pub fn format_shift(channel: &Channel) -> String {
    let shift = channel.tx_hz as i128 - channel.rx_hz as i128;
    if shift == 0 {
        return "simplex".to_owned();
    }
    let sign = if shift > 0 { "+" } else { "\u{2212}" };
    format!("{sign}{:.4}", shift.unsigned_abs() as f64 / 1_000_000.0)
}

#[must_use]
pub fn format_tone(tone: Option<&Tone>) -> String {
    match tone {
        None => "-".to_owned(),
        Some(Tone::Ctcss { decihertz }) => format!("{:.1}", f64::from(*decihertz) / 10.0),
        Some(Tone::Dcs { code, inverted }) => {
            format!("D{code:03}{}", if *inverted { "I" } else { "N" })
        }
    }
}

#[must_use]
pub fn channel_kind(channel: &Channel) -> ChannelKind {
    channel.mode.kind()
}

#[must_use]
pub fn kind_label(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Fm => "FM",
        ChannelKind::Dmr => "DMR",
    }
}

#[must_use]
pub fn power_label(power: Power) -> &'static str {
    match power {
        Power::Min => "min",
        Power::Low => "low",
        Power::Mid => "mid",
        Power::High => "high",
        Power::Max => "max",
    }
}

#[must_use]
pub fn contact_kind_label(kind: ContactKind) -> &'static str {
    match kind {
        ContactKind::Private => "private",
        ContactKind::Group => "group",
        ContactKind::All => "all",
    }
}

#[must_use]
pub fn channel_detail(channel: &Channel) -> String {
    match &channel.mode {
        ChannelMode::Dmr(dmr) => {
            let slot = if dmr.time_slot == TimeSlot::Two {
                "TS2"
            } else {
                "TS1"
            };
            format!(
                "CC{} {slot} {}",
                dmr.color_code,
                dmr.contact.as_deref().unwrap_or("-")
            )
        }
        ChannelMode::Fm(fm) => {
            let width = if fm.bandwidth == Bandwidth::Wide {
                "25 kHz"
            } else {
                "12.5 kHz"
            };
            format!(
                "{width}  {} / {}",
                format_tone(fm.rx_tone.as_ref()),
                format_tone(fm.tx_tone.as_ref())
            )
        }
    }
}

#[must_use]
pub fn job_percent(job: &CpsJob) -> f64 {
    if job.total_bytes == 0 {
        return if job.state == CpsJobState::Done {
            100.0
        } else {
            0.0
        };
    }
    (job.done_bytes as f64 / job.total_bytes as f64 * 100.0).clamp(0.0, 100.0)
}

#[must_use]
pub fn job_is_active(job: &CpsJob) -> bool {
    matches!(job.state, CpsJobState::Pending | CpsJobState::Running)
}

#[must_use]
pub fn latest_job(jobs: &[CpsJob]) -> Option<&CpsJob> {
    jobs.iter().max_by_key(|job| job.id)
}

#[must_use]
pub fn any_active(jobs: &[CpsJob]) -> bool {
    jobs.iter().any(job_is_active)
}

#[must_use]
pub fn describe_job(job: &CpsJob) -> String {
    let verb = match job.kind {
        CpsJobKind::Read => "Reading",
        CpsJobKind::Write => "Writing",
        CpsJobKind::Identify => "Identifying",
    };
    match job.state {
        CpsJobState::Pending | CpsJobState::Running => {
            format!("{verb} \u{b7} {} \u{b7} {:.0}%", job.step, job_percent(job))
        }
        CpsJobState::Done => format!("{verb} finished"),
        CpsJobState::Cancelled => format!("{verb} cancelled"),
        CpsJobState::Failed => job
            .error
            .clone()
            .unwrap_or_else(|| format!("{verb} failed")),
    }
}

#[must_use]
pub fn candidate_models(
    port: Option<&CpsPort>,
    models: &[RadioModelDescriptor],
) -> Vec<RadioModelDescriptor> {
    let Some(port) = port else {
        return models.to_vec();
    };
    let named = |model: &&RadioModelDescriptor| port.candidate_models.contains(&model.id);
    models
        .iter()
        .filter(named)
        .chain(models.iter().filter(|model| !named(model)))
        .cloned()
        .collect()
}

#[must_use]
pub fn model_label(model: &RadioModelDescriptor) -> String {
    format!("{} {}", model.manufacturer, model.model)
}

#[must_use]
pub fn port_options(ports: &[CpsPort]) -> Vec<(String, String)> {
    ports
        .iter()
        .map(|port| (port.port.clone(), port.label.clone()))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct IssueGroup {
    pub severity: IssueSeverity,
    pub label: &'static str,
    pub issues: Vec<ConversionIssue>,
}

const SEVERITIES: [(IssueSeverity, &str); 3] = [
    (IssueSeverity::Dropped, "Left behind"),
    (IssueSeverity::Adjusted, "Changed to fit"),
    (IssueSeverity::Note, "Worth knowing"),
];

#[must_use]
pub fn group_issues(report: Option<&ConversionReport>) -> Vec<IssueGroup> {
    let Some(report) = report else {
        return Vec::new();
    };
    SEVERITIES
        .iter()
        .map(|(severity, label)| IssueGroup {
            severity: *severity,
            label,
            issues: report
                .issues
                .iter()
                .filter(|issue| issue.severity == *severity)
                .cloned()
                .collect(),
        })
        .filter(|group| !group.issues.is_empty())
        .collect()
}

#[must_use]
pub fn issue_line(issue: &ConversionIssue) -> String {
    let place: Vec<&str> = [issue.item.as_deref(), issue.field.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if place.is_empty() {
        issue.message.clone()
    } else {
        format!("{}: {}", place.join(" \u{b7} "), issue.message)
    }
}

#[must_use]
pub fn report_summary(report: Option<&ConversionReport>) -> String {
    let Some(report) = report else {
        return String::new();
    };
    let dropped = report.dropped();
    let adjusted = report.adjusted();
    if dropped == 0 && adjusted == 0 {
        return "Everything fits".to_owned();
    }
    let mut parts = Vec::new();
    if dropped > 0 {
        parts.push(format!("{dropped} left behind"));
    }
    if adjusted > 0 {
        parts.push(format!("{adjusted} changed"));
    }
    parts.join(", ")
}

#[must_use]
pub fn counts_line(report: &ConversionReport) -> String {
    let (before, after) = (report.before, report.after);
    [
        ("channels", before.channels, after.channels),
        ("contacts", before.contacts, after.contacts),
        ("zones", before.zones, after.zones),
        ("scan lists", before.scan_lists, after.scan_lists),
    ]
    .into_iter()
    .map(|(label, from, to)| {
        if from == to {
            format!("{to} {label}")
        } else {
            format!("{to}/{from} {label}")
        }
    })
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::{
        Admit, CodeplugCounts, DmrChannel, FmChannel, IssueScope, PortMatch, RadioFeatures,
        RadioLimits, UsbMatch,
    };

    use super::*;

    fn fm() -> Channel {
        Channel {
            name: "OE1XUU".to_owned(),
            rx_hz: 438_950_000,
            tx_hz: 431_350_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                rx_tone: Some(Tone::Ctcss { decihertz: 1230 }),
                tx_tone: Some(Tone::Dcs {
                    code: 23,
                    inverted: true,
                }),
                squelch: None,
                admit: Admit::Always,
            }),
        }
    }

    fn dmr() -> Channel {
        Channel {
            name: "TG232".to_owned(),
            rx_hz: 439_000_000,
            tx_hz: 439_000_000,
            power: Power::Low,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Dmr(DmrChannel {
                color_code: 1,
                time_slot: TimeSlot::Two,
                contact: Some("Austria".to_owned()),
                group_list: None,
                radio_id: None,
                admit: Admit::ColorCodeFree,
            }),
        }
    }

    fn job(id: u64, kind: CpsJobKind, state: CpsJobState, total: u64) -> CpsJob {
        CpsJob {
            id,
            kind,
            model_id: "anytone-d890uv".to_owned(),
            port: "/dev/cu.usb".to_owned(),
            state,
            step: "channels".to_owned(),
            done_bytes: 50,
            total_bytes: total,
            started_at: "2026-08-23T00:00:00Z".to_owned(),
            finished_at: None,
            device_id: None,
            codeplug_id: None,
            radio: None,
            report: None,
            error: None,
        }
    }

    fn limits() -> RadioLimits {
        RadioLimits {
            channels: 0,
            contacts: 0,
            group_lists: 0,
            group_list_members: 0,
            zones: 0,
            zone_channels: 0,
            scan_lists: 0,
            scan_list_members: 0,
            radio_ids: 0,
            channel_name_len: 0,
            contact_name_len: 0,
            group_list_name_len: 0,
            zone_name_len: 0,
            scan_list_name_len: 0,
            radio_id_name_len: 0,
            rx_ranges: Vec::new(),
            tx_ranges: Vec::new(),
            powers: Vec::new(),
            modes: Vec::new(),
            frequency_step_hz: 0,
            features: RadioFeatures::default(),
        }
    }

    fn model(id: &str, manufacturer: &str, name: &str, usb: Vec<UsbMatch>) -> RadioModelDescriptor {
        RadioModelDescriptor {
            id: id.to_owned(),
            manufacturer: manufacturer.to_owned(),
            model: name.to_owned(),
            family: id.to_owned(),
            usb,
            needs_explicit_selection: true,
            transfer_bytes: 1,
            limits: limits(),
        }
    }

    fn counts(scan_lists: u32, radio_ids: u32) -> CodeplugCounts {
        CodeplugCounts {
            channels: 35,
            contacts: 1,
            group_lists: 1,
            zones: 4,
            scan_lists,
            radio_ids,
        }
    }

    fn report() -> ConversionReport {
        ConversionReport {
            target_model: "radtel-rt4d".to_owned(),
            source_model: None,
            before: counts(1, 2),
            after: counts(0, 1),
            issues: vec![
                ConversionIssue::new(
                    IssueSeverity::Dropped,
                    IssueScope::ScanList,
                    "this radio has no scan lists",
                )
                .item("scan lists"),
                ConversionIssue::new(
                    IssueSeverity::Adjusted,
                    IssueScope::Channel,
                    "Max is not offered; using High",
                )
                .item("OE1XUU")
                .field("power"),
            ],
        }
    }

    #[test]
    fn a_channel_shows_its_receive_frequency_and_stored_shift() {
        assert_eq!(format_mhz(fm().rx_hz), "438.95000");
        assert_eq!(format_shift(&fm()), "\u{2212}7.6000");
        assert_eq!(format_shift(&dmr()), "simplex");
    }

    #[test]
    fn a_channel_names_its_mode_settings() {
        assert_eq!(channel_kind(&fm()), ChannelKind::Fm);
        assert_eq!(channel_kind(&dmr()), ChannelKind::Dmr);
        assert_eq!(channel_detail(&fm()), "25 kHz  123.0 / D023I");
        assert_eq!(channel_detail(&dmr()), "CC1 TS2 Austria");
    }

    #[test]
    fn a_missing_tone_is_a_dash() {
        assert_eq!(format_tone(None), "-");
        assert_eq!(format_tone(Some(&Tone::Ctcss { decihertz: 885 })), "88.5");
        assert_eq!(
            format_tone(Some(&Tone::Dcs {
                code: 23,
                inverted: false
            })),
            "D023N"
        );
    }

    #[test]
    fn progress_counts_the_bytes_the_radio_owes() {
        assert_eq!(
            job_percent(&job(1, CpsJobKind::Read, CpsJobState::Running, 200)),
            25.0
        );
        assert_eq!(
            job_percent(&job(1, CpsJobKind::Read, CpsJobState::Done, 0)),
            100.0
        );
        assert_eq!(
            job_percent(&job(1, CpsJobKind::Read, CpsJobState::Running, 0)),
            0.0
        );
    }

    #[test]
    fn a_job_says_what_is_happening() {
        assert_eq!(
            describe_job(&job(1, CpsJobKind::Read, CpsJobState::Running, 200)),
            "Reading \u{b7} channels \u{b7} 25%"
        );
        assert_eq!(
            describe_job(&job(1, CpsJobKind::Write, CpsJobState::Done, 200)),
            "Writing finished"
        );
        let mut failed = job(1, CpsJobKind::Read, CpsJobState::Failed, 200);
        failed.error = Some("no answer from the radio".to_owned());
        assert_eq!(describe_job(&failed), "no answer from the radio");
        assert_eq!(
            describe_job(&job(1, CpsJobKind::Read, CpsJobState::Cancelled, 200)),
            "Reading cancelled"
        );
    }

    #[test]
    fn the_newest_job_is_picked_and_a_busy_port_is_known() {
        let jobs = [
            job(1, CpsJobKind::Read, CpsJobState::Done, 200),
            job(4, CpsJobKind::Read, CpsJobState::Running, 200),
        ];
        assert_eq!(latest_job(&jobs).map(|job| job.id), Some(4));
        assert!(any_active(&jobs));
        assert!(!any_active(&[job(
            1,
            CpsJobKind::Read,
            CpsJobState::Done,
            200
        )]));
        assert!(latest_job(&[]).is_none());
    }

    #[test]
    fn models_claiming_the_port_come_first() {
        let models = [
            model("radtel-rt4d", "Radtel", "RT-4D", Vec::new()),
            model(
                "anytone-d890uv",
                "AnyTone",
                "AT-D890UV",
                vec![UsbMatch {
                    vid: 0x0483,
                    pid: 0x5740,
                }],
            ),
        ];
        let port = CpsPort {
            port: "/dev/cu.usb".to_owned(),
            label: "STM32 \u{b7} /dev/cu.usb".to_owned(),
            match_kind: PortMatch::Probable,
            manufacturer: None,
            product: None,
            serial_number: None,
            usb_vid: None,
            usb_pid: None,
            candidate_models: vec!["anytone-d890uv".to_owned()],
        };
        let ids: Vec<String> = candidate_models(Some(&port), &models)
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, ["anytone-d890uv", "radtel-rt4d"]);
        assert_eq!(candidate_models(None, &models).len(), 2);
        assert_eq!(model_label(&models[1]), "AnyTone AT-D890UV");
    }

    #[test]
    fn what_was_lost_is_grouped_above_what_was_changed() {
        let report = report();
        let groups = group_issues(Some(&report));
        let severities: Vec<_> = groups.iter().map(|group| group.severity).collect();
        assert_eq!(
            severities,
            [IssueSeverity::Dropped, IssueSeverity::Adjusted]
        );
        let labels: Vec<_> = groups.iter().map(|group| group.label).collect();
        assert_eq!(labels, ["Left behind", "Changed to fit"]);
        assert!(group_issues(None).is_empty());
    }

    #[test]
    fn each_issue_is_one_readable_line() {
        let report = report();
        assert_eq!(
            issue_line(&report.issues[1]),
            "OE1XUU \u{b7} power: Max is not offered; using High"
        );
        let anonymous = ConversionIssue::new(
            IssueSeverity::Dropped,
            IssueScope::ScanList,
            "this radio has no scan lists",
        );
        assert_eq!(issue_line(&anonymous), "this radio has no scan lists");
    }

    #[test]
    fn the_move_is_summarised_in_one_phrase() {
        let mut report = report();
        assert_eq!(report_summary(Some(&report)), "1 left behind, 1 changed");
        report.issues.clear();
        assert_eq!(report_summary(Some(&report)), "Everything fits");
        assert_eq!(report_summary(None), "");
    }

    #[test]
    fn only_counts_that_moved_show_both_sides() {
        assert_eq!(
            counts_line(&report()),
            "35 channels \u{b7} 1 contacts \u{b7} 4 zones \u{b7} 0/1 scan lists"
        );
    }

    #[test]
    fn a_zone_lists_its_a_channels_then_its_b_channels() {
        let zone = Zone {
            name: "Home".to_owned(),
            channels_a: vec!["A1".to_owned()],
            channels_b: vec!["B1".to_owned(), "B2".to_owned()],
        };
        assert_eq!(zone_channels(&zone), ["A1", "B1", "B2"]);
    }
}
