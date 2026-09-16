use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

const BANNED: &[(&str, &str)] = &[
    ("Msps", "MS/s"),
    ("msps", "S/s"),
    ("ksps", "kS/s"),
    ("Ksps", "kS/s"),
    ("MSPS", "MS/s"),
    ("sps", "S/s"),
    ("Sps", "S/s"),
    ("SPS", "S/s"),
    ("Msamples/s", "MS/s"),
    ("ksamples/s", "kS/s"),
    ("samples/s", "S/s"),
    ("MSa/s", "MS/s"),
    ("kSa/s", "kS/s"),
    ("Sa/s", "S/s"),
    ("Sa/sym", "S/sym"),
    ("bps", "bit/s"),
    ("Bps", "bit/s"),
    ("kbps", "kbit/s"),
    ("Kbps", "kbit/s"),
    ("Mbps", "Mbit/s"),
    ("Gbps", "Gbit/s"),
    ("KHz", "kHz"),
    ("Khz", "kHz"),
    ("Mhz", "MHz"),
    ("MHZ", "MHz"),
    ("Ghz", "GHz"),
    ("GHZ", "GHz"),
    ("HZ", "Hz"),
    ("KB", "kB"),
    ("Kb", "kB"),
    ("dbm", "dBm"),
    ("dBM", "dBm"),
    ("DB", "dB"),
    ("db", "dB"),
    ("uS", "µs"),
    ("uV", "µV"),
    ("uA", "µA"),
    ("mS", "ms"),
    ("sec", "s"),
    ("Sec", "s"),
    ("secs", "s"),
    ("mins", "min"),
    ("hrs", "h"),
];

const DIRECTORIES: &[&str] = &["crates", "apps", "xtask", "web/src", "web/e2e"];

const EXTENSIONS: &[&str] = &["rs", "ts", "tsx"];

const CHECKER: &str = "xtask/src/units.rs";

pub(crate) fn check(root: &Path) -> Result<()> {
    let mut offences = Vec::new();
    for directory in DIRECTORIES {
        for source in sources(&root.join(directory))? {
            let relative = source.strip_prefix(root).unwrap_or(&source).to_owned();
            if relative.ends_with(CHECKER) {
                continue;
            }
            let text = std::fs::read_to_string(&source)
                .with_context(|| format!("read {}", source.display()))?;
            offences.extend(scan(&text).into_iter().map(|(line, found, si)| {
                format!("{}:{line}: `{found}` should be `{si}`", relative.display())
            }));
        }
    }
    ensure!(
        offences.is_empty(),
        "these strings reach a reader in units the SI does not use. Format a quantity with \
         `sdrmm_wire::units` or the web `si` helper instead of spelling a prefix by hand:\n{}",
        offences.join("\n")
    );
    Ok(())
}

fn scan(text: &str) -> Vec<(usize, String, &'static str)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for literal in literals(line) {
            for (banned, si) in BANNED {
                if rendered(literal, banned) {
                    found.push((index + 1, (*banned).to_owned(), *si));
                }
            }
        }
    }
    found
}

fn rendered(literal: &str, banned: &str) -> bool {
    let bytes = literal.as_bytes();
    let mut at = 0;
    while let Some(offset) = literal[at..].find(banned) {
        let start = at + offset;
        let end = start + banned.len();
        at = start + 1;
        let before = literal[..start].chars().next_back();
        let after = bytes.get(end).copied().map(char::from);
        let word = before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
            && after.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if word && quantity(&literal[..start]) {
            return true;
        }
    }
    false
}

fn quantity(before: &str) -> bool {
    let trimmed = before.trim_end();
    trimmed.len() < before.len() && (trimmed.ends_with(['}', ')']) || ends_with_digit(trimmed))
}

fn ends_with_digit(text: &str) -> bool {
    text.chars().next_back().is_some_and(|c| c.is_ascii_digit())
}

fn literals(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find(['"', '`']) {
        let quote = rest.as_bytes()[open] as char;
        let body = &rest[open + 1..];
        let Some(close) = body.find(quote) else {
            break;
        };
        out.push(&body[..close]);
        rest = &body[close + 1..];
    }
    out
}

fn sources(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("read {}", dir.display()))?
            .path();
        if path.is_dir() {
            out.extend(sources(&path)?);
        } else if path
            .extension()
            .is_some_and(|e| EXTENSIONS.iter().any(|allowed| e == *allowed))
        {
            out.push(path);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offences(text: &str) -> Vec<String> {
        scan(text)
            .into_iter()
            .map(|(_, found, _)| found)
            .collect::<Vec<_>>()
    }

    #[test]
    fn a_rate_spelled_in_samples_per_second_the_si_way_passes() {
        assert!(offences(r#"format!("{rate} MS/s")"#).is_empty());
        assert!(offences(r#"`${formatSampleRate(rate)}`"#).is_empty());
        assert!(offences(r#"format!("{hz} kHz · {snr} dB · {rate} kbit/s")"#).is_empty());
    }

    #[test]
    fn a_rate_spelled_any_other_way_fails() {
        assert_eq!(offences(r#"format!("{rate:.3} Msps")"#), ["Msps"]);
        assert_eq!(offences("`${rate} MSa/s`"), ["MSa/s"]);
        assert_eq!(offences(r#""2.048 Msps""#), ["Msps"]);
        assert_eq!(offences("`${bits} kbps`"), ["kbps"]);
        assert_eq!(offences(r#""145.5 Mhz""#), ["Mhz"]);
        assert_eq!(offences(r#""32 KB""#), ["KB"]);
    }

    #[test]
    fn a_unit_token_that_is_not_a_reading_is_left_alone() {
        assert!(offences(r#"let msps = throughput();"#).is_empty());
        assert!(offences(r#"const SPS: usize = 8;"#).is_empty());
        assert!(offences(r#"match unit { "khz" => 1e3, "mhz" => 1e6 }"#).is_empty());
        assert!(offences(r#""samples per second""#).is_empty());
        assert!(offences(r#""bitrate_kbps""#).is_empty());
    }

    #[test]
    fn a_reading_is_caught_wherever_the_number_comes_from() {
        assert_eq!(offences(r#""at 8 Msps or more""#), ["Msps"]);
        assert_eq!(offences(r#"format!("{} Sa/s", rate)"#), ["Sa/s"]);
        assert_eq!(offences("`${(hz / 1e6).toFixed(3)} Mhz`"), ["Mhz"]);
    }
}
