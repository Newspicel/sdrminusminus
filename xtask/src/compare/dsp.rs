use std::path::Path;

use anyhow::{Context, Result};

use super::report::{self, Better, Group, Suite};

mod harness;
mod output;

use harness::Timing;
use output::Measured;

pub struct Kernel {
    pub id: &'static str,
    pub title: &'static str,
    pub ratio: Option<f64>,
    pub note: Option<&'static str>,
}

pub const KERNELS: &[Kernel] = &[
    Kernel {
        id: "fir",
        title: "FIR, 127 taps",
        ratio: None,
        note: None,
    },
    Kernel {
        id: "decimate",
        title: "FIR, 127 taps, decimate by 4",
        ratio: None,
        note: None,
    },
    Kernel {
        id: "resample",
        title: "Resample 44.1 to 48 kHz",
        ratio: Some(48_000.0 / 44_100.0),
        note: Some("Each library's default filter design."),
    },
    Kernel {
        id: "nco",
        title: "NCO mix",
        ratio: None,
        note: None,
    },
    Kernel {
        id: "ddc",
        title: "DDC 20 MS/s to 48 kHz",
        ratio: Some(48e3 / 20e6),
        note: Some("Each library's own chain, same passband."),
    },
    Kernel {
        id: "fft",
        title: "FFT, 4,096 points",
        ratio: None,
        note: None,
    },
    Kernel {
        id: "fm",
        title: "FM demod",
        ratio: None,
        note: None,
    },
];

const TIMING: Timing = Timing {
    block: 8192,
    reps: 51,
    rep_seconds: 0.02,
};

pub struct Tool {
    pub name: &'static str,
    pub version: String,
    pub measured: Measured,
}

pub fn run(root: &Path, ours: bool) -> Result<()> {
    let built = if ours {
        vec![harness::ours(root)?]
    } else {
        harness::build(root)?
    };
    let mut tools = Vec::new();
    for built in built {
        println!("measuring {}", built.name);
        let text = built.execute(&TIMING)?;
        let measured =
            output::parse(&text).with_context(|| format!("read {} results", built.name))?;
        output::check_ratios(&measured).with_context(|| format!("{} output rate", built.name))?;
        let version = built
            .version
            .or_else(|| measured.version.clone())
            .with_context(|| format!("{} did not report a version", built.name))?;
        tools.push(Tool {
            name: built.name,
            version,
            measured,
        });
    }
    let suite = Suite {
        machine: report::machine()?,
        groups: groups(&tools),
    };
    report::publish(root, "dsp", suite, ours)
}

pub fn groups(tools: &[Tool]) -> Vec<Group> {
    KERNELS
        .iter()
        .map(|kernel| {
            let mut group = Group::new(kernel.id, kernel.title, "Msps", Better::Higher);
            if let Some(note) = kernel.note {
                group = group.with_note(note);
            }
            for tool in tools {
                if let Some(value) = tool.measured.value(kernel.id) {
                    group.push(tool.name, &tool.version, value);
                }
            }
            group
        })
        .filter(|group| !group.results.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &'static str, text: &str) -> Tool {
        Tool {
            name,
            version: "1".into(),
            measured: output::parse(text).expect("valid output"),
        }
    }

    #[test]
    fn groups_follow_kernel_order_and_skip_unmeasured_tools() {
        let tools = [
            tool(report::SELF, "fft\t500\nfir\t100\n"),
            tool("FutureSDR", "fir\t50\n"),
        ];
        let groups = groups(&tools);
        let ids: Vec<&str> = groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, ["fir", "fft"]);
        assert_eq!(groups[0].results.len(), 2);
        assert_eq!(groups[1].results.len(), 1);
        assert_eq!(groups[0].results[1].tool, "FutureSDR");
    }
}
