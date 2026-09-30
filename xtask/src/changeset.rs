use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;

const DIR: &str = ".changeset";
const GUIDE: &str = "README.md";
const CHANGELOG: &str = "CHANGELOG.md";
const TITLE: &str = "# Changelog\n";
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    fn name(self) -> &'static str {
        match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Patch, Self::Minor, Self::Major]
            .into_iter()
            .find(|bump| bump.name() == text)
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Major => "Breaking changes",
            Self::Minor => "Features",
            Self::Patch => "Fixes",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    bump: Bump,
    summary: String,
    commit: Option<String>,
}

struct Pending {
    path: PathBuf,
    change: Change,
}

pub fn add(root: &Path, bump: Bump, summary: &str) -> Result<()> {
    let summary = summary.trim();
    ensure!(!summary.is_empty(), "a changeset needs a summary");
    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{}.md", slug(summary)));
    ensure!(!path.exists(), "{} already exists", path.display());
    let text = format!("---\nbump: {}\n---\n\n{summary}\n", bump.name());
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

pub fn check(root: &Path) -> Result<()> {
    load(root).map(drop)
}

pub fn release(root: &Path, dry_run: bool) -> Result<()> {
    let mut pending = load(root)?;
    ensure!(
        !pending.is_empty(),
        "nothing to release: {DIR} holds no changesets. Add one with `cargo xtask changeset`"
    );
    for entry in &mut pending {
        entry.change.commit = added_in(root, &entry.path)?;
    }
    let bump = pending
        .iter()
        .map(|entry| entry.change.bump)
        .max()
        .context("no changesets")?;
    let version = next_version(&latest_version(root)?, bump)?;
    let changes: Vec<Change> = pending.iter().map(|entry| entry.change.clone()).collect();
    let section = render(&version, &today()?, &changes);
    if dry_run {
        print!("{section}");
        return Ok(());
    }
    ensure_clean(root)?;
    write_changelog(root, &section)?;
    for entry in &pending {
        std::fs::remove_file(&entry.path)
            .with_context(|| format!("remove {}", entry.path.display()))?;
    }
    commit_and_tag(root, &version)?;
    println!("tagged v{version}. Publish with `git push --atomic origin main v{version}`");
    Ok(())
}

pub fn notes(root: &Path, version: &str, out: Option<&Path>) -> Result<()> {
    let path = root.join(CHANGELOG);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {CHANGELOG}"))?;
    let notes = section(&text, version.strip_prefix('v').unwrap_or(version))?;
    match out {
        Some(out) => {
            std::fs::write(out, notes).with_context(|| format!("write {}", out.display()))?;
        }
        None => print!("{notes}"),
    }
    Ok(())
}

fn load(root: &Path) -> Result<Vec<Pending>> {
    let dir = root.join(DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        let markdown = path.extension().is_some_and(|ext| ext == "md");
        if markdown && path.file_name().is_some_and(|name| name != GUIDE) {
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let change = parse(&text).with_context(|| format!("{}", path.display()))?;
            Ok(Pending { path, change })
        })
        .collect()
}

fn parse(text: &str) -> Result<Change> {
    let text = text.replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .context("starts without a `---` front matter block")?;
    let (front, body) = rest
        .split_once("\n---\n")
        .context("front matter has no closing `---`")?;
    let mut bump = None;
    for line in front.lines().filter(|line| !line.trim().is_empty()) {
        let (key, value) = line
            .split_once(':')
            .with_context(|| format!("`{line}` is not `key: value`"))?;
        match key.trim() {
            "bump" => {
                let value = value.trim().trim_matches('"');
                bump = Some(Bump::parse(value).with_context(|| {
                    format!("bump `{value}` is not one of patch, minor, major")
                })?);
            }
            other => bail!("unknown front matter key `{other}`"),
        }
    }
    let summary = body.trim();
    ensure!(!summary.is_empty(), "has no summary");
    Ok(Change {
        bump: bump.context("front matter has no `bump`")?,
        summary: summary.to_string(),
        commit: None,
    })
}

fn slug(summary: &str) -> String {
    let words: Vec<String> = summary
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(6)
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "change".to_string()
    } else {
        words.join("-")
    }
}

