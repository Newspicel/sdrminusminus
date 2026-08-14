pub mod afsk;
pub mod analog;
pub mod ask;
pub mod framing;
pub mod gmsk;
pub mod linear;
pub mod mfsk;
pub mod msk;
pub mod multicarrier;
pub mod ofdm;
pub mod orthogonal;
pub mod ppm;
pub mod psk;
pub mod qam;
pub mod spread;

use crate::ber::{
    Curve,
    sweep::{Link, penalty_db, worst_penalty_db, worst_penalty_db_vs_curve},
    theory,
};

/// Where committed artifacts live, relative to the workspace root — the path the docs-row rule
/// in `cargo xtask check` resolves and `CATALOG.md` writes out in full.
pub const BASELINE_DIR: &str = "crates/modem/baselines";

/// Grid points a smoke tier measures, from the front of the committed grid. Three is the
/// crate's own idiom: enough to place the shoulder, cheap enough to run on every `cargo test`.
pub const SMOKE_POINTS: usize = 3;

/// Errors a committed curve's point collects before its ratio is believed. Errors arrive in
/// two populations — a steady trickle and rare whole-trial failures when a low-SNR trial
/// mis-anchors — so the budget is set by the heavy tail: 2000 keeps a shoulder point's
/// realisation from being one failed trial's. The trial-bit *cap* that bounds the steep
/// high-SNR points instead is per entry, stated with each entry's grids.
pub const FULL_ERRORS: u64 = 2_000;

/// Worst drift a re-measured tier may show against its committed artifact. Same seed, same
/// budgets and same grid points make each measured point a *reproduction* of the committed
/// one — bit-identical on one host — so this slack absorbs cross-platform float drift and
/// nothing else.
pub const DRIFT_TOLERANCE_DB: f64 = 0.5;

/// One sweep tier: the grid, the seed that names every point's realisation, and the error
/// budget. `(seed, grid index)` names a point, so a grid *prefix* at the same seed and budget
/// reproduces the committed points exactly — which is what makes a smoke tier a regression
/// gate rather than an independent measurement.
#[derive(Clone, Copy, Debug)]
pub struct Tier {
    pub grid: &'static [f64],
    pub seed: u64,
    pub min_errors: u64,
    pub max_trial_bits: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum Reference {
    /// Commit-and-guard: no closed form describes the chain, so the committed artifact is the
    /// reference and the drift gate is the whole judgement. Every partial-response CPM row
    /// through a discriminator is here.
    Committed,
    /// Oracle-matched: a closed form the whole curve is held to, as worst horizontal penalty
    /// across the grid.
    Oracle {
        name: &'static str,
        ber: fn(f64) -> f64,
        tolerance_db: f64,
    },
    /// Oracle-matched through the chain's own documented offset at one BER — the honest
    /// mapping where no closed form describes the detector exactly. The M = 2 discriminator
    /// row is the case: neither the coherent nor exactly the noncoherent detector, and its
    /// framing overhead is charged to Eb, so the offset documents the *chain*.
    OffsetOracle {
        name: &'static str,
        ber: fn(f64) -> f64,
        at_ber: f64,
        offset_db: f64,
        tolerance_db: f64,
    },
}

/// One committed curve and everything needed to reproduce and judge it.
#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    /// Artifact stem under [`BASELINE_DIR`], without extension (e.g. `"cpm/msk_awgn"`).
    pub stem: &'static str,
    pub link: fn() -> Link,
    /// The committed sweep.
    pub full: Tier,
    /// Grid points the smoke tier measures, from the front of `full.grid`, at the same seed
    /// and budgets.
    pub smoke_points: usize,
    pub reference: Reference,
}

impl Measurement {
    /// A commit-and-guard measurement at the crate's standard error budget — every CPM row.
    #[must_use]
    pub const fn committed(
        stem: &'static str,
        link: fn() -> Link,
        grid: &'static [f64],
        seed: u64,
        max_trial_bits: u64,
    ) -> Self {
        Self {
            stem,
            link,
            full: Tier {
                grid,
                seed,
                min_errors: FULL_ERRORS,
                max_trial_bits,
            },
            smoke_points: SMOKE_POINTS,
            reference: Reference::Committed,
        }
    }

    /// This measurement's sweep at one tier: the committed grid, or its smoke prefix.
    #[must_use]
    pub fn tier(&self, full: bool) -> Tier {
        let mut tier = self.full;
        if !full {
            tier.grid = &tier.grid[..self.smoke_points.min(tier.grid.len())];
        }
        tier
    }

