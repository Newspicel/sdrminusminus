use std::path::Path;

use anyhow::{Context, Result, bail};
use sdrmm_modem::ber::{
    Curve,
    analog::{self as analog_harness, SinadCurve},
    catalog::{self, Entry, Measurement, analog as analog_catalog},
    impair::ChannelSpec,
    sweep::{save_csv, save_json, sweep_ber},
};

pub fn run(root: &Path, entry: &str, out: Option<&Path>, full: bool) -> Result<()> {
    let dir = out.map_or_else(|| root.join("target/ber"), Path::to_path_buf);
    if let Some(analog) = analog_catalog::find(entry) {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create output directory {}", dir.display()))?;
        return measure_analog(root, analog, &dir, full);
    }
    let Some(entry) = catalog::find(entry) else {
        let known: Vec<&str> = catalog::ENTRIES
            .iter()
            .map(|e| e.name)
            .chain(analog_catalog::ENTRIES.iter().map(|e| e.name))
            .collect();
        bail!(
            "unknown ber entry `{entry}`; known entries: {}",
            known.join(", ")
        );
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create output directory {}", dir.display()))?;
    measure(root, entry, &dir, full)
}

fn measure_analog(
    root: &Path,
    entry: &analog_catalog::AnalogEntry,
    dir: &Path,
    full: bool,
) -> Result<()> {
    let mut failures = Vec::new();
    for m in entry.measurements {
        let curve = analog_harness::sweep_sinad(
            &(m.link)(),
            &ChannelSpec::default(),
            m.tier(full),
            m.seed,
            m.trials,
        );
        println!("{}", curve.label);
        for p in &curve.points {
            println!(
                "{:>5.1} dB  SINAD {:>7.2} dB  THD {:>6.2} %",
                p.snr_db, p.sinad_db, p.thd_percent
            );
        }
        write_sinad_curve(&curve, dir, &m.stem.replace('/', "_"))?;
        if let Err(fault) = judge_analog(root, m, &curve) {
            println!("FAIL: {fault}");
            failures.push(fault);
        } else {
            println!("PASS: {}", m.stem);
        }
        println!();
    }
    if failures.is_empty() {
        println!(
            "PASS: {} ({} measured)",
            entry.name,
            entry.measurements.len()
        );
        return Ok(());
    }
    bail!(
        "FAIL: {} — {} of {} measurements off their reference:\n  {}",
        entry.name,
        failures.len(),
        entry.measurements.len(),
        failures.join("\n  ")
    );
}

fn judge_analog(
    root: &Path,
    m: &analog_catalog::AnalogMeasurement,
    curve: &SinadCurve,
) -> std::result::Result<(), String> {
    let mut faults = Vec::new();
    let (Some(lo), Some(hi)) = (curve.points.first(), curve.points.last()) else {
        return Err(format!("{}: the sweep produced no usable point", m.stem));
    };
    match analog_harness::load_json(&root.join(m.artifact())) {
        Ok(committed) => {
            let drift = analog_harness::worst_shortfall_db_vs_curve(
                curve, &committed, lo.snr_db, hi.snr_db,
            );
            println!("{}: drift vs committed {drift:+.4} dB", m.stem);
            if drift.abs() >= catalog::DRIFT_TOLERANCE_DB {
                faults.push(format!("{} drifted {drift:+.4} dB", m.stem));
            }
        }
        Err(_) => println!(
            "{}: no committed artifact at {} — nothing to guard against",
            m.stem,
            m.artifact()
        ),
    }
    match m.reference.oracle() {
        Some((name, oracle, from_db, tolerance)) if hi.snr_db >= from_db => {
            let gap = analog_harness::worst_shortfall_db(curve, oracle, from_db, hi.snr_db);
            println!(
                "{}: {gap:+.4} dB vs {name} (tolerance {tolerance} dB)",
                m.stem
            );
            if gap.abs() >= tolerance {
                faults.push(format!("{} is {gap:+.4} dB from {name}", m.stem));
            }
        }
        Some((name, _, from_db, _)) => println!(
            "{}: below {from_db} dB there is no {name} to judge against",
            m.stem
        ),
        None => println!(
            "{}: commit-and-guard — no closed form; the committed curve is the reference",
            m.stem
        ),
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults.join("; "))
    }
}

fn write_sinad_curve(curve: &SinadCurve, dir: &Path, stem: &str) -> Result<()> {
    let json = dir.join(format!("{stem}.json"));
    analog_harness::save_json(curve, &json).with_context(|| format!("write {}", json.display()))?;
    let csv = dir.join(format!("{stem}.csv"));
    analog_harness::save_csv(curve, &csv).with_context(|| format!("write {}", csv.display()))?;
    println!("wrote {}", json.display());
    println!("wrote {}", csv.display());
    Ok(())
}

