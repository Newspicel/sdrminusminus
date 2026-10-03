use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const DESCRIPTION_LIMIT: usize = 4096;

#[derive(Serialize)]
struct Payload {
    embeds: [Embed; 1],
}

#[derive(Serialize)]
struct Embed {
    title: String,
    url: String,
    description: String,
}

pub fn payload(version: &str, notes: &Path, out: &Path) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let notes =
        std::fs::read_to_string(notes).with_context(|| format!("read {}", notes.display()))?;
    let url = format!("{REPOSITORY}/releases/tag/v{version}");
    let (description, truncated) = fit(&discord_markdown(&notes), &url);
    if truncated {
        eprintln!("warning: release notes cut to {DESCRIPTION_LIMIT} characters for Discord");
    }
    let payload = Payload {
        embeds: [Embed {
            title: format!("SDR-- {version}"),
            url,
            description,
        }],
    };
    let json = serde_json::to_string(&payload)? + "\n";
    std::fs::write(out, json).with_context(|| format!("write {}", out.display()))
}

fn discord_markdown(notes: &str) -> String {
    notes
        .lines()
        .map(|line| match line.strip_prefix("### ") {
            Some(heading) => format!("**{heading}**"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fit(notes: &str, url: &str) -> (String, bool) {
    let notes = notes.trim_end();
    if notes.chars().count() <= DESCRIPTION_LIMIT {
        return (notes.to_string(), false);
    }
    let more = format!("\n\n[Full notes]({url})");
    let budget = DESCRIPTION_LIMIT - more.chars().count();
    let cut: String = notes.chars().take(budget).collect();
    let kept = cut.rfind('\n').map_or(cut.as_str(), |end| &cut[..end]);
    (format!("{}{more}", kept.trim_end()), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_notes_pass_through() {
        assert_eq!(fit("- a\n", "u"), ("- a".to_string(), false));
    }

    #[test]
    fn long_notes_end_on_a_whole_line_with_a_link() {
        let notes = "- change\n".repeat(1000);
        let (description, truncated) = fit(&notes, "https://x/v1");
        assert!(truncated);
        assert!(description.chars().count() <= DESCRIPTION_LIMIT);
        assert!(description.ends_with("- change\n\n[Full notes](https://x/v1)"));
    }

    #[test]
    fn headings_become_bold() {
        assert_eq!(
            discord_markdown("### Minor changes\n\n- a"),
            "**Minor changes**\n\n- a"
        );
    }
}
