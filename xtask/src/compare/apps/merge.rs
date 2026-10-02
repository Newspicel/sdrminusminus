use super::{RECEIVERS, sample::Usage};
use crate::compare::report::{Better, Group, Machine, Suite};

#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub tool: String,
    pub version: String,
    pub receivers: usize,
    pub usage: Usage,
}

#[derive(Clone, Copy)]
enum Kind {
    Cpu,
    Memory,
}

pub fn case_id(receivers: usize) -> String {
    format!("nfm-{receivers}")
}

pub fn merge(old: Option<Suite>, machine: Machine, measured: &[Measurement]) -> Suite {
    let previous = old
        .filter(|suite| suite.machine.cpu == machine.cpu && suite.machine.os == machine.os)
        .map(|suite| suite.groups)
        .unwrap_or_default();
    let groups = RECEIVERS
        .iter()
        .flat_map(|&receivers| [(receivers, Kind::Cpu), (receivers, Kind::Memory)])
        .filter_map(|(receivers, kind)| {
            let mut group = blank(receivers, kind);
            if let Some(found) = previous.iter().find(|old| old.id == group.id) {
                group.results.clone_from(&found.results);
            }
            for entry in measured.iter().filter(|m| m.receivers == receivers) {
                group.results.retain(|old| old.tool != entry.tool);
                group.push(&entry.tool, &entry.version, value(entry, kind));
            }
            (!group.results.is_empty()).then_some(group)
        })
        .collect();
    Suite { machine, groups }
}

fn value(entry: &Measurement, kind: Kind) -> f64 {
    let raw = match kind {
        Kind::Cpu => entry.usage.cpu_percent,
        Kind::Memory => entry.usage.peak_memory_mib,
    };
    (raw * 10.0).round() / 10.0
}

fn blank(receivers: usize, kind: Kind) -> Group {
    let what = if receivers == 1 {
        "1 NFM receiver".to_owned()
    } else {
        format!("{receivers} NFM receivers")
    };
    let case = case_id(receivers);
    match kind {
        Kind::Cpu => Group::new(
            &format!("{case}-cpu"),
            &format!("{what}, CPU"),
            "% core",
            Better::Lower,
        ),
        Kind::Memory => Group::new(
            &format!("{case}-memory"),
            &format!("{what}, memory"),
            "MiB",
            Better::Lower,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::report::SELF;

    fn machine(cpu: &str) -> Machine {
        Machine {
            cpu: cpu.into(),
            os: "macOS".into(),
            date: "2026-10-01".into(),
        }
    }

    fn measured(tool: &str, receivers: usize, cpu: f64) -> Measurement {
        Measurement {
            tool: tool.into(),
            version: "1".into(),
            receivers,
            usage: Usage {
                cpu_percent: cpu,
                peak_memory_mib: 100.04,
            },
        }
    }

    #[test]
    fn each_case_gets_a_cpu_and_a_memory_group_in_order() {
        let suite = merge(
            None,
            machine("M4"),
            &[measured(SELF, 16, 1.0), measured(SELF, 1, 2.0)],
        );
        let ids: Vec<&str> = suite.groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(
            ids,
            ["nfm-1-cpu", "nfm-1-memory", "nfm-16-cpu", "nfm-16-memory"]
        );
        assert_eq!(suite.groups[1].results[0].value, 100.0);
    }

    #[test]
    fn a_new_run_replaces_only_its_own_tool() {
        let first = merge(
            None,
            machine("M4"),
            &[measured(SELF, 4, 10.0), measured("SDR++", 4, 30.0)],
        );
        let second = merge(Some(first), machine("M4"), &[measured(SELF, 4, 12.0)]);
        let cpu = &second.groups[0].results;
        assert_eq!(cpu.len(), 2);
        assert!(cpu.iter().any(|e| e.tool == "SDR++" && e.value == 30.0));
        assert!(cpu.iter().any(|e| e.tool == SELF && e.value == 12.0));
    }

    #[test]
    fn results_from_another_machine_are_dropped() {
        let first = merge(None, machine("M1"), &[measured("SDR++", 4, 30.0)]);
        let second = merge(Some(first), machine("M4"), &[measured(SELF, 4, 12.0)]);
        assert_eq!(second.groups[0].results.len(), 1);
    }
}
