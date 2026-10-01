use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub cpu_percent: f64,
    pub peak_rss_mib: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Process {
    pid: u32,
    ppid: u32,
    rss_kib: u64,
    cpu: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Tick {
    at: Instant,
    cpu: Duration,
    rss_kib: u64,
}

pub fn settle(pid: u32, warmup: Duration) -> Result<()> {
    let end = Instant::now() + warmup;
    while Instant::now() < end {
        tick(pid)?;
        thread::sleep(TICK);
    }
    Ok(())
}

pub fn measure(pid: u32, window: Duration) -> Result<Usage> {
    let start = Instant::now();
    let mut ticks = vec![tick(pid)?];
    while start.elapsed() < window {
        thread::sleep(TICK);
        ticks.push(tick(pid)?);
    }
    usage(&ticks)
}

pub fn median(runs: &[Usage]) -> Result<Usage> {
    ensure!(!runs.is_empty(), "no runs");
    let middle = |pick: fn(&Usage) -> f64| {
        let mut values: Vec<f64> = runs.iter().map(pick).collect();
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    Ok(Usage {
        cpu_percent: middle(|usage| usage.cpu_percent),
        peak_rss_mib: middle(|usage| usage.peak_rss_mib),
    })
}

fn tick(pid: u32) -> Result<Tick> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss=,time="])
        .output()
        .context("run ps")?;
    ensure!(out.status.success(), "ps failed");
    let table = parse_table(&String::from_utf8(out.stdout)?)?;
    let tree = tree(&table, pid);
    ensure!(!tree.is_empty(), "process {pid} is gone");
    Ok(Tick {
        at: Instant::now(),
        cpu: tree.iter().map(|process| process.cpu).sum(),
        rss_kib: tree.iter().map(|process| process.rss_kib).sum(),
    })
}

fn usage(ticks: &[Tick]) -> Result<Usage> {
    let (Some(first), Some(last)) = (ticks.first(), ticks.last()) else {
        bail!("no samples");
    };
    let wall = last.at.duration_since(first.at).as_secs_f64();
    ensure!(wall > 0.0, "the sampling window is empty");
    let cpu = last.cpu.saturating_sub(first.cpu).as_secs_f64();
    let peak = ticks.iter().map(|tick| tick.rss_kib).max().unwrap_or(0);
    Ok(Usage {
        cpu_percent: 100.0 * cpu / wall,
        peak_rss_mib: peak as f64 / 1024.0,
    })
}

fn tree(table: &[Process], root: u32) -> Vec<Process> {
    let mut found: Vec<Process> = table
        .iter()
        .filter(|process| process.pid == root)
        .copied()
        .collect();
    let mut next = 0;
    while let Some(parent) = found.get(next).map(|process| process.pid) {
        found.extend(table.iter().filter(|process| process.ppid == parent));
        next += 1;
    }
    found
}

fn parse_table(text: &str) -> Result<Vec<Process>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_row)
        .collect()
}

fn parse_row(line: &str) -> Result<Process> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [pid, ppid, rss, time] = fields.as_slice() else {
        bail!("unexpected ps row `{line}`");
    };
    Ok(Process {
        pid: pid.parse().with_context(|| format!("pid in `{line}`"))?,
        ppid: ppid.parse().with_context(|| format!("ppid in `{line}`"))?,
        rss_kib: rss.parse().with_context(|| format!("rss in `{line}`"))?,
        cpu: parse_cpu_time(time)?,
    })
}

fn parse_cpu_time(text: &str) -> Result<Duration> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>()?, clock),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in clock.split(':') {
        let value: f64 = part.parse().with_context(|| format!("cpu time `{text}`"))?;
        seconds = seconds * 60.0 + value;
    }
    ensure!(
        clock.split(':').count() <= 3,
        "cpu time `{text}` has too many fields"
    );
    Ok(Duration::from_secs_f64(days * 86_400.0 + seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_reads_every_ps_layout() {
        assert_eq!(
            parse_cpu_time("0:01.50").unwrap(),
            Duration::from_millis(1_500)
        );
        assert_eq!(parse_cpu_time("39:38.02").unwrap().as_secs(), 39 * 60 + 38);
        assert_eq!(parse_cpu_time("01:02:03").unwrap().as_secs(), 3_723);
        assert_eq!(parse_cpu_time("2-00:00:01").unwrap().as_secs(), 172_801);
        assert!(parse_cpu_time("1:2:3:4").is_err());
        assert!(parse_cpu_time("soon").is_err());
    }

    #[test]
    fn the_tree_holds_the_root_and_every_descendant() {
        let table = parse_table(
            "  1     0  100  0:01.00\n 10     1  200  0:02.00\n 11    10  300  0:03.00\n 12     1  400  0:04.00\n",
        )
        .unwrap();
        let pids: Vec<u32> = tree(&table, 10).iter().map(|process| process.pid).collect();
        assert_eq!(pids, [10, 11]);
        assert!(tree(&table, 99).is_empty());
    }

    #[test]
    fn usage_is_cpu_over_wall_time_and_the_highest_memory() {
        let start = Instant::now();
        let ticks = [
            Tick {
                at: start,
                cpu: Duration::from_secs(10),
                rss_kib: 2_048,
            },
            Tick {
                at: start + Duration::from_secs(2),
                cpu: Duration::from_secs(11),
                rss_kib: 4_096,
            },
            Tick {
                at: start + Duration::from_secs(4),
                cpu: Duration::from_secs(12),
                rss_kib: 3_072,
            },
        ];
        let usage = usage(&ticks).unwrap();
        assert!((usage.cpu_percent - 50.0).abs() < 1e-9);
        assert!((usage.peak_rss_mib - 4.0).abs() < 1e-9);
    }

    #[test]
    fn the_median_is_taken_per_metric() {
        let run = |cpu, rss| Usage {
            cpu_percent: cpu,
            peak_rss_mib: rss,
        };
        let middle = median(&[run(30.0, 1.0), run(10.0, 3.0), run(20.0, 2.0)]).unwrap();
        assert_eq!(middle, run(20.0, 2.0));
        assert!(median(&[]).is_err());
    }

    #[test]
    fn a_single_sample_is_no_measurement() {
        let tick = Tick {
            at: Instant::now(),
            cpu: Duration::ZERO,
            rss_kib: 0,
        };
        assert!(usage(&[tick]).is_err());
    }
}
