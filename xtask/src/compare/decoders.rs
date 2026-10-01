mod convert;
mod ours;
mod parse;
mod programs;
mod signals;
mod usage;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};

use super::report::{self, Better, Group, SELF, Suite};
use convert::Fixture;
use signals::{INPUT, Reference, SIGNALS, Show, Signal};

const RUNS: usize = 3;

pub fn run(root: &Path, ours: bool) -> Result<()> {
    if cfg!(debug_assertions) {
        return rerun_release(root, ours);
    }
    let work = root.join("target/compare/decoders");
    std::fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;
    let version = report::version(root)?;
    let mut groups = Vec::new();
    for signal in SIGNALS {
        groups.extend(measure(root, &work, &version, signal, ours)?);
    }
    let suite = Suite {
        machine: report::machine()?,
        groups,
    };
    report::publish(root, "decoders", suite, ours)
}

fn rerun_release(root: &Path, ours: bool) -> Result<()> {
    let status = Command::new("cargo")
        .args([
            "run",
            "--release",
            "-p",
            "xtask",
            "--",
            "compare",
            "decoders",
        ])
        .args(ours.then_some("--ours"))
        .current_dir(root)
        .status()
        .context("run cargo")?;
    ensure!(status.success(), "the release run failed");
    Ok(())
}

fn measure(
    root: &Path,
    work: &Path,
    version: &str,
    signal: &Signal,
    ours: bool,
) -> Result<Vec<Group>> {
    let fixture = prepared(root, signal)?;
    let loops = signal.loops(fixture.seconds());
    let fed = loops as f64 * fixture.seconds();
    let mut decodes = Group::new(
        &format!("{}-decodes", signal.id),
        &format!("{} decodes", signal.name),
        "msgs",
        Better::Higher,
    );
    let mut speed = Group::new(
        &format!("{}-speed", signal.id),
        &format!("{} speed", signal.name),
        "× realtime",
        Better::Higher,
    );
    let found = count(signal, &ours::decode(&fixture, signal, 1)?.keys);
    let cpu = median((0..RUNS).map(|_| Ok(ours::decode(&fixture, signal, loops)?.cpu_seconds)))?;
    record(&mut decodes, &mut speed, SELF, version, found, fed / cpu);
    let references = if ours { &[][..] } else { signal.references };
    for reference in references {
        let ready = programs::prepare(root, reference.program)?;
        let inputs = Inputs::write(work, &fixture, signal, reference, loops)?;
        let found = count(
            signal,
            &invoke(&ready.binary, reference, &inputs.once)?.keys,
        );
        let cpu = median(
            (0..RUNS).map(|_| Ok(invoke(&ready.binary, reference, &inputs.looped)?.cpu_seconds)),
        )?;
        let name = reference.program.name;
        record(
            &mut decodes,
            &mut speed,
            name,
            &ready.version,
            found,
            fed / cpu,
        );
    }
    Ok([(decodes, signal.decodes), (speed, signal.speed)]
        .into_iter()
        .filter_map(|(group, show)| shown(group, show))
        .collect())
}

fn shown(group: Group, show: Show) -> Option<Group> {
    match show {
        Show::Hidden => None,
        Show::Plain => Some(group),
        Show::Noted(note) => Some(group.with_note(note)),
    }
}

fn prepared(root: &Path, signal: &Signal) -> Result<Fixture> {
    let fixture = Fixture::load(root, signal.fixture)?;
    match signal.cu8_rate {
        Some(rate) => fixture.as_cu8(rate),
        None => Ok(fixture),
    }
}

fn record(
    decodes: &mut Group,
    speed: &mut Group,
    tool: &str,
    version: &str,
    found: usize,
    realtime: f64,
) {
    println!(
        "{:<28} {tool:<12} {found:>4} msgs {realtime:>9.1}× realtime",
        decodes.title
    );
    decodes.push(tool, version, found as f64);
    speed.push(tool, version, realtime);
}

struct Inputs {
    once: PathBuf,
    looped: PathBuf,
}