fn render(version: &str, date: &str, changes: &[Change]) -> String {
    let mut out = format!("## {version} ({date})\n");
    for bump in [Bump::Major, Bump::Minor, Bump::Patch] {
        let group: Vec<&Change> = changes.iter().filter(|c| c.bump == bump).collect();
        if group.is_empty() {
            continue;
        }
        let _ = write!(out, "\n### {}\n\n", bump.heading());
        for change in group {
            out.push_str(&item(change));
        }
    }
    out
}

fn item(change: &Change) -> String {
    let summary = match &change.commit {
        Some(hash) => with_link(&change.summary, hash),
        None => change.summary.clone(),
    };
    let mut out = String::new();
    for (index, line) in summary.lines().enumerate() {
        let prefix = match (index, line.is_empty()) {
            (0, _) => "- ",
            (_, true) => "",
            _ => "  ",
        };
        let _ = writeln!(out, "{prefix}{line}");
    }
    out
}

fn with_link(summary: &str, hash: &str) -> String {
    let short = hash.get(..7).unwrap_or(hash);
    let link = format!(" ([{short}]({REPOSITORY}/commit/{hash}))");
    match summary.split_once("\n\n") {
        Some((first, rest)) => format!("{first}{link}\n\n{rest}"),
        None => format!("{summary}{link}"),
    }
}

fn section(changelog: &str, version: &str) -> Result<String> {
    let heading = format!("## {version}");
    let mut lines = changelog.lines();
    lines
        .by_ref()
        .find(|line| {
            line.strip_prefix(&heading)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        })
        .with_context(|| format!("{CHANGELOG} has no `{heading}` section"))?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    let body = body.join("\n");
    let body = body.trim();
    ensure!(
        !body.is_empty(),
        "the `{heading}` section in {CHANGELOG} is empty"
    );
    Ok(format!("{body}\n"))
}

fn prepend(changelog: &str, section: &str) -> String {
    let rest = changelog
        .strip_prefix(TITLE)
        .unwrap_or(changelog)
        .trim_start();
    if rest.is_empty() {
        format!("{TITLE}\n{section}")
    } else {
        format!("{TITLE}\n{section}\n{rest}")
    }
}

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

fn next_version(previous: &str, bump: Bump) -> Result<String> {
    let [major, minor, patch] = parse_version(previous)
        .with_context(|| format!("`{previous}` is not a major.minor.patch version"))?;
    let [major, minor, patch] = match bump {
        Bump::Major => [major + 1, 0, 0],
        Bump::Minor => [major, minor + 1, 0],
        Bump::Patch => [major, minor, patch + 1],
    };
    Ok(format!("{major}.{minor}.{patch}"))
}

fn latest_version(root: &Path) -> Result<String> {
    let tags = git(root, &["tag", "--list", "v*", "--sort=-v:refname"])?;
    Ok(tags
        .lines()
        .filter_map(|tag| tag.strip_prefix('v'))
        .find(|version| parse_version(version).is_some())
        .unwrap_or("0.0.0")
        .to_string())
}

fn added_in(root: &Path, path: &Path) -> Result<Option<String>> {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let hash = git(
        root,
        &["log", "--diff-filter=A", "--format=%H", "-1", "--", &rel],
    )?;
    let hash = hash.trim();
    Ok((!hash.is_empty()).then(|| hash.to_string()))
}

fn ensure_clean(root: &Path) -> Result<()> {
    let status = git(root, &["status", "--porcelain"])?;
    ensure!(
        status.trim().is_empty(),
        "the working tree has uncommitted changes. Commit them first, so every changeset links \
         to the commit that added it:\n{status}"
    );
    Ok(())
}

fn write_changelog(root: &Path, section: &str) -> Result<()> {
    let path = root.join(CHANGELOG);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).context(format!("read {CHANGELOG}")),
    };
    std::fs::write(&path, prepend(&existing, section)).with_context(|| format!("write {CHANGELOG}"))
}

