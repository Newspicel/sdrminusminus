use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use super::footprint;

const TICK: Duration = Duration::from_millis(250);
const ATTEMPTS: usize = 3;
const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub cpu_percent: f64,
    pub peak_memory_mib: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Process {
    pid: u32,
    ppid: u32,
    zombie: bool,
    cpu: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Tick {
    at: Instant,
    cpu: Duration,
    memory_bytes: u64,
}

pub fn settle(pids: &[u32], warmup: Duration) -> Result<()> {
    let end = Instant::now() + warmup;
    while Instant::now() < end {
        tick(pids)?;
        thread::sleep(TICK);
    }
    Ok(())
}

pub fn measure(pids: &[u32], window: Duration) -> Result<Usage> {
    let start = Instant::now();
    let mut ticks = vec![tick(pids)?];
    while start.elapsed() < window {
        thread::sleep(TICK);
        ticks.push(tick(pids)?);
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
        peak_memory_mib: middle(|usage| usage.peak_memory_mib),
    })
}

fn tick(pids: &[u32]) -> Result<Tick> {
    let mut attempt = 1;
    loop {
        match read(pids) {
            Err(_) if attempt < ATTEMPTS => {
                attempt += 1;
                thread::sleep(TICK);
            }
            done => return done,
        }
    }
}

fn read(pids: &[u32]) -> Result<Tick> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,time="])
        .output()
        .context("run ps")?;
    ensure!(out.status.success(), "ps failed");
    let table = parse_table(&String::from_utf8(out.stdout)?)?;
    let processes = forest(&table, pids)?;
    let alive: Vec<u32> = processes
        .iter()
        .filter(|process| !process.zombie)
        .map(|process| process.pid)
        .collect();
    Ok(Tick {
        at: Instant::now(),
        cpu: processes.iter().map(|process| process.cpu).sum(),
        memory_bytes: footprint::bytes(&alive)?,
    })
}

fn forest(table: &[Process], roots: &[u32]) -> Result<Vec<Process>> {
    let mut found: Vec<Process> = Vec::new();
    for &root in roots {
        let grown = tree(table, root);
        ensure!(!grown.is_empty(), "process {root} is gone");
        for process in grown {
            if !found.contains(&process) {
                found.push(process);
            }
        }
    }
    Ok(found)
}

fn usage(ticks: &[Tick]) -> Result<Usage> {
    let (Some(first), Some(last)) = (ticks.first(), ticks.last()) else {
        bail!("no samples");
    };
    let wall = last.at.duration_since(first.at).as_secs_f64();
    ensure!(wall > 0.0, "the sampling window is empty");
    let cpu = last.cpu.saturating_sub(first.cpu).as_secs_f64();
    let peak = ticks
        .iter()
        .map(|tick| tick.memory_bytes)
        .max()
        .unwrap_or(0);
    Ok(Usage {
        cpu_percent: 100.0 * cpu / wall,
        peak_memory_mib: peak as f64 / MIB,
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
    let [pid, ppid, stat, time] = fields.as_slice() else {
        bail!("unexpected ps row `{line}`");
    };
    Ok(Process {
        pid: pid.parse().with_context(|| format!("pid in `{line}`"))?,
        ppid: ppid.parse().with_context(|| format!("ppid in `{line}`"))?,
        zombie: stat.starts_with('Z'),
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
            "  1     0 Ss  0:01.00\n 10     1 S   0:02.00\n 11    10 Z   0:03.00\n 12     1 R+  0:04.00\n",
        )
        .unwrap();
        let pids: Vec<u32> = tree(&table, 10).iter().map(|process| process.pid).collect();
        assert_eq!(pids, [10, 11]);
        assert!(
            table
                .iter()
                .any(|process| process.pid == 11 && process.zombie)
        );
        assert!(!table[0].zombie);
        assert!(tree(&table, 99).is_empty());
    }

    #[test]
    fn several_roots_count_each_process_once() {
        let table = parse_table(
            "  1     0 Ss  0:01.00\n 10     1 S   0:02.00\n 11    10 Z   0:03.00\n 12     1 R+  0:04.00\n",
        )
        .unwrap();
        let pids: Vec<u32> = forest(&table, &[10, 11, 12])
            .unwrap()
            .iter()
            .map(|process| process.pid)
            .collect();
        assert_eq!(pids, [10, 11, 12]);
        assert!(forest(&table, &[10, 99]).is_err());
    }

    #[test]
    fn usage_is_cpu_over_wall_time_and_the_highest_memory() {
        let start = Instant::now();
        let ticks = [
            Tick {
                at: start,
                cpu: Duration::from_secs(10),
                memory_bytes: 2 << 20,
            },
            Tick {
                at: start + Duration::from_secs(2),
                cpu: Duration::from_secs(11),
                memory_bytes: 4 << 20,
            },
            Tick {
                at: start + Duration::from_secs(4),
                cpu: Duration::from_secs(12),
                memory_bytes: 3 << 20,
            },
        ];
        let usage = usage(&ticks).unwrap();
        assert!((usage.cpu_percent - 50.0).abs() < 1e-9);
        assert!((usage.peak_memory_mib - 4.0).abs() < 1e-9);
    }

    #[test]
    fn the_median_is_taken_per_metric() {
        let run = |cpu, memory| Usage {
            cpu_percent: cpu,
            peak_memory_mib: memory,
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
            memory_bytes: 0,
        };
        assert!(usage(&[tick]).is_err());
    }
}
