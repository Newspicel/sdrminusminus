use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const SELF: &str = "SDR--";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    pub machine: Machine,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub cpu: String,
    pub os: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub unit: String,
    pub better: Better,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub results: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Better {
    Higher,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub tool: String,
    pub version: String,
    pub value: f64,
}

impl Group {
    pub fn new(id: &str, title: &str, unit: &str, better: Better) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            unit: unit.to_owned(),
            better,
            note: None,
            results: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    pub fn push(&mut self, tool: &str, version: &str, value: f64) {
        self.results.push(Entry {
            tool: tool.to_owned(),
            version: version.to_owned(),
            value,
        });
    }
}

pub fn machine() -> Result<Machine> {
    Ok(Machine {
        cpu: cpu()?,
        os: os()?,
        date: output("date", &["-u", "+%F"])?,
    })
}

pub fn version(root: &Path) -> Result<String> {
    let root = root.to_string_lossy();
    let described = output("git", &["-C", &root, "describe", "--tags", "--dirty"])?;
    Ok(described.strip_prefix('v').unwrap_or(&described).to_owned())
}

pub fn publish(root: &Path, name: &str, suite: Suite, ours: bool) -> Result<()> {
    if !ours {
        return write(root, name, &suite);
    }
    let text = std::fs::read_to_string(path(root, name))
        .with_context(|| format!("read the published {name} data"))?;
    let old: Suite = serde_json::from_str(&text)?;
    write(root, name, &replace_ours(old, suite)?)
}

fn replace_ours(old: Suite, new: Suite) -> Result<Suite> {
    ensure!(
        old.machine.cpu == new.machine.cpu && old.machine.os == new.machine.os,
        "the published data is from {} on {}",
        old.machine.cpu,
        old.machine.os
    );
    let mut groups = Vec::with_capacity(old.groups.len());
    for mut group in old.groups {
        let fresh = new
            .groups
            .iter()
            .find(|fresh| fresh.id == group.id)
            .with_context(|| format!("{} was not measured", group.id))?;
        group.results.retain(|entry| entry.tool != SELF);
        group.results.extend(
            fresh
                .results
                .iter()
                .filter(|entry| entry.tool == SELF)
                .cloned(),
        );
        groups.push(group);
    }
    Ok(Suite {
        machine: new.machine,
        groups,
    })
}

fn path(root: &Path, name: &str) -> PathBuf {
    root.join("site/src/data/bench")
        .join(format!("{name}.json"))
}

pub fn write(root: &Path, name: &str, suite: &Suite) -> Result<()> {
    validate(suite)?;
    let path = path(root, name);
    let mut text = serde_json::to_string_pretty(suite)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn validate(suite: &Suite) -> Result<()> {
    for group in &suite.groups {
        ensure!(!group.results.is_empty(), "{} has no results", group.id);
        ensure!(
            group.results.iter().any(|entry| entry.tool == SELF),
            "{} has no {SELF} result",
            group.id
        );
        for entry in &group.results {
            ensure!(
                entry.value.is_finite() && entry.value >= 0.0,
                "{} {} measured {}",
                group.id,
                entry.tool,
                entry.value
            );
        }
    }
    Ok(())
}

fn cpu() -> Result<String> {
    if cfg!(target_os = "macos") {
        return output("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    let info = std::fs::read_to_string("/proc/cpuinfo").context("read /proc/cpuinfo")?;
    info.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name.trim().to_owned())
        .context("no model name in /proc/cpuinfo")
}

fn os() -> Result<String> {
    if cfg!(target_os = "macos") {
        return Ok(format!(
            "macOS {}",
            output("sw_vers", &["-productVersion"])?
        ));
    }
    output("uname", &["-sr"])
}

pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    ensure!(out.status.success(), "{program} {args:?} failed");
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite(tool: &str, value: f64) -> Suite {
        let mut group = Group::new("fir", "FIR", "Msps", Better::Higher);
        group.push(tool, "1", value);
        Suite {
            machine: Machine {
                cpu: "cpu".into(),
                os: "os".into(),
                date: "2026-10-01".into(),
            },
            groups: vec![group],
        }
    }

    #[test]
    fn a_group_without_our_result_is_refused() {
        assert!(validate(&suite("liquid-dsp", 1.0)).is_err());
        assert!(validate(&suite(SELF, 1.0)).is_ok());
    }

    #[test]
    fn ours_replaces_only_our_results() {
        let mut old = suite(SELF, 1.0);
        old.groups[0].push("liquid-dsp", "1", 5.0);
        let mut new = suite(SELF, 2.0);
        new.machine.date = "2026-10-02".into();
        let merged = replace_ours(old, new).expect("same machine");
        let values: Vec<_> = merged.groups[0]
            .results
            .iter()
            .map(|entry| (entry.tool.as_str(), entry.value))
            .collect();
        assert_eq!(values, [("liquid-dsp", 5.0), (SELF, 2.0)]);
        assert_eq!(merged.machine.date, "2026-10-02");
    }

    #[test]
    fn ours_refuses_another_machine_or_a_missing_group() {
        let mut other = suite(SELF, 2.0);
        other.machine.cpu = "other".into();
        assert!(replace_ours(suite(SELF, 1.0), other).is_err());
        let mut empty = suite(SELF, 2.0);
        empty.groups.clear();
        assert!(replace_ours(suite(SELF, 1.0), empty).is_err());
    }

    #[test]
    fn a_broken_measurement_is_refused() {
        assert!(validate(&suite(SELF, f64::NAN)).is_err());
        assert!(validate(&suite(SELF, -1.0)).is_err());
    }

    #[test]
    fn better_is_written_in_lowercase() {
        let text = serde_json::to_string(&suite(SELF, 1.0)).expect("json");
        assert!(text.contains("\"better\":\"higher\""));
    }
}
