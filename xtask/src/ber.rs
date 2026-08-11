//! `cargo xtask ber <entry>` — run one measurement-harness entry and write its curve to disk
//! (MODEM-PLAN §3.1: "`cargo xtask ber <entry>` → CSV/JSON").
//!
//! The command is the local face of the harness's CI contract: the same sweep the crate's own
//! gate tests run, but with the curve landed as files — JSON as the committed-artifact format,
//! CSV for plotting — so a curve can be reviewed, diffed, or fed to external tooling without
//! rerunning anything. PASS/FAIL is judged the way the gates judge it: worst horizontal
//! penalty against the entry's oracle, held to the phase-0 tolerance of 0.2 dB, and a FAIL is
//! a nonzero exit so CI and scripts read it without parsing text. The files are written even
//! on FAIL — a failing curve is exactly the one worth looking at.
//!
//! Two tiers per entry, mirroring the crate's smoke/full test split (MODEM-PLAN §4.4 CI
//! policy): the default is the fast smoke subset, `--full` the nightly grid. The full tier
//! reuses the gate tests' seeds and error budgets on purpose, so a curve this command writes
//! is bit-identical to what the in-crate gate measured and comparable against the committed
//! baseline without a seed footnote.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sdrmm_modem::ber::{
    Curve,
    impair::ChannelSpec,
    reference,
    sweep::{save_csv, save_json, sweep_ber, worst_penalty_db},
    theory,
};

/// The phase-0 calibration tolerance (MODEM-PLAN §7 phase 0): a measured curve within this of
/// its closed-form oracle. Entries whose reference is commit-and-guard rather than a closed
/// form will carry their own gate when they land.
const TOLERANCE_DB: f64 = 0.2;

/// An entry's runner: sweep, write files into the given directory, print the verdict, and
/// return `Err` on FAIL. `full` selects the nightly grid over the smoke subset.
type Runner = fn(dir: &Path, full: bool) -> Result<()>;

/// The registry `cargo xtask ber` dispatches on. Later phases add an entry by pushing one
/// line here and writing its runner below — nothing else in the command changes.
const ENTRIES: &[(&str, Runner)] = &[("bpsk-ideal", bpsk_ideal)];

pub fn run(root: &Path, entry: &str, out: Option<&Path>, full: bool) -> Result<()> {
    let Some((_, runner)) = ENTRIES.iter().find(|(name, _)| *name == entry) else {
        let known: Vec<&str> = ENTRIES.iter().map(|(name, _)| *name).collect();
        bail!(
            "unknown ber entry `{entry}`; known entries: {}",
            known.join(", ")
        );
    };
    let dir = out.map_or_else(|| root.join("target/ber"), Path::to_path_buf);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create output directory {}", dir.display()))?;
    runner(&dir, full)
}

/// The harness's own calibration standard (MODEM-PLAN §4.1): `reference::ideal_bpsk` against
/// the exact ½·erfc(√γ) form. The error budgets are the gate tests' own, far above the
/// 100-error floor, because at the shallow low-SNR log-slope a 100-error point's horizontal
/// confidence interval is wider than the 0.2 dB being judged (see the budget discussion on the
/// reference tests).
fn bpsk_ideal(dir: &Path, full: bool) -> Result<()> {
    let grid: Vec<f64> = if full {
        (0..=10).map(f64::from).collect()
    } else {
        (0..=6).step_by(2).map(f64::from).collect()
    };
    let (seed, min_errors, max_trial_bits) = if full {
        (0x5eed, 10_000, 50_000_000)
    } else {
        (0x0b9, 5_000, 4_000_000)
    };
    let link = reference::ideal_bpsk();
    let curve = sweep_ber(
        &link,
        &ChannelSpec::default(),
        &grid,
        seed,
        min_errors,
        max_trial_bits,
    );
    print_points(&curve);
    write_curve(&curve, dir, "bpsk_ideal")?;
    let worst = worst_penalty_db(&curve, theory::bpsk_ber, grid[0], grid[grid.len() - 1]);
    verdict("bpsk-ideal", "exact ½·erfc(√γ)", worst)
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

/// One PASS/FAIL line against [`TOLERANCE_DB`], with FAIL as a nonzero exit. The penalty is
/// signed on purpose — a curve *beating* theory past counting noise is a harness bug, so the
/// gate reads magnitude and the sign stays visible for the reader.
fn verdict(entry: &str, oracle: &str, worst_db: f64) -> Result<()> {
    println!("worst penalty vs {oracle}: {worst_db:+.4} dB (tolerance {TOLERANCE_DB} dB)");
    if worst_db.abs() < TOLERANCE_DB {
        println!("PASS: {entry}");
        return Ok(());
    }
    bail!("FAIL: {entry} is {worst_db:+.4} dB from its oracle (tolerance {TOLERANCE_DB} dB)");
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
        for (name, _) in ENTRIES {
            assert!(msg.contains(name), "{msg:?} does not name {name}");
        }
    }

    /// The command path end-to-end at the smoke tier: bpsk-ideal passes its own gate and both
    /// file formats land where `--out` pointed. The sweep itself is tested in `sdrmm-modem`;
    /// this covers the wiring above it.
    #[test]
    fn bpsk_ideal_smoke_passes_and_writes_both_files() {
        let dir = std::env::temp_dir().join(format!("sdrmm-xtask-ber-{}", std::process::id()));
        run(Path::new("."), "bpsk-ideal", Some(&dir), false).unwrap();
        for name in ["bpsk_ideal.json", "bpsk_ideal.csv"] {
            assert!(dir.join(name).is_file(), "{name} missing");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