    /// Path of the committed artifact relative to the workspace root.
    #[must_use]
    pub fn artifact(&self) -> String {
        format!("{BASELINE_DIR}/{}.json", self.stem)
    }

    #[must_use]
    pub fn reference_gap(&self, curve: &Curve) -> Option<(String, f64, f64)> {
        let grid = curve.points.first()?.ebn0_db;
        let last = curve.points.last()?.ebn0_db;
        match self.reference {
            Reference::Committed => None,
            Reference::Oracle {
                name,
                ber,
                tolerance_db,
            } => Some((
                name.to_string(),
                worst_penalty_db(curve, ber, grid, last),
                tolerance_db,
            )),
            Reference::OffsetOracle {
                name,
                ber,
                at_ber,
                offset_db,
                tolerance_db,
            } => {
                // The offset is defined at one BER, so a tier whose grid stops above that
                // crossing simply cannot answer — a smoke prefix sits on the shoulder. Silent
                // rather than failing: the drift gate against the committed artifact still
                // judges every tier.
                let measured = penalty_db(curve, ber, at_ber);
                if !measured.is_finite() {
                    return None;
                }
                // The gap judged is the distance from the *documented* offset, so a chain that
                // drifted away from its recorded discriminator loss fails even though it still
                // sits near theory.
                Some((
                    format!("{name} + documented {offset_db:+.2} dB at BER {at_ber:.0e}"),
                    measured - offset_db,
                    tolerance_db,
                ))
            }
        }
    }

    /// Worst horizontal drift of a freshly measured curve from the committed one, over the
    /// measured grid's span.
    #[must_use]
    pub fn drift_db(&self, curve: &Curve, committed: &Curve) -> Option<f64> {
        let lo = curve.points.first()?.ebn0_db;
        let hi = curve.points.last()?.ebn0_db;
        Some(worst_penalty_db_vs_curve(curve, committed, lo, hi))
    }
}

/// One catalog entry as the harness knows it — the name `cargo xtask ber <entry>` takes.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub name: &'static str,
    pub measurements: &'static [Measurement],
}

const BPSK_IDEAL: &[Measurement] = &[Measurement {
    stem: "bpsk_ideal_awgn",
    link: super::reference::ideal_bpsk,
    full: Tier {
        grid: &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
        seed: 0x5eed,
        min_errors: 10_000,
        max_trial_bits: 50_000_000,
    },
    smoke_points: SMOKE_POINTS,
    reference: Reference::Oracle {
        name: "exact ½·erfc(√γ)",
        ber: theory::bpsk_ber,
        tolerance_db: 0.2,
    },
}];

/// Every entry with a runner, in catalog order. A phase adds a row here and the command, the
/// crate's gates and `CATALOG.md`'s runner column all learn about it at once.
pub const ENTRIES: &[Entry] = &[
    Entry {
        name: "bpsk-ideal",
        measurements: BPSK_IDEAL,
    },
    Entry {
        name: "mfsk",
        measurements: mfsk::MEASUREMENTS,
    },
    Entry {
        name: "gmsk",
        measurements: gmsk::MEASUREMENTS,
    },
    Entry {
        name: "msk",
        measurements: msk::MEASUREMENTS,
    },
    Entry {
        name: "afsk",
        measurements: afsk::MEASUREMENTS,
    },
    Entry {
        name: "ask",
        measurements: ask::MEASUREMENTS,
    },
    Entry {
        name: "psk",
        measurements: psk::COHERENT,
    },
    Entry {
        name: "dpsk",
        measurements: psk::DIFFERENTIAL,
    },
    Entry {
        name: "oqpsk",
        measurements: psk::OFFSET,
    },
    Entry {
        name: "pi4-dqpsk",
        measurements: psk::PI4,
    },
    Entry {
        name: "mfsk-orthogonal",
        measurements: orthogonal::MEASUREMENTS,
    },
    Entry {
        name: "ppm",
        measurements: ppm::MEASUREMENTS,
    },
    Entry {
        name: "qam",
        measurements: qam::SQUARE,
    },
    Entry {
        name: "qam-cross",
        measurements: qam::CROSS,
    },
    Entry {
        name: "qam-star",
        measurements: qam::STAR,
    },
    Entry {
        name: "qam-nonuniform",
        measurements: qam::HIERARCHICAL,
    },
    Entry {
        name: "apsk",
        measurements: qam::APSK,
    },
    Entry {
        name: "ofdm",
        measurements: ofdm::MODULATIONS,
    },
    Entry {
        name: "ofdm-genie",
        measurements: ofdm::GENIE,
    },
    Entry {
        name: "ofdm-estimation",
        measurements: ofdm::ESTIMATION,
    },
    Entry {
        name: "dmt",
        measurements: ofdm::DMT,
    },
    Entry {
        name: "gfdm",
        measurements: multicarrier::GFDM,
    },
    Entry {
        name: "ufmc",
        measurements: multicarrier::UFMC,
    },
    Entry {
        name: "fbmc",
        measurements: multicarrier::FBMC,
    },
    Entry {
        name: "otfs",
        measurements: multicarrier::OTFS,
    },
    Entry {
        name: "dsss",
        measurements: spread::DSSS,
    },
    Entry {
        name: "cck",
        measurements: spread::CCK,
    },
    Entry {
        name: "css",
        measurements: spread::CSS,
    },
    Entry {
        name: "fhss",
        measurements: spread::FHSS,
    },
];