/// Every measurement of the entry, each judged on its own line, with the entry's verdict the
/// worst of them. All of them run even after one fails: the point of landing files is to look
/// at the whole entry, and stopping at the first bad curve would hide the rest.
fn measure(root: &Path, entry: &Entry, dir: &Path, full: bool) -> Result<()> {
    let mut failures = Vec::new();
    for m in entry.measurements {
        let curve = sweep(m, full);
        print_points(&curve);
        write_curve(&curve, dir, &m.stem.replace('/', "_"))?;
        if let Err(fault) = judge(root, m, &curve) {
            println!("FAIL: {}", fault);
            failures.push(fault);
        } else {
            println!("PASS: {}", m.stem);
        }
        println!();
    }
    if failures.is_empty() {
        println!(
            "PASS: {} ({} measured)",
            entry.name,
            entry.measurements.len()
        );
        return Ok(());
    }
    bail!(
        "FAIL: {} — {} of {} measurements off their reference:\n  {}",
        entry.name,
        failures.len(),
        entry.measurements.len(),
        failures.join("\n  ")
    );
}

fn sweep(m: &Measurement, full: bool) -> Curve {
    let tier = m.tier(full);
    sweep_ber(
        &(m.link)(),
        &ChannelSpec::default(),
        tier.grid,
        tier.seed,
        tier.min_errors,
        tier.max_trial_bits,
    )
}

fn judge(root: &Path, m: &Measurement, curve: &Curve) -> std::result::Result<(), String> {
    let mut faults = Vec::new();

    let path = root.join(m.artifact());
    match sdrmm_modem::ber::sweep::load_json(&path) {
        Ok(committed) => match m.drift_db(curve, &committed) {
            Some(drift) => {
                println!(
                    "{}: drift vs committed {drift:+.4} dB (tolerance {} dB)",
                    m.stem,
                    catalog::DRIFT_TOLERANCE_DB
                );
                if drift.abs() >= catalog::DRIFT_TOLERANCE_DB {
                    faults.push(format!("{} drifted {drift:+.4} dB", m.stem));
                }
            }
            None => faults.push(format!("{}: the sweep produced no usable point", m.stem)),
        },
        // Not a fault: this is how a new entry's first curve is created. It is loud, though —
        // an unjudged curve is a measurement nobody has checked yet.
        Err(_) => println!(
            "{}: no committed artifact at {} — nothing to guard against",
            m.stem,
            m.artifact()
        ),
    }

    match m.reference_gap(curve) {
        Some((reference, gap, tolerance)) => {
            println!(
                "{}: {gap:+.4} dB vs {reference} (tolerance {tolerance} dB)",
                m.stem
            );
            if gap.abs() >= tolerance {
                faults.push(format!("{} is {gap:+.4} dB from {reference}", m.stem));
            }
        }
        None => println!(
            "{}: commit-and-guard — no closed form; the committed curve is the reference",
            m.stem
        ),
    }

    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults.join("; "))
    }
}

fn print_points(curve: &Curve) {
    println!("{}", curve.label);
    for p in &curve.points {
        println!(
            "{:>5.1} dB  {:>10} / {:<12} BER {:.3e}",
            p.ebn0_db,
            p.errors,
            p.trials,
            p.rate()
        );
    }
}

fn write_curve(curve: &Curve, dir: &Path, stem: &str) -> Result<()> {
    let json = dir.join(format!("{stem}.json"));
    save_json(curve, &json).with_context(|| format!("write {}", json.display()))?;
    let csv = dir.join(format!("{stem}.csv"));
    save_csv(curve, &csv).with_context(|| format!("write {}", csv.display()))?;
    println!("wrote {}", json.display());
    println!("wrote {}", csv.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error is the command's usage message, so it must name every registered entry —
    /// a registry line someone forgets to mention would be undiscoverable from the CLI.
    #[test]
    fn unknown_entry_lists_the_known_ones() {
        let err = run(Path::new("."), "no-such-entry", None, false)
            .expect_err("an unknown entry must not run");
        let msg = err.to_string();
        for name in catalog::ENTRIES
            .iter()
            .map(|e| e.name)
            .chain(analog_catalog::ENTRIES.iter().map(|e| e.name))
        {
            assert!(msg.contains(name), "{msg:?} does not name {name}");
        }
    }

    /// Two measurements of one entry must not collide in the output directory. Stems are
    /// registry paths (`cpm/gmsk_bt03_awgn`); this command flattens them into file names, and
    /// a flattening that lost a distinction would silently overwrite one curve with another.
    #[test]
    fn flattened_stems_stay_unique_within_an_entry() {
        for entry in catalog::ENTRIES {
            let mut names: Vec<String> = entry
                .measurements
                .iter()
                .map(|m| m.stem.replace('/', "_"))
                .collect();
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), count, "{} writes colliding files", entry.name);
        }
    }

    /// The command path end-to-end at the smoke tier: the calibration entry passes its own
    /// gate and both file formats land where `--out` pointed. The sweeps themselves are tested
    /// in `sdrmm-modem`; this covers the wiring above them.
    #[test]
    fn bpsk_ideal_smoke_passes_and_writes_both_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let dir = std::env::temp_dir().join(format!("sdrmm-xtask-ber-{}", std::process::id()));
        run(&root, "bpsk-ideal", Some(&dir), false).unwrap();
        for name in ["bpsk_ideal_awgn.json", "bpsk_ideal_awgn.csv"] {
            assert!(dir.join(name).is_file(), "{name} missing");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
