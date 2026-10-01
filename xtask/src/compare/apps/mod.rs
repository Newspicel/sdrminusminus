use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};

use super::report::{self, SELF, Suite};

mod feeder;
mod gqrx;
mod merge;
mod running;
mod sample;
mod sdrmm;
mod sdrpp;
mod signal;

use feeder::Feeder;
use merge::Measurement;
use running::Running;
use sample::Usage;
use signal::Signal;

const RECEIVERS: [usize; 3] = [1, 4, 16];
const MAX_LAG: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Program {
    Sdrmm,
    Sdrpp,
    Gqrx,
}

#[derive(Args)]
pub struct Apps {
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Program::Sdrmm, Program::Sdrpp, Program::Gqrx])]
    programs: Vec<Program>,
    #[arg(long, default_value_t = 10)]
    warmup: u64,
    #[arg(long, default_value_t = 30)]
    window: u64,
    #[arg(long, default_value_t = 3)]
    runs: usize,
    #[arg(long, requires_all = ["tool", "version", "case"])]
    pid: Option<u32>,
    #[arg(long, requires = "pid")]
    tool: Option<String>,
    #[arg(long, requires = "pid")]
    version: Option<String>,
    #[arg(long, requires = "pid", value_parser = parse_case)]
    case: Option<usize>,
}

#[derive(Clone, Copy)]
struct Timing {
    warmup: Duration,
    window: Duration,
}

pub fn run(root: &Path, args: &Apps) -> Result<()> {
    let timing = Timing {
        warmup: Duration::from_secs(args.warmup),
        window: Duration::from_secs(args.window),
    };
    let measured = match args.pid {
        Some(pid) => vec![attached(pid, args, timing)?],
        None => launched(root, &args.programs, timing, args.runs)?,
    };
    let path = root.join("site/src/data/bench/apps.json");
    let old = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Suite>(&text).ok());
    let suite = merge::merge(old, report::machine()?, &measured);
    report::write(root, "apps", &suite)
}

fn attached(pid: u32, args: &Apps, timing: Timing) -> Result<Measurement> {
    let (Some(tool), Some(version), Some(receivers)) = (&args.tool, &args.version, args.case)
    else {
        bail!("--pid needs --tool, --version and --case");
    };
    println!("sampling {tool} (pid {pid}), {}", merge::case_id(receivers));
    sample::settle(pid, timing.warmup)?;
    Ok(Measurement {
        tool: tool.clone(),
        version: version.clone(),
        receivers,
        usage: sample::measure(pid, timing.window)?,
    })
}

fn launched(
    root: &Path,
    programs: &[Program],
    timing: Timing,
    runs: usize,
) -> Result<Vec<Measurement>> {
    let signal = signal::prepare(root)?;
    let work = root.join("target/compare/apps/work");
    let mut measured = Vec::new();
    for &program in programs {
        let (tool, version) = prepare(root, program)?;
        for receivers in cases(program) {
            println!("{tool} {version}: {}", merge::case_id(receivers));
            let repeated = (0..runs)
                .map(|_| {
                    std::fs::remove_dir_all(&work).ok();
                    std::fs::create_dir_all(&work)
                        .with_context(|| format!("create {}", work.display()))?;
                    let usage =
                        session(root, program, &signal, receivers, &work)?.measure(timing)?;
                    println!(
                        "  {:.1} % core, {:.1} MiB",
                        usage.cpu_percent, usage.peak_rss_mib
                    );
                    Ok(usage)
                })
                .collect::<Result<Vec<_>>>()
                .and_then(|usages| sample::median(&usages));
            match repeated {
                Ok(usage) => measured.push(Measurement {
                    tool: tool.to_owned(),
                    version: version.clone(),
                    receivers,
                    usage,
                }),
                Err(err) => println!("  not recorded: {err:#}"),
            }
        }
    }
    Ok(measured)
}

struct Session {
    feeder: Option<Feeder>,
    running: Running,
}

impl Session {
    fn measure(mut self, timing: Timing) -> Result<Usage> {
        let pid = self.running.pid();
        sample::settle(pid, timing.warmup)?;
        if let Some(feeder) = &self.feeder {
            feeder.reset_lag();
        }
        let usage = sample::measure(pid, timing.window)?;
        self.running.alive()?;
        if let Some(feeder) = &self.feeder {
            ensure!(!feeder.failed(), "stopped reading the IQ feed");
            let lag = feeder.worst_lag();
            ensure!(
                lag <= MAX_LAG,
                "fell {} ms behind real time",
                lag.as_millis()
            );
        }
        Ok(usage)
    }
}

fn session(
    root: &Path,
    program: Program,
    signal: &Signal,
    receivers: usize,
    work: &Path,
) -> Result<Session> {
    match program {
        Program::Sdrmm => Ok(Session {
            running: sdrmm::launch(root, signal, receivers, work)?,
            feeder: None,
        }),
        Program::Sdrpp => {
            let feeder = Feeder::start(&signal.raw, signal::RATE)?;
            Ok(Session {
                running: sdrpp::launch(root, &feeder, receivers, work)?,
                feeder: Some(feeder),
            })
        }
        Program::Gqrx => Ok(Session {
            running: gqrx::launch(signal, receivers, work)?,
            feeder: None,
        }),
    }
}

fn prepare(root: &Path, program: Program) -> Result<(&'static str, String)> {
    match program {
        Program::Sdrmm => {
            sdrmm::build(root)?;
            Ok((SELF, report::version(root)?))
        }
        Program::Sdrpp => {
            sdrpp::install(root)?;
            Ok((sdrpp::TOOL, sdrpp::version(root)?))
        }
        Program::Gqrx => {
            gqrx::install()?;
            Ok((gqrx::TOOL, gqrx::version()?))
        }
    }
}

fn cases(program: Program) -> Vec<usize> {
    match program {
        Program::Gqrx => vec![1],
        Program::Sdrmm | Program::Sdrpp => RECEIVERS.to_vec(),
    }
}

fn parse_case(text: &str) -> Result<usize, String> {
    RECEIVERS
        .into_iter()
        .find(|&receivers| merge::case_id(receivers) == text)
        .ok_or_else(|| {
            let known: Vec<String> = RECEIVERS.into_iter().map(merge::case_id).collect();
            format!("unknown case `{text}`, known: {}", known.join(", "))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_is_named_by_its_receiver_count() {
        assert_eq!(parse_case("nfm-4"), Ok(4));
        assert!(parse_case("nfm-3").is_err());
    }

    #[test]
    fn gqrx_only_runs_the_single_receiver_case() {
        assert_eq!(cases(Program::Gqrx), [1]);
        assert_eq!(cases(Program::Sdrpp), RECEIVERS);
    }
}
