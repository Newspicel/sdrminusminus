use sdrmm_wire::cps::{ConversionReport, IssueSeverity};
use zgui::prelude::*;

use super::{
    Cps,
    logic::{counts_line, group_issues, issue_line, latest_job, report_summary},
};

const SHOWN_ISSUES: usize = 40;

fn tone(severity: IssueSeverity) -> &'static str {
    match severity {
        IssueSeverity::Dropped => "legend tool-danger",
        IssueSeverity::Adjusted => "legend tool-warn",
        IssueSeverity::Note => "legend",
    }
}

pub fn view(cps: Cps) -> impl IntoView {
    move || {
        let report = cps.report.get().or_else(|| {
            cps.jobs.data.with(|data| {
                data.as_ref()
                    .and_then(|data| latest_job(&data.jobs))
                    .and_then(|job| job.report.clone())
            })
        })?;
        Some(AnyView::new(report_box(&report)))
    }
}

fn report_box(report: &ConversionReport) -> impl IntoView {
    let groups: Vec<AnyView> = group_issues(Some(report))
        .into_iter()
        .map(|group| {
            let count = group.issues.len();
            let mut lines: Vec<AnyView> = group
                .issues
                .iter()
                .take(SHOWN_ISSUES)
                .map(|issue| {
                    AnyView::new(view! { text(class = "tool-dim tool-mono") {{issue_line(issue)}} })
                })
                .collect();
            if count > SHOWN_ISSUES {
                let more = format!("and {} more", count - SHOWN_ISSUES);
                lines.push(AnyView::new(view! { text(class = "tool-faint") {{more}} }));
            }
            let title = format!("{} ({count})", group.label);
            AnyView::new(view! {
                column(class = "tool-group") {
                    text(class = tone(group.severity)) {{title}}
                    {lines}
                }
            })
        })
        .collect();
    let title = format!("Fit for {}", report.target_model);
    view! {
        column(class = "cps__report") {
            row(class = "tool-bar") {
                text(class = "legend") {{title}}
                spacer() {}
                text(class = "tool-mono tool-ink") {{report_summary(Some(report))}}
            }
            text(class = "tool-faint tool-mono") {{counts_line(report)}}
            {groups}
        }
    }
}
