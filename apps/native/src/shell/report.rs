use std::fmt::Write as _;

use sdrmm_wire::{diagnostics::DiagnosticsReport, doctor::CheckStatus, patch::PatchGraph};

use crate::shell::toasts::ClientEvent;

pub const MAX_ISSUE_URL: usize = 6000;
pub const MAX_BUNDLE_LOG_LINES: usize = 200;
pub const MAX_TITLE: usize = 120;
pub const PASTE_MARKER: &str =
    "Paste the diagnostics bundle here: it is already on your clipboard.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub nodes: usize,
    pub edges: usize,
    pub kinds: Vec<(String, usize)>,
}

pub struct BundleInput<'a> {
    pub version: &'a str,
    pub client: &'a str,
    pub collected: &'a str,
    pub diagnostics: Option<&'a DiagnosticsReport>,
    pub events: &'a [ClientEvent],
    pub dropped_events: usize,
    pub workspace: Option<&'a WorkspaceFacts>,
}

#[must_use]
pub fn workspace_facts(graph: &PatchGraph) -> WorkspaceFacts {
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for node in &graph.nodes {
        let kind = node.body.kind();
        match kinds.iter_mut().find(|(seen, _)| seen == kind) {
            Some((_, count)) => *count += 1,
            None => kinds.push((kind.to_owned(), 1)),
        }
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    WorkspaceFacts {
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        kinds,
    }
}

#[must_use]
pub fn build_bundle(input: &BundleInput<'_>) -> String {
    [
        Some(environment_section(input)),
        doctor_section(input.diagnostics),
        input.workspace.map(workspace_section),
        server_log_section(input.diagnostics),
        client_log_section(input.events, input.dropped_events),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn or_unknown(value: &str) -> &str {
    if value.is_empty() { "unknown" } else { value }
}

fn environment_section(input: &BundleInput<'_>) -> String {
    let platform = input.diagnostics.map_or("unknown", |diagnostics| {
        diagnostics.doctor.platform.as_str()
    });
    let collected = input.diagnostics.map_or(input.collected, |diagnostics| {
        diagnostics.generated_at.as_str()
    });
    let rows = [
        ("SDR--", or_unknown(input.version)),
        ("Platform", or_unknown(platform)),
        ("Client", or_unknown(input.client)),
        ("Collected", collected),
    ];
    let body = rows
        .iter()
        .map(|(name, value)| format!("| {name} | {value} |"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("### Environment\n\n| | |\n| --- | --- |\n{body}")
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "OK",
        CheckStatus::Warn => "WARN",
        CheckStatus::Fail => "FAIL",
    }
}

fn doctor_section(diagnostics: Option<&DiagnosticsReport>) -> Option<String> {
    let checks = &diagnostics?.doctor.checks;
    if checks.is_empty() {
        return None;
    }
    let lines = checks
        .iter()
        .map(|check| {
            let hint = check
                .hint
                .as_deref()
                .map_or_else(String::new, |hint| format!("\n  hint: {hint}"));
            format!(
                "- **{}** {}: {}{hint}",
                status_label(check.status),
                check.name,
                check.detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("### Diagnostics\n\n{lines}"))
}

fn workspace_section(workspace: &WorkspaceFacts) -> String {
    let kinds = workspace
        .kinds
        .iter()
        .map(|(kind, count)| format!("{kind} \u{d7}{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let tail = if kinds.is_empty() {
        String::new()
    } else {
        format!(": {kinds}")
    };
    format!(
        "### Workspace\n\n{} nodes, {} edges{tail}",
        workspace.nodes, workspace.edges
    )
}

fn server_log_section(diagnostics: Option<&DiagnosticsReport>) -> Option<String> {
    let diagnostics = diagnostics?;
    let log = &diagnostics.log;
    if log.is_empty() {
        return None;
    }
    let shown = &log[log.len().saturating_sub(MAX_BUNDLE_LOG_LINES)..];
    let omitted =
        usize::try_from(diagnostics.dropped).unwrap_or(usize::MAX) + (log.len() - shown.len());
    let lines = shown
        .iter()
        .map(|line| {
            format!(
                "{} {} {}: {}",
                line.at,
                line.level.label(),
                line.target,
                line.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "### Server log{}\n\n```\n{lines}\n```",
        count_note(shown.len(), omitted)
    ))
}

fn client_log_section(events: &[ClientEvent], dropped: usize) -> Option<String> {
    if events.is_empty() {
        return None;
    }
    let lines = events
        .iter()
        .map(|event| {
            format!(
                "{} {} {}: {}",
                event.at,
                event.level.to_uppercase(),
                event.source,
                event.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "### Client log{}\n\n```\n{lines}\n```",
        count_note(events.len(), dropped)
    ))
}

fn count_note(shown: usize, omitted: usize) -> String {
    if omitted > 0 {
        format!(" ({shown} lines, {omitted} older dropped)")
    } else {
        format!(" ({shown} lines)")
    }
}

#[must_use]
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let kept = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if kept {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn issues_base(repository: &str) -> String {
    format!("{}/issues/new", repository.trim_end_matches('/'))
}

fn with_params(base: &str, params: &[(&str, &str)]) -> String {
    let query = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| format!("{name}={}", encode_component(value)))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        base.to_owned()
    } else {
        format!("{base}&{query}")
    }
}

#[must_use]
pub fn bug_issue_url(repository: &str, title: &str, version: &str, bundle: &str) -> String {
    let base = format!("{}?template=bug.yml", issues_base(repository));
    let full = with_params(
        &base,
        &[
            ("title", title),
            ("version", version),
            ("environment", bundle),
        ],
    );
    if full.len() <= MAX_ISSUE_URL {
        return full;
    }
    with_params(
        &base,
        &[
            ("title", title),
            ("version", version),
            ("environment", PASTE_MARKER),
        ],
    )
}

#[must_use]
pub fn feature_issue_url(repository: &str, version: &str) -> String {
    with_params(
        &format!("{}?template=feature.yml", issues_base(repository)),
        &[("version", version)],
    )
}

#[must_use]
pub fn issue_title(seed: &str) -> String {
    let collapsed = seed.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_TITLE {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(MAX_TITLE - 1).collect();
    cut.push('\u{2026}');
    cut
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        diagnostics::{LogLevel, LogLine},
        doctor::{DoctorCheck, DoctorReport},
        patch::{ChannelNode, DeviceNode, NodeBody, PatchEdge, PatchNode, PortRef, Position},
    };

    use super::*;

    const REPO: &str = "https://github.com/Newspicel/sdrminusminus";

    fn diagnostics(log: Vec<LogLine>, dropped: u64) -> DiagnosticsReport {
        DiagnosticsReport {
            generated_at: "2026-09-16T10:00:00Z".to_owned(),
            doctor: DoctorReport {
                version: "0.4.0".to_owned(),
                platform: "macos/aarch64".to_owned(),
                checks: vec![
                    DoctorCheck {
                        id: "backends".to_owned(),
                        name: "Device backends".to_owned(),
                        status: CheckStatus::Warn,
                        detail: "compiled backends: virtual".to_owned(),
                        hint: Some("rebuild with --features soapy".to_owned()),
                    },
                    DoctorCheck {
                        id: "storage.db".to_owned(),
                        name: "Database".to_owned(),
                        status: CheckStatus::Ok,
                        detail: "~/Library/sdrmm.db".to_owned(),
                        hint: None,
                    },
                ],
            },
            log,
            dropped,
        }
    }

    fn line(message: &str) -> LogLine {
        LogLine {
            at: "2026-09-16T09:59:00Z".to_owned(),
            level: LogLevel::Warn,
            target: "sdrmm_engine".to_owned(),
            message: message.to_owned(),
        }
    }

    fn events() -> Vec<ClientEvent> {
        vec![ClientEvent {
            at: "2026-09-16T09:59:30Z".to_owned(),
            level: "error",
            source: "toast",
            message: "[engine] boom".to_owned(),
        }]
    }

    fn input<'a>(
        diagnostics: Option<&'a DiagnosticsReport>,
        events: &'a [ClientEvent],
        dropped_events: usize,
        workspace: Option<&'a WorkspaceFacts>,
    ) -> BundleInput<'a> {
        BundleInput {
            version: "0.4.0",
            client: "native macos/aarch64",
            collected: "now",
            diagnostics,
            events,
            dropped_events,
            workspace,
        }
    }

    fn node(id: &str, body: NodeBody, label: Option<&str>) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 1.0, y: 2.0 },
            size: None,
            label: label.map(str::to_owned),
        }
    }

    fn channel() -> NodeBody {
        NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
            record_calls: false,
            tuning_locked: false,
        })
    }

    #[test]
    fn counts_kinds_without_carrying_names_or_positions() {
        let graph = PatchGraph {
            nodes: vec![
                node(
                    "device-1",
                    NodeBody::Device(DeviceNode::default()),
                    Some("Home RTL"),
                ),
                node("channel-1", channel(), Some("145.5")),
                node("channel-2", channel(), None),
            ],
            edges: vec![PatchEdge {
                from: PortRef {
                    node: "device-1".to_owned(),
                    port: "iq".to_owned(),
                },
                to: PortRef {
                    node: "channel-1".to_owned(),
                    port: "iq".to_owned(),
                },
            }],
        };
        let facts = workspace_facts(&graph);
        assert_eq!(
            facts,
            WorkspaceFacts {
                nodes: 3,
                edges: 1,
                kinds: vec![("channel".to_owned(), 2), ("device".to_owned(), 1)],
            }
        );
        assert!(!format!("{facts:?}").contains("Home RTL"));
    }

    #[test]
    fn carries_the_environment_the_checks_and_both_logs() {
        let report = diagnostics(vec![line("device open failed")], 0);
        let facts = WorkspaceFacts {
            nodes: 3,
            edges: 1,
            kinds: vec![("device".to_owned(), 1)],
        };
        let events = events();
        let bundle = build_bundle(&input(Some(&report), &events, 0, Some(&facts)));
        assert!(bundle.contains("| SDR-- | 0.4.0 |"));
        assert!(bundle.contains("| Platform | macos/aarch64 |"));
        assert!(bundle.contains("**WARN** Device backends"));
        assert!(bundle.contains("hint: rebuild with --features soapy"));
        assert!(bundle.contains("3 nodes, 1 edges: device \u{d7}1"));
        assert!(bundle.contains("device open failed"));
        assert!(bundle.contains("[engine] boom"));
    }

    #[test]
    fn leaves_out_the_workspace_when_it_is_not_offered() {
        let report = diagnostics(vec![line("x")], 0);
        let bundle = build_bundle(&input(Some(&report), &[], 0, None));
        assert!(!bundle.contains("### Workspace"));
        assert!(!bundle.contains("### Client log"));
    }

    #[test]
    fn still_reports_when_the_server_could_not_be_reached() {
        let events = events();
        let bundle = build_bundle(&input(None, &events, 2, None));
        assert!(bundle.contains("| Platform | unknown |"));
        assert!(!bundle.contains("### Diagnostics"));
        assert!(bundle.contains("### Client log (1 lines, 2 older dropped)"));
    }

    #[test]
    fn caps_the_server_log_and_says_what_it_left_out() {
        let log = (0..MAX_BUNDLE_LOG_LINES + 40)
            .map(|at| line(&format!("line {at}")))
            .collect();
        let report = diagnostics(log, 7);
        let bundle = build_bundle(&input(Some(&report), &[], 0, None));
        assert!(bundle.contains(&format!(
            "### Server log ({MAX_BUNDLE_LOG_LINES} lines, 47 older dropped)"
        )));
        assert!(!bundle.contains("line 39\n"));
        assert!(bundle.contains(&format!("line {}", MAX_BUNDLE_LOG_LINES + 39)));
    }

    #[test]
    fn prefills_the_bug_form_with_the_bundle_when_it_fits() {
        let url = bug_issue_url(REPO, "Device open failed", "0.4.0", "short bundle");
        assert!(url.starts_with(&format!("{REPO}/issues/new?template=bug.yml&")));
        assert!(url.contains("title=Device%20open%20failed"));
        assert!(url.contains("version=0.4.0"));
        assert!(url.contains("environment=short%20bundle"));
    }

    #[test]
    fn falls_back_to_a_paste_marker_rather_than_a_url_github_would_reject() {
        let url = bug_issue_url(REPO, "Big one", "0.4.0", &"x".repeat(20_000));
        assert!(url.len() <= MAX_ISSUE_URL);
        assert!(url.contains(&encode_component(PASTE_MARKER)));
    }

    #[test]
    fn tolerates_a_trailing_slash_and_drops_empty_parameters() {
        assert_eq!(
            feature_issue_url(&format!("{REPO}/"), "0.4.0"),
            format!("{REPO}/issues/new?template=feature.yml&version=0.4.0")
        );
        assert_eq!(
            feature_issue_url(REPO, ""),
            format!("{REPO}/issues/new?template=feature.yml")
        );
    }

    #[test]
    fn collapses_whitespace_and_caps_the_title() {
        assert_eq!(
            issue_title("  device   open\nfailed "),
            "device open failed"
        );
        assert_eq!(issue_title(""), "");
        assert_eq!(issue_title(&"y".repeat(200)).chars().count(), MAX_TITLE);
    }
}