impl Inputs {
    fn write(
        work: &Path,
        fixture: &Fixture,
        signal: &Signal,
        reference: &Reference,
        loops: usize,
    ) -> Result<Self> {
        let stem = format!(
            "{}-{}",
            signal.id,
            reference
                .program
                .name
                .replace(' ', "-")
                .to_ascii_lowercase()
        );
        let extension = reference.format.extension();
        let once = work.join(format!("{stem}-once.{extension}"));
        let looped = work.join(format!("{stem}-x{loops}.{extension}"));
        let tail = if reference.tail {
            signal.tail_seconds
        } else {
            0.0
        };
        convert::write(fixture, signal.offset_hz, reference.format, 1, tail, &once)?;
        convert::write(
            fixture,
            signal.offset_hz,
            reference.format,
            loops,
            tail,
            &looped,
        )?;
        Ok(Self { once, looped })
    }
}

struct Output {
    keys: Vec<String>,
    cpu_seconds: f64,
}

fn invoke(binary: &Path, reference: &Reference, input: &Path) -> Result<Output> {
    let input = input.to_string_lossy();
    let args: Vec<&str> = reference
        .args
        .iter()
        .map(|&arg| if arg == INPUT { input.as_ref() } else { arg })
        .collect();
    let dir = binary.parent().context("binary without a directory")?;
    let before = usage::children()?;
    let out = Command::new(binary)
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {}", binary.display()))?;
    let cpu_seconds = usage::children()? - before;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let code = out.status.code().unwrap_or(-1);
    ensure!(
        reference.exit_codes.contains(&code),
        "{} exited with {code}: {}",
        reference.program.name,
        stderr.lines().rev().take(5).collect::<Vec<_>>().join(" | ")
    );
    let text = format!("{}\n{stderr}", String::from_utf8_lossy(&out.stdout));
    Ok(Output {
        keys: (reference.keys)(&text),
        cpu_seconds,
    })
}

fn count(signal: &Signal, keys: &[String]) -> usize {
    if signal.unique {
        keys.iter().collect::<BTreeSet<_>>().len()
    } else {
        keys.len()
    }
}

fn median(runs: impl Iterator<Item = Result<f64>>) -> Result<f64> {
    let mut values = runs.collect::<Result<Vec<_>>>()?;
    ensure!(!values.is_empty(), "nothing was measured");
    values.sort_by(f64::total_cmp);
    let value = values[values.len() / 2];
    ensure!(value > 0.0, "a run used no measurable CPU time");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_signals_count_each_message_once() {
        let keys: Vec<String> = ["a", "b", "a"].map(String::from).to_vec();
        let adsb = SIGNALS.iter().find(|s| s.id == "adsb").expect("adsb");
        let dmr = SIGNALS.iter().find(|s| s.id == "dmr").expect("dmr");
        assert_eq!(count(adsb, &keys), 2);
        assert_eq!(count(dmr, &keys), 3);
    }

    #[test]
    fn hidden_groups_are_dropped_and_notes_kept() {
        let group = || Group::new("x", "X", "msgs", Better::Higher);
        assert!(shown(group(), Show::Hidden).is_none());
        assert_eq!(shown(group(), Show::Plain).and_then(|g| g.note), None);
        assert_eq!(
            shown(group(), Show::Noted("n"))
                .and_then(|g| g.note)
                .as_deref(),
            Some("n")
        );
    }

    #[test]
    fn notes_stay_short() {
        for signal in SIGNALS {
            for show in [signal.decodes, signal.speed] {
                if let Show::Noted(note) = show {
                    assert!(note.split_whitespace().count() <= 10, "{note}");
                    assert!(!note.contains('\u{2014}'), "{note}");
                }
            }
        }
    }

    #[test]
    fn the_median_run_is_reported() {
        let runs = [3.0, 1.0, 2.0].map(Ok);
        assert_eq!(median(runs.into_iter()).expect("median"), 2.0);
        assert!(median([0.0].map(Ok).into_iter()).is_err());
    }
}