impl Entry {
    /// Whether this entry's artifacts live under the linear engine's baseline directory — how the
    /// crate's own tooling picks the linear rows out of the registry without a second list.
    #[must_use]
    pub fn stem_prefix_is_linear(&self) -> bool {
        self.measurements
            .iter()
            .all(|m| m.stem.starts_with("linear/"))
    }
}

/// The entry registered under `name`, if any.
#[must_use]
pub fn find(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

/// The measurement that owns an artifact stem, whichever entry it belongs to — how a gate
/// names a curve without also restating the link, grid, seed and budget it was measured with.
#[must_use]
pub fn measurement(stem: &str) -> Option<&'static Measurement> {
    ENTRIES
        .iter()
        .flat_map(|entry| entry.measurements)
        .find(|m| m.stem == stem)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    /// A registered measurement whose artifact is missing is a runner that cannot be judged —
    /// and the docs-row rule would never catch it, because that rule reads the catalog against
    /// the tree, not against this registry.
    #[test]
    fn every_registered_measurement_has_its_committed_artifact() {
        let root = workspace_root();
        for entry in ENTRIES {
            assert!(
                !entry.measurements.is_empty(),
                "entry `{}` registers no measurement",
                entry.name
            );
            for m in entry.measurements {
                let path = root.join(m.artifact());
                assert!(path.is_file(), "{}: {} missing", entry.name, m.artifact());
            }
        }
    }

    /// Entry names are the command's public surface and artifact stems are file paths: a
    /// duplicate of either silently shadows a measurement.
    #[test]
    fn entry_names_and_artifact_stems_are_unique() {
        let mut names: Vec<&str> = ENTRIES.iter().map(|e| e.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate entry name");

        let mut stems: Vec<&str> = ENTRIES
            .iter()
            .flat_map(|e| e.measurements.iter().map(|m| m.stem))
            .collect();
        let count = stems.len();
        stems.sort_unstable();
        stems.dedup();
        assert_eq!(stems.len(), count, "duplicate artifact stem");
    }

    /// The smoke tier is a gate only because it reproduces committed points: a prefix longer
    /// than the grid, or a grid too short to place a shoulder, would quietly make it something
    /// else.
    #[test]
    fn every_smoke_tier_is_a_prefix_of_its_committed_grid() {
        for entry in ENTRIES {
            for m in entry.measurements {
                let full = m.tier(true);
                let smoke = m.tier(false);
                assert!(
                    full.grid.len() >= SMOKE_POINTS,
                    "{}: committed grid is shorter than the smoke tier",
                    m.stem
                );
                assert_eq!(smoke.grid, &full.grid[..SMOKE_POINTS], "{}", m.stem);
                assert_eq!(smoke.seed, full.seed, "{}", m.stem);
                assert_eq!(smoke.min_errors, full.min_errors, "{}", m.stem);
            }
        }
    }

    /// Grids are read as ascending Eb/N0 by every consumer that interpolates a crossing
    /// ([`worst_penalty_db_vs_curve`], the limits runner's sensitivity search).
    #[test]
    fn every_grid_ascends() {
        for entry in ENTRIES {
            for m in entry.measurements {
                let grid = m.full.grid;
                assert!(
                    grid.windows(2).all(|w| w[1] > w[0]),
                    "{}: grid is not ascending: {grid:?}",
                    m.stem
                );
            }
        }
    }
}
