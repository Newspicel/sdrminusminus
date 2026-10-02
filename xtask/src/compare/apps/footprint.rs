use std::process::Command;

use anyhow::{Context, Result, ensure};

const EACH: &str = "]: ";
const LABEL: &str = "Footprint: ";
const SUMMARY: &str = "Summary Footprint: ";

pub fn bytes(pids: &[u32]) -> Result<u64> {
    let mut command = Command::new("footprint");
    command.args(["-f", "bytes", "--noCategories"]);
    for pid in pids {
        command.arg("-p").arg(pid.to_string());
    }
    let out = command.output().context("run footprint")?;
    ensure!(
        out.status.success(),
        "footprint failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    total(&String::from_utf8(out.stdout)?, pids)
}

fn total(text: &str, pids: &[u32]) -> Result<u64> {
    let each: Vec<(u32, u64)> = text.lines().filter_map(entry).collect();
    let missing: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|pid| each.iter().all(|(read, _)| read != pid))
        .collect();
    ensure!(missing.is_empty(), "footprint could not read {missing:?}");
    Ok(text
        .lines()
        .find_map(|line| value(line, SUMMARY))
        .unwrap_or_else(|| each.iter().map(|(_, bytes)| bytes).sum()))
}

fn entry(line: &str) -> Option<(u32, u64)> {
    let (head, rest) = line.split_once(EACH)?;
    let pid = head.rsplit_once('[')?.1.parse().ok()?;
    Some((pid, value(rest, LABEL)?))
}

fn value(text: &str, label: &str) -> Option<u64> {
    text.split_once(label)?
        .1
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "\
======
Google Chrome Helper [46331]: 64-bit    Footprint: 600 B (16384 bytes per page)
======
Auxiliary data:
    phys_footprint: 600 B
======
Google Chrome [46326]: 64-bit    Footprint: 100 B (16384 bytes per page)
======
Summary Footprint: 650 B
";

    #[test]
    fn several_processes_use_the_summary() {
        assert_eq!(total(TWO, &[46331, 46326]).unwrap(), 650);
    }

    #[test]
    fn one_process_uses_its_own_footprint() {
        let one = "sdrmm [7]: 64-bit    Footprint: 42 B (16384 bytes per page)\n";
        assert_eq!(total(one, &[7]).unwrap(), 42);
    }

    #[test]
    fn a_process_it_could_not_read_is_an_error() {
        let err = total(TWO, &[46331, 46326, 9]).unwrap_err();
        assert!(err.to_string().contains("[9]"));
    }
}
