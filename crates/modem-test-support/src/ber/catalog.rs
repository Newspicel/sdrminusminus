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

pub const BASELINE_DIR: &str = "crates/modem/baselines";

pub const SMOKE_POINTS: usize = 3;

pub const FULL_ERRORS: u64 = 2_000;

pub const DRIFT_TOLERANCE_DB: f64 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct Tier {
    pub grid: &'static [f64],
    pub seed: u64,
    pub min_errors: u64,
    pub max_trial_bits: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum Reference {
    Committed,
    Oracle {
        name: &'static str,
        ber: fn(f64) -> f64,
        tolerance_db: f64,
    },
    OffsetOracle {
        name: &'static str,
        ber: fn(f64) -> f64,
        at_ber: f64,
        offset_db: f64,
        tolerance_db: f64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    pub stem: &'static str,
    pub link: fn() -> Link,
    pub full: Tier,
    pub smoke_points: usize,
    pub reference: Reference,
}

impl Measurement {
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

    #[must_use]
    pub fn tier(&self, full: bool) -> Tier {
        let mut tier = self.full;
        if !full {
            tier.grid = &tier.grid[..self.smoke_points.min(tier.grid.len())];
        }
        tier
    }

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
                let measured = penalty_db(curve, ber, at_ber);
                if !measured.is_finite() {
                    return None;
                }
                Some((
                    format!("{name} + documented {offset_db:+.2} dB at BER {at_ber:.0e}"),
                    measured - offset_db,
                    tolerance_db,
                ))
            }
        }
    }

    #[must_use]
    pub fn drift_db(&self, curve: &Curve, committed: &Curve) -> Option<f64> {
        let lo = curve.points.first()?.ebn0_db;
        let hi = curve.points.last()?.ebn0_db;
        Some(worst_penalty_db_vs_curve(curve, committed, lo, hi))
    }
}

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
    #[must_use]
    pub fn stem_prefix_is_linear(&self) -> bool {
        self.measurements
            .iter()
            .all(|m| m.stem.starts_with("linear/"))
    }
}

#[must_use]
pub fn find(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

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