fn commit_and_tag(root: &Path, version: &str) -> Result<()> {
    git(root, &["add", "--all", "--", CHANGELOG, DIR])?;
    git(
        root,
        &["commit", "--message", &format!("Release {version}")],
    )?;
    git(
        root,
        &[
            "tag",
            "--annotate",
            &format!("v{version}"),
            "--message",
            &format!("v{version}"),
        ],
    )?;
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("failed to spawn git")?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("git printed non UTF-8 output")
}

fn today() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("the clock is before 1970")?
        .as_secs();
    let (year, month, day) = civil(i64::try_from(seconds / 86_400)?);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(bump: Bump, summary: &str, commit: Option<&str>) -> Change {
        Change {
            bump,
            summary: summary.to_string(),
            commit: commit.map(str::to_string),
        }
    }

    #[test]
    fn parses_front_matter_and_summary() {
        let parsed = parse("---\nbump: minor\n---\n\nAdds a thing.\n").expect("parses");
        assert_eq!(parsed, change(Bump::Minor, "Adds a thing.", None));
        let quoted = parse("---\r\nbump: \"patch\"\r\n---\r\nFix.\r\n").expect("parses");
        assert_eq!(quoted.bump, Bump::Patch);
    }

    #[test]
    fn rejects_broken_changesets() {
        for text in [
            "Adds a thing.",
            "---\nbump: minor\n",
            "---\nbump: huge\n---\nText",
            "---\nkind: minor\n---\nText",
            "---\n---\nText",
            "---\nbump: minor\n---\n\n",
        ] {
            assert!(parse(text).is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn slugs_follow_the_summary() {
        assert_eq!(
            slug("RTL-SDR: fix gain after reconnect on macOS 15"),
            "rtl-sdr-fix-gain-after-reconnect"
        );
        assert_eq!(slug("…"), "change");
    }

    #[test]
    fn bumps_from_the_previous_version() {
        assert_eq!(next_version("1.9.0", Bump::Patch).expect("bumps"), "1.9.1");
        assert_eq!(next_version("1.9.3", Bump::Minor).expect("bumps"), "1.10.0");
        assert_eq!(next_version("1.9.3", Bump::Major).expect("bumps"), "2.0.0");
        assert!(next_version("1.9", Bump::Patch).is_err());
        assert!(parse_version("1.2.3.4").is_none());
    }

    #[test]
    fn renders_groups_in_order_with_links() {
        let changes = [
            change(Bump::Patch, "Fix A.", None),
            change(
                Bump::Minor,
                "Add B.\n\n```yaml\nb: 1\n```",
                Some("abcdef123456"),
            ),
            change(Bump::Major, "Drop C.", None),
        ];
        let expected = format!(
            "## 2.0.0 (2026-10-01)\n\n### Breaking changes\n\n- Drop C.\n\n### Features\n\n\
             - Add B. ([abcdef1]({REPOSITORY}/commit/abcdef123456))\n\n  ```yaml\n  b: 1\n  ```\n\n\
             ### Fixes\n\n- Fix A.\n"
        );
        assert_eq!(render("2.0.0", "2026-10-01", &changes), expected);
    }

    #[test]
    fn prepends_below_the_title() {
        assert_eq!(prepend("", "## 1.0.0\n"), "# Changelog\n\n## 1.0.0\n");
        assert_eq!(
            prepend("# Changelog\n\n## 1.0.0\n", "## 1.1.0\n"),
            "# Changelog\n\n## 1.1.0\n\n## 1.0.0\n"
        );
    }

    #[test]
    fn extracts_one_release() {
        let log = "# Changelog\n\n## 1.10.0 (2026-10-01)\n\n### Fixes\n\n- B.\n\n## 1.1.0 (2026-01-01)\n\n- A.\n";
        assert_eq!(
            section(log, "1.10.0").expect("found"),
            "### Fixes\n\n- B.\n"
        );
        assert_eq!(section(log, "1.1.0").expect("found"), "- A.\n");
        assert!(section(log, "1.1").is_err());
        assert!(section("# Changelog\n\n## 1.2.0\n", "1.2.0").is_err());
    }

    #[test]
    fn converts_days_to_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_727), (2026, 10, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }
}
