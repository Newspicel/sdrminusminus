//! The resistance runner (MODEM-PLAN §4.3): *where an entry fails, measured*. For every
//! impairment axis the runner binary-searches the largest level the entry survives while held
//! at a fixed operating point — its measured 1e-3 sensitivity plus [`SENSITIVITY_MARGIN_DB`] —
//! and commits the result as the entry's limits table: robustness as numbers, not adjectives.
//! Sensitivity is measured first because every axis row's operating point is defined off it;
//! an axis row without a sensitivity has no stated meaning.
//!
//! Two pass criteria exist (§4.3), and every row records which one produced it: the default
//! failure floor — BER above [`FAILURE_BER`] at the operating point — and the "≤ 1 dB Eb/N0
//! penalty" form the plan states for the tracking axes (CFO, drift, sample-clock ppm, phase
//! noise, …). A threshold without its criterion is not a measurement, and [`compare_tables`]
//! refuses to compare thresholds taken under different ones.
//!
//! Determinism is inherited from the sweep: an axis search is a fixed sequence of seeded
//! one-point BER measurements, so every committed threshold regenerates bit-for-bit from its
//! seed and search bracket. Regression is one-sided by design — a number moving *better* than
//! the committed table is never a failure; only worsening past tolerance is (§4.3: limits
//! tables regress like curves).

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::{
    Curve, FAILURE_BER, SENSITIVITY_MARGIN_DB,
    impair::{Cfo, ChannelSpec, Drift, Multipath, MultipathProfile, PhaseNoise},
    sweep::{Link, sweep_ber},
};

// --- The committed artifact ------------------------------------------------------------------

/// One row of a limits table (§4.3): the largest level of `axis` (stated in `unit`) at which
/// the entry still met `criterion` — or, for `profile:` rows, the measured degradation a named
/// composite profile costs. The criterion string rides in the row because a threshold is
/// uninterpretable without its pass condition, and the regression comparator must refuse to
/// compare thresholds measured under different ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitRow {
    pub axis: String,
    pub unit: String,
    pub threshold: f64,
    pub criterion: String,
}

/// The committed resistance artifact for one catalog entry (§4.3). `seed` names the run every
/// number regenerates from (§4.1: fixed seeds everywhere). The sensitivities are `Option`
/// because a swept span that never reached a ratio has *not measured* it — `None` must stay
/// distinguishable from any number. Composite-profile degradations live in `rows` under
/// `profile:<name>` axes, in dB, so one comparator guards the whole table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitsTable {
    pub entry: String,
    pub seed: u64,
    pub sensitivity_db_1e2: Option<f64>,
    pub sensitivity_db_1e3: Option<f64>,
    pub sensitivity_db_1e4: Option<f64>,
    pub rows: Vec<LimitRow>,
}

impl LimitsTable {
    /// Starts a table from a measured [`Sensitivity`] — the §4.3 ordering made structural:
    /// no axis row can exist before the sensitivity that defines its operating point.
    #[must_use]
    pub fn new(entry: impl Into<String>, seed: u64, sensitivity: &Sensitivity) -> Self {
        Self {
            entry: entry.into(),
            seed,
            sensitivity_db_1e2: sensitivity.db_at_1e2,
            sensitivity_db_1e3: sensitivity.db_at_1e3,
            sensitivity_db_1e4: sensitivity.db_at_1e4,
            rows: Vec::new(),
        }
    }

    /// The operating point every axis closure measures at: the 1e-3 sensitivity plus the
    /// standing margin (§4.3). `None` while 1e-3 is unmeasured — an axis search without an
    /// operating point measures nothing defined.
    #[must_use]
    pub fn operating_point_db(&self) -> Option<f64> {
        self.sensitivity_db_1e3.map(|db| db + SENSITIVITY_MARGIN_DB)
    }
}

/// Writes the table as pretty JSON — the same committed-artifact format as the sweep's
/// curves, human-diffable so a regression review reads exactly which threshold moved.
pub fn save_json(table: &LimitsTable, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(table).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<LimitsTable> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

// --- Sensitivity (§4.3 row one) --------------------------------------------------------------

/// A link's measured clean-channel sensitivity: the Eb/N0 at BER 1e-2 / 1e-3 / 1e-4, read off
/// one [`sweep_ber`] curve by inverse interpolation. The curve rides along so the later steps
/// — the ≤ 1 dB-penalty criterion, profile degradation — reuse the measurement instead of
/// resweeping it.
#[derive(Clone, Debug)]
pub struct Sensitivity {
    pub curve: Curve,
    pub db_at_1e2: Option<f64>,
    pub db_at_1e3: Option<f64>,
    pub db_at_1e4: Option<f64>,
}

/// Measures the sensitivity rows. `points_db` must bracket every ratio the caller intends to
/// read: a crossing outside the swept span reports `None`, never an extrapolation —
/// extrapolating the number every other row's operating point hangs off would let one
/// optimistic grid poison a whole table.
pub fn measure_sensitivity(
    link: &Link,
    channel_template: &ChannelSpec,
    points_db: &[f64],
    seed: u64,
    min_errors: u64,
    max_trial_bits: u64,
) -> Sensitivity {
    let curve = sweep_ber(
        link,
        channel_template,
        points_db,
        seed,
        min_errors,
        max_trial_bits,
    );
    Sensitivity {
        db_at_1e2: ebn0_at_ber(&curve, 1e-2),
        db_at_1e3: ebn0_at_ber(&curve, 1e-3),
        db_at_1e4: ebn0_at_ber(&curve, 1e-4),
        curve,
    }
}

// --- Curve interpolation ---------------------------------------------------------------------
//
// Both directions are linear in (dB, log10 BER) — the convention the sweep's comparators use,
// because a waterfall is near-exponential in dB and the log domain is where a straight segment
// approximates it honestly. Nothing extrapolates: outside the measured span the honest answer
// is None, and the §4.3 fix for that is a wider grid, not a projected number.

/// The Eb/N0 at which `curve` crosses `ber`, or `None` when the measured span never reaches
/// it — the §4.3 sensitivity read.
#[must_use]
pub fn ebn0_at_ber(curve: &Curve, ber: f64) -> Option<f64> {
    if ber <= 0.0 || !ber.is_finite() {
        return None;
    }
    let pts = usable_log_points(curve);
    let target = ber.log10();
    for pair in pts.windows(2) {
        let (db_a, la) = pair[0];
        let (db_b, lb) = pair[1];
        if (la - target) * (lb - target) <= 0.0 {
            if (la - lb).abs() < 1e-12 {
                return Some(db_a);
            }
            return Some(db_a + (la - target) / (la - lb) * (db_b - db_a));
        }
    }
    None
}

/// The interpolated BER at `ebn0_db`, or `None` outside the measured span — the forward read
/// [`penalty_criterion`] needs to state its pass BER.
#[must_use]
pub fn ber_at_ebn0(curve: &Curve, ebn0_db: f64) -> Option<f64> {
    let pts = usable_log_points(curve);
    for pair in pts.windows(2) {
        let (db_a, la) = pair[0];
        let (db_b, lb) = pair[1];
        if (ebn0_db - db_a) * (ebn0_db - db_b) <= 0.0 {
            // A zero-width segment states no slope; an exact hit is still an answer.
            if (db_b - db_a).abs() < 1e-12 {
                return Some(10f64.powf(la));
            }
            let t = (ebn0_db - db_a) / (db_b - db_a);
            return Some(10f64.powf(la + t * (lb - la)));
        }
    }
    None
}

/// (dB, log₁₀ rate) for every usable point. Points with no trials carry no information;
/// errorless points enter as their bound `0.5 / trials` — real "at most this" information,
/// under the same convention the sweep's comparators interpolate with, so a sensitivity read
/// here and a penalty read there can never disagree about what a curve says.
fn usable_log_points(curve: &Curve) -> Vec<(f64, f64)> {
    curve
        .points
        .iter()
        .filter(|p| p.trials > 0)
        .map(|p| {
            let rate = if p.errors == 0 {
                0.5 / p.trials as f64
            } else {
                p.rate()
            };
            (p.ebn0_db, rate.log10())
        })
        .collect()
}

// --- Failure criteria (§4.3) -----------------------------------------------------------------

/// The pass condition an axis row was measured under. Both reduce to "measured BER at the
/// operating point stays at or below a limit", so one search serves both — but the limits
/// differ by orders of magnitude, and a row must say which one its threshold means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Criterion {
    /// The §4.3 default: post-detection BER may not exceed [`FAILURE_BER`] while the entry
    /// operates [`SENSITIVITY_MARGIN_DB`] above its measured 1e-3 sensitivity.
    FailureBer,
    /// The plan's "≤ N dB penalty" rows: pass while the measured BER stays at or below
    /// `max_ber` — the clean link's own BER `penalty_db` below the operating point, resolved
    /// once by [`penalty_criterion`]. Carrying the resolved BER keeps every probe of the
    /// search a plain comparison instead of a curve read per axis value.
    MaxPenalty { penalty_db: f64, max_ber: f64 },
}

impl Criterion {
    /// The BER a probe must stay at or below to pass.
    #[must_use]
    pub fn ber_limit(self) -> f64 {
        match self {
            Self::FailureBer => FAILURE_BER,
            Self::MaxPenalty { max_ber, .. } => max_ber,
        }
    }

    /// The string recorded in the row. Deliberately free of measured numbers: two runs of the
    /// same harness must produce byte-identical criteria, or [`compare_tables`] would refuse
    /// to compare their thresholds.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::FailureBer => {
                format!("BER <= {FAILURE_BER:e} at sensitivity(1e-3) + {SENSITIVITY_MARGIN_DB} dB")
            }
            Self::MaxPenalty { penalty_db, .. } => {
                format!(
                    "<= {penalty_db} dB Eb/N0 penalty at sensitivity(1e-3) + {SENSITIVITY_MARGIN_DB} dB"
                )
            }
        }
    }
}

/// Builds the "≤ `penalty_db` dB penalty" criterion from the entry's own clean sensitivity
/// curve: an impairment costs at most `penalty_db` exactly when the BER it produces at the
/// operating point is no worse than what the clean link measures `penalty_db` lower. `None`
/// when the curve does not cover that point — a criterion that cannot state its pass BER must
/// not silently become a different criterion.
#[must_use]
pub fn penalty_criterion(clean: &Curve, op_ebn0_db: f64, penalty_db: f64) -> Option<Criterion> {
    let max_ber = ber_at_ebn0(clean, op_ebn0_db - penalty_db)?;
    Some(Criterion::MaxPenalty {
        penalty_db,
        max_ber,
    })
}

// --- The axis runner -------------------------------------------------------------------------

/// One seeded single-point BER measurement — the intended body of an axis-search closure.
/// The same `seed` is passed deliberately at every axis value (common random numbers): probes
/// then differ only in the impairment level, never in noise luck, which is what makes the
/// search's pass/fail boundary a property of the axis. An impossible empty sweep reads as
/// BER 1.0 — certain failure, never a silent pass.
pub fn measure_ber(
    link: &Link,
    spec: &ChannelSpec,
    ebn0_db: f64,
    seed: u64,
    min_errors: u64,
    max_trial_bits: u64,
) -> f64 {
    let curve = sweep_ber(link, spec, &[ebn0_db], seed, min_errors, max_trial_bits);
    curve.points.first().map_or(1.0, |p| p.rate())
}

/// Hard cap on bisection steps: 64 halvings take any bracket below f64 resolution, so the cap
/// never truncates a real search — it exists so a zero (or NaN) tolerance still terminates
/// deterministically.
const MAX_SEARCH_ITERS: u32 = 64;

/// Binary-searches the largest axis value in `[0, max_axis]` still meeting `criterion`
/// (§4.3). `ber_at` measures the BER at one axis value with the link held at its operating
/// point — sensitivity(1e-3) + [`SENSITIVITY_MARGIN_DB`] — and must be deterministic per
/// value ([`measure_ber`] with a fixed seed is the intended body). A NaN reading counts as a
/// failure: an unmeasurable point must shrink the claimed limit, never extend it.
///
/// Return semantics, all deterministic: `0.0` means the link fails even unimpaired (itself a
/// finding, not an error); `max_axis` means no failure inside the bracket — the bracket, not
/// the link, bounded the answer, so widen it if this row's value matters; anything else is the
/// largest *probed passing* value, within `tolerance` of the true boundary.
pub fn search_axis_limit(
    criterion: Criterion,
    max_axis: f64,
    tolerance: f64,
    ber_at: impl Fn(f64) -> f64,
) -> f64 {
    let limit = criterion.ber_limit();
    let passes = |value: f64| ber_at(value) <= limit;
    if !passes(0.0) {
        return 0.0;
    }
    if passes(max_axis) {
        return max_axis;
    }
    let (mut lo, mut hi) = (0.0f64, max_axis);
    for _ in 0..MAX_SEARCH_ITERS {
        if hi - lo <= tolerance {
            break;
        }
        let mid = 0.5 * (lo + hi);
        if passes(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// [`search_axis_limit`] plus the row bookkeeping, so every entry's rows come out shaped
/// identically — same axis vocabulary, same criterion strings — which is what lets one
/// comparator guard every table in the catalog.
pub fn measure_axis_row(
    axis: impl Into<String>,
    unit: impl Into<String>,
    criterion: Criterion,
    max_axis: f64,
    tolerance: f64,
    ber_at: impl Fn(f64) -> f64,
) -> LimitRow {
    LimitRow {
        axis: axis.into(),
        unit: unit.into(),
        threshold: search_axis_limit(criterion, max_axis, tolerance, ber_at),
        criterion: criterion.label(),
    }
}

// --- Composite profiles (§4.3 combined stress) -----------------------------------------------

/// The named composite stress profiles: several axes at once, at documented levels, because
/// fielded receivers never see one impairment at a time. Levels are stated in the impairment
/// models' own rate-free per-sample units — the physical reading follows from an entry's
/// sample rate, and the doc comments give the symbol-relative view at the harness's canonical
/// 8 samples/symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositeProfile {
    /// Dense-scatter urban mobile: exponential PDP with 2 samples RMS delay spread (¼ symbol
    /// at 8 sps) over 8 taps, residual CFO of 1e-4 cycles/sample, slow drift of 1e-11
    /// cycles/sample² (≈23 Hz/s at 48 kHz), and 2° RMS integrated phase noise. Meant for
    /// entries with tracking loops — an open-loop chain measures ∞ degradation here, loudly,
    /// which is the honest number for a receiver that cannot track at all.
    MobileUrban,
    /// Benign static indoor: one weak reflection (1 sample at −12 dB, 1 rad) and 1° RMS phase
    /// noise — the profile every entry, even an open-loop reference chain, should survive
    /// with a fraction of a dB.
    StaticIndoor,
}

impl CompositeProfile {
    /// The name limits tables and axis strings cite, e.g. `profile:mobile-urban`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::MobileUrban => "mobile-urban",
            Self::StaticIndoor => "static-indoor",
        }
    }

    /// The profile's axes at their documented levels, layered onto `base`. AWGN is left alone
    /// deliberately: noise stays the sweep's own axis, applied canonically last, so a
    /// degradation sweep through a profile still states true Eb/N0 per point.
    #[must_use]
    pub fn apply(self, base: ChannelSpec) -> ChannelSpec {
        match self {
            Self::MobileUrban => base
                .multipath(Multipath::new(MultipathProfile::ExponentialPdp {
                    rms_delay_spread_samples: 2.0,
                    taps: 8,
                }))
                .cfo(Cfo::from_cycles_per_sample(1e-4))
                // Sample rate 1.0 makes the argument cycles/sample² directly — the only
                // rate-free spelling the constructor admits.
                .drift(Drift::from_hz_per_s(1e-11, 1.0))
                .phase_noise(PhaseNoise::new(2.0)),
            Self::StaticIndoor => base
                .multipath(Multipath::new(MultipathProfile::TwoRay {
                    delay_samples: 1,
                    relative_db: -12.0,
                    phase_rad: 1.0,
                }))
                .phase_noise(PhaseNoise::new(1.0)),
        }
    }
}

/// Criterion string of the `profile:` degradation rows. [`compare_tables`] keys a row's
/// worse-direction off it: degradation grows when things get worse, while every axis
/// threshold shrinks.
pub const DEGRADATION_CRITERION: &str = "Eb/N0 degradation at BER 1e-3";

/// The horizontal cost of `impaired` vs `clean` at `at_ber`, in dB. +∞ when either curve
/// misses the crossing: a profile that pushes the error floor above `at_ber` has degraded the
/// link beyond measure, and the infinite value fails any `< tolerance` gate instead of
/// vacuously passing it — the sweep comparators' convention.
#[must_use]
pub fn degradation_db(impaired: &Curve, clean: &Curve, at_ber: f64) -> f64 {
    match (ebn0_at_ber(impaired, at_ber), ebn0_at_ber(clean, at_ber)) {
        (Some(i), Some(c)) => i - c,
        _ => f64::INFINITY,
    }
}

/// Sweeps the link clean and under `profile` — same seed, so the two curves differ only by
/// the profile, not by noise realisation — and returns the measured degradation as the row
/// recorded alongside the axis rows.
pub fn measure_profile_degradation(
    link: &Link,
    base: &ChannelSpec,
    profile: CompositeProfile,
    points_db: &[f64],
    seed: u64,
    min_errors: u64,
    max_trial_bits: u64,
) -> LimitRow {
    let clean = sweep_ber(link, base, points_db, seed, min_errors, max_trial_bits);
    let impaired = sweep_ber(
        link,
        &profile.apply(*base),
        points_db,
        seed,
        min_errors,
        max_trial_bits,
    );
    LimitRow {
        axis: format!("profile:{}", profile.name()),
        unit: "dB".to_string(),
        threshold: degradation_db(&impaired, &clean, 1e-3),
        criterion: DEGRADATION_CRITERION.to_string(),
    }
}

// --- Regression comparison (§4.3: limits tables regress like curves) -------------------------

/// Compares a fresh measurement against the committed table. One-sided by design: moving
/// *better* is never a failure; each committed number may move worse by at most
/// `tolerance_fraction` of its own magnitude, applied per row (sensitivities compare in dB
/// under the same rule). A committed row or sensitivity the measurement no longer produces is
/// a regression — a vanished measurement is worse than a smaller one — and a row whose unit
/// or criterion changed is flagged rather than compared, because those thresholds are not the
/// same quantity. `Err` lists every violation, so one CI run reports the whole damage rather
/// than the first row of it.
pub fn compare_tables(
    measured: &LimitsTable,
    committed: &LimitsTable,
    tolerance_fraction: f64,
) -> Result<(), Vec<String>> {
    let mut faults = Vec::new();
    let sensitivities = [
        (
            "sensitivity at BER 1e-2",
            measured.sensitivity_db_1e2,
            committed.sensitivity_db_1e2,
        ),
        (
            "sensitivity at BER 1e-3",
            measured.sensitivity_db_1e3,
            committed.sensitivity_db_1e3,
        ),
        (
            "sensitivity at BER 1e-4",
            measured.sensitivity_db_1e4,
            committed.sensitivity_db_1e4,
        ),
    ];
    for (what, m, c) in sensitivities {
        compare_sensitivity(what, m, c, tolerance_fraction, &mut faults);
    }
    for row in &committed.rows {
        compare_row(measured, row, tolerance_fraction, &mut faults);
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults)
    }
}

/// Sensitivity is "lower is better": needing more Eb/N0 for the same BER is the regression.
/// A committed value the measurement never reached fails too — losing a measurement is worse
/// than worsening it. A ratio the committed table itself never measured guards nothing.
fn compare_sensitivity(
    what: &str,
    measured: Option<f64>,
    committed: Option<f64>,
    tolerance_fraction: f64,
    faults: &mut Vec<String>,
) {
    let Some(c) = committed else { return };
    let Some(m) = measured else {
        faults.push(format!(
            "{what}: committed {c} dB, but the measurement never reached it"
        ));
        return;
    };
    let allowance = tolerance_fraction * c.abs();
    if m.is_nan() || m > c + allowance {
        faults.push(format!(
            "{what}: {c} dB -> {m} dB (allowed worsening {allowance} dB)"
        ));
    }
}

/// Axis thresholds are "higher is better" — tolerating more of an impairment — except the
/// `profile:` degradation rows, where the recorded number is a cost and grows when things get
/// worse; [`DEGRADATION_CRITERION`] is what tells the two apart. Two equal infinities (a
/// profile committed as unmeasurable and still unmeasurable) subtract to NaN and correctly
/// pass; an explicit NaN threshold never compares "worse", so it is rejected by name — a
/// table with an unmeasurable number in it must not pass a regression gate.
fn compare_row(
    measured: &LimitsTable,
    committed: &LimitRow,
    tolerance_fraction: f64,
    faults: &mut Vec<String>,
) {
    let axis = &committed.axis;
    let Some(m) = measured.rows.iter().find(|r| r.axis == *axis) else {
        faults.push(format!(
            "row '{axis}': committed, but missing from the measurement"
        ));
        return;
    };
    if m.unit != committed.unit || m.criterion != committed.criterion {
        faults.push(format!(
            "row '{axis}': unit or criterion changed; the thresholds are not the same quantity"
        ));
        return;
    }
    let worse_by = if committed.criterion == DEGRADATION_CRITERION {
        m.threshold - committed.threshold
    } else {
        committed.threshold - m.threshold
    };
    let allowance = tolerance_fraction * committed.threshold.abs();
    if m.threshold.is_nan() || committed.threshold.is_nan() || worse_by > allowance {
        faults.push(format!(
            "row '{axis}': threshold {} -> {} {} (allowed worsening {allowance})",
            committed.threshold, m.threshold, m.unit
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::{CurvePoint, reference::ideal_bpsk, theory};

    /// A synthetic curve read straight off the BPSK oracle on a 1 dB grid — exact counts, so
    /// interpolation tests read grid curvature, not counting noise.
    fn synth_bpsk_curve() -> Curve {
        Curve {
            label: "synthetic bpsk".to_string(),
            points: (0..=12)
                .map(|db| {
                    let rate = theory::bpsk_ber(f64::from(db));
                    CurvePoint {
                        ebn0_db: f64::from(db),
                        errors: (rate * 1e12) as u64,
                        trials: 1_000_000_000_000,
                    }
                })
                .collect(),
        }
    }

    fn sample_table() -> LimitsTable {
        LimitsTable {
            entry: "ideal-bpsk".to_string(),
            seed: 0x5eed,
            sensitivity_db_1e2: Some(4.32),
            sensitivity_db_1e3: Some(6.79),
            sensitivity_db_1e4: None,
            rows: vec![
                LimitRow {
                    axis: "static CFO".to_string(),
                    unit: "cycles/sample".to_string(),
                    threshold: 120.0e-6,
                    criterion: Criterion::FailureBer.label(),
                },
                LimitRow {
                    axis: "profile:mobile-urban".to_string(),
                    unit: "dB".to_string(),
                    threshold: 1.5,
                    criterion: DEGRADATION_CRITERION.to_string(),
                },
            ],
        }
    }

    /// §4.3 row one on the calibration link: measured sensitivity sits on the closed form.
    /// 300 errors per point keep the 1e-3 crossing's counting noise near 0.06 dB — well
    /// inside the 0.3 dB asserted (the sweep's own tests document the horizontal-CI budget).
    #[test]
    fn sensitivity_matches_theory() {
        let link = ideal_bpsk();
        let sens = measure_sensitivity(
            &link,
            &ChannelSpec::default(),
            &[4.0, 5.0, 6.0, 7.0],
            0x11317,
            300,
            1_000_000,
        );
        let s2 = sens.db_at_1e2.unwrap();
        let s3 = sens.db_at_1e3.unwrap();
        assert!((s2 - 4.323).abs() < 0.3, "1e-2 sensitivity {s2} dB");
        assert!((s3 - 6.7895).abs() < 0.3, "1e-3 sensitivity {s3} dB");
        // The grid tops out near BER 8e-4: 1e-4 was never reached and must read as unmeasured.
        assert!(sens.db_at_1e4.is_none());
        let table = LimitsTable::new("ideal-bpsk", 0x11317, &sens);
        assert_eq!(table.sensitivity_db_1e3, sens.db_at_1e3);
        let op = table.operating_point_db().unwrap();
        assert!((op - s3 - SENSITIVITY_MARGIN_DB).abs() < 1e-12);
    }

    #[test]
    fn crossings_interpolate_in_log_ber() {
        let curve = synth_bpsk_curve();
        // Recovered crossing within the 1 dB grid's log-domain curvature (~0.02 dB at the
        // knee — the same bound the sweep's comparator tests state).
        let db3 = ebn0_at_ber(&curve, 1e-3).unwrap();
        assert!((db3 - 6.7895).abs() < 0.03, "1e-3 at {db3} dB");
        // Forward and inverse reads use the same segments, so they must agree exactly.
        let back = ber_at_ebn0(&curve, db3).unwrap();
        assert!((back.log10() + 3.0).abs() < 1e-9, "round trip {back:e}");
        // Outside the measured span the honest answer is None, in both directions.
        assert!(ebn0_at_ber(&curve, 1e-12).is_none());
        assert!(ebn0_at_ber(&curve, 0.9).is_none());
        assert!(ber_at_ebn0(&curve, -1.0).is_none());
        assert!(ber_at_ebn0(&curve, 12.5).is_none());
        // Nonsense targets cross nothing.
        assert!(ebn0_at_ber(&curve, 0.0).is_none());
        assert!(ebn0_at_ber(&curve, f64::NAN).is_none());
    }

    #[test]
    fn errorless_points_carry_their_bound_and_empty_points_nothing() {
        let curve = Curve {
            label: "bound".to_string(),
            points: vec![
                CurvePoint {
                    ebn0_db: 8.0,
                    errors: 100,
                    trials: 100_000,
                },
                CurvePoint {
                    ebn0_db: 10.0,
                    errors: 0,
                    trials: 1_000_000,
                },
            ],
        };
        // The errorless point enters as its bound 5e-7, so the 1e-4 crossing is bracketed.
        let db = ebn0_at_ber(&curve, 1e-4).unwrap();
        assert!(db > 8.0 && db < 10.0, "crossing {db} dB");
        // A point with no trials states nothing; one usable point leaves no segment.
        let broken = Curve {
            label: "broken".to_string(),
            points: vec![
                CurvePoint {
                    ebn0_db: 8.0,
                    errors: 0,
                    trials: 0,
                },
                CurvePoint {
                    ebn0_db: 10.0,
                    errors: 50,
                    trials: 1_000,
                },
            ],
        };
        assert!(ebn0_at_ber(&broken, 5e-2).is_none());
    }

    #[test]
    fn criteria_state_their_limits_and_labels() {
        assert!((Criterion::FailureBer.ber_limit() - FAILURE_BER).abs() < 1e-18);
        assert_eq!(
            Criterion::FailureBer.label(),
            "BER <= 1e-2 at sensitivity(1e-3) + 3 dB"
        );
        let crit = penalty_criterion(&synth_bpsk_curve(), 9.79, 1.0).unwrap();
        let Criterion::MaxPenalty {
            penalty_db,
            max_ber,
        } = crit
        else {
            panic!("penalty_criterion built {crit:?}");
        };
        assert!((penalty_db - 1.0).abs() < 1e-12);
        // The pass BER is the clean curve read at op − 1 dB; the 1 dB grid's log-domain
        // curvature allows a few percent.
        let want = theory::bpsk_ber(8.79);
        assert!(
            (max_ber.log10() - want.log10()).abs() < 0.05,
            "max_ber {max_ber:e}, want {want:e}"
        );
        assert_eq!(
            crit.label(),
            "<= 1 dB Eb/N0 penalty at sensitivity(1e-3) + 3 dB"
        );
        // Off the measured span there is no criterion to state.
        assert!(penalty_criterion(&synth_bpsk_curve(), 0.5, 1.0).is_none());
        assert!(penalty_criterion(&synth_bpsk_curve(), 40.0, 1.0).is_none());
    }

    #[test]
    fn search_handles_degenerate_predicates() {
        // Never failing: the bracket, not the link, bounds the answer.
        let unbounded = search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| 0.0);
        assert!((unbounded - 8.0).abs() < 1e-15);
        // Failing unimpaired: the link tolerates none of the axis.
        assert!(search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| 1.0) == 0.0);
        // A NaN reading is a failure, never a pass.
        assert!(search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| f64::NAN) == 0.0);
        // A known step boundary is recovered to within the tolerance from below…
        let step = |v: f64| if v <= 0.37 { 0.0 } else { 1.0 };
        let found = search_axis_limit(Criterion::FailureBer, 1.0, 1e-6, step);
        assert!(found <= 0.37 && 0.37 - found < 1e-6, "found {found}");
        // …and a zero tolerance still terminates, pinned by the iteration cap at f64 depth.
        let exact = search_axis_limit(Criterion::FailureBer, 1.0, 0.0, step);
        assert!((exact - 0.37).abs() < 1e-15, "exact {exact}");
    }

    /// Both criteria drive the same runner; the penalty form is simply a stricter BER limit,
    /// so on a monotone axis it must find a smaller threshold.
    #[test]
    fn stricter_criterion_gives_smaller_threshold() {
        let ber = |v: f64| v; // an axis whose BER *is* its value, exactly monotone
        let floor = search_axis_limit(Criterion::FailureBer, 1.0, 1e-7, ber);
        let penalty = search_axis_limit(
            Criterion::MaxPenalty {
                penalty_db: 1.0,
                max_ber: 1e-3,
            },
            1.0,
            1e-7,
            ber,
        );
        assert!((floor - FAILURE_BER).abs() < 1e-6, "floor {floor}");
        assert!((penalty - 1e-3).abs() < 1e-6, "penalty {penalty}");
        assert!(penalty < floor);
    }

    /// The §4.3 axis runner end-to-end on the reference link. An open-loop matched filter
    /// integrates a whole block with no carrier recovery, so it tolerates only a vanishing
    /// CFO — the measured number validates the runner, not the link; a real entry's tracking
    /// loop is what earns a respectable row here.
    #[test]
    fn cfo_axis_search_is_finite_and_deterministic() {
        let link = ideal_bpsk();
        let sens = measure_sensitivity(
            &link,
            &ChannelSpec::default(),
            &[6.0, 7.0],
            0xcf0,
            200,
            1_000_000,
        );
        let mut table = LimitsTable::new("ideal-bpsk", 0xcf0, &sens);
        let op_db = table.operating_point_db().unwrap();
        // 24 576 bits (6 blocks) per probe: enough to separate BER 1e-2 from the clean-link
        // ~1e-5 unambiguously, cheap enough that a ~16-probe bisection stays fast.
        let ber_at = |cfo_cps: f64| {
            let spec = ChannelSpec::default().cfo(Cfo::from_cycles_per_sample(cfo_cps));
            measure_ber(&link, &spec, op_db, 0xcf1, 60, 24_576)
        };
        let row = measure_axis_row(
            "static CFO",
            "cycles/sample",
            Criterion::FailureBer,
            1e-3,
            1e-7,
            ber_at,
        );
        assert!(row.threshold.is_finite());
        assert!(
            row.threshold > 1e-6 && row.threshold < 1e-4,
            "threshold {} cycles/sample",
            row.threshold
        );
        assert_eq!(row.criterion, Criterion::FailureBer.label());
        // Bit-identical on a rerun: the search is a fixed sequence of seeded measurements.
        let again = search_axis_limit(Criterion::FailureBer, 1e-3, 1e-7, ber_at);
        assert_eq!(row.threshold.to_bits(), again.to_bits());
        table.rows.push(row);
        assert!(compare_tables(&table, &table, 0.05).is_ok());
    }

    #[test]
    fn profiles_carry_their_documented_axes() {
        assert_eq!(CompositeProfile::MobileUrban.name(), "mobile-urban");
        assert_eq!(CompositeProfile::StaticIndoor.name(), "static-indoor");
        let mu = CompositeProfile::MobileUrban.apply(ChannelSpec::default());
        assert!(mu.multipath.is_some());
        assert!(mu.cfo.is_some() && mu.drift.is_some() && mu.phase_noise.is_some());
        // Noise stays the sweep's axis: a profile carrying its own AWGN would break the
        // canonical noise-last Eb/N0 accounting.
        assert!(mu.awgn.is_none());
        let si = CompositeProfile::StaticIndoor.apply(ChannelSpec::default());
        assert!(si.multipath.is_some() && si.phase_noise.is_some());
        assert!(si.cfo.is_none() && si.drift.is_none() && si.awgn.is_none());
    }

    /// The pure comparator first: a curve against itself costs nothing, a 0.5 dB shift reads
    /// as exactly 0.5 dB, and an unreachable crossing is loud, not silent.
    #[test]
    fn degradation_reads_a_known_shift() {
        let clean = synth_bpsk_curve();
        let shifted = Curve {
            label: "shifted".to_string(),
            points: clean
                .points
                .iter()
                .map(|p| CurvePoint {
                    ebn0_db: p.ebn0_db + 0.5,
                    ..*p
                })
                .collect(),
        };
        assert!(degradation_db(&clean, &clean, 1e-3).abs() < 1e-9);
        let d = degradation_db(&shifted, &clean, 1e-3);
        assert!((d - 0.5).abs() < 1e-9, "shift read {d} dB");
        let empty = Curve {
            label: "empty".to_string(),
            points: vec![],
        };
        assert!(degradation_db(&empty, &clean, 1e-3).is_infinite());
    }

    /// Combined stress (§4.3) measured end-to-end on the mildest profile: the reference link
    /// has no equaliser and no carrier loop, yet one weak reflection plus 1° of phase noise
    /// must cost only a fraction of a dB — and the result must come out shaped as a row.
    #[test]
    fn static_indoor_degradation_is_measured_and_mild() {
        let link = ideal_bpsk();
        let row = measure_profile_degradation(
            &link,
            &ChannelSpec::default(),
            CompositeProfile::StaticIndoor,
            &[5.0, 6.0, 7.0, 8.0],
            0x51de,
            150,
            400_000,
        );
        assert_eq!(row.axis, "profile:static-indoor");
        assert_eq!(row.unit, "dB");
        assert_eq!(row.criterion, DEGRADATION_CRITERION);
        assert!(row.threshold.is_finite());
        assert!(
            row.threshold > -0.5 && row.threshold < 1.5,
            "degradation {} dB",
            row.threshold
        );
    }

    #[test]
    fn comparator_passes_identical_and_improved_tables() {
        let committed = sample_table();
        assert!(compare_tables(&committed, &committed, 0.1).is_ok());
        let mut better = sample_table();
        better.rows[0].threshold = 150.0e-6; // tolerates more CFO
        better.rows[1].threshold = 0.9; // the profile costs less
        better.sensitivity_db_1e3 = Some(6.5); // needs less Eb/N0
        // A newly measured axis is progress, not a regression.
        better.rows.push(LimitRow {
            axis: "frequency drift".to_string(),
            unit: "cycles/sample^2".to_string(),
            threshold: 1e-9,
            criterion: Criterion::FailureBer.label(),
        });
        assert!(compare_tables(&better, &committed, 0.1).is_ok());
        // Wobble inside the tolerance is not a regression either.
        let mut wobble = sample_table();
        wobble.rows[0].threshold = 115.0e-6;
        assert!(compare_tables(&wobble, &committed, 0.1).is_ok());
    }

    #[test]
    fn comparator_flags_doctored_and_missing_rows() {
        let committed = sample_table();
        let mut worse = sample_table();
        worse.rows[0].threshold = 80.0e-6; // 33% under committed, tolerance 10%
        worse.rows[1].threshold = 2.5; // degradation grew
        worse.sensitivity_db_1e3 = Some(7.9);
        let faults = compare_tables(&worse, &committed, 0.1).unwrap_err();
        assert_eq!(faults.len(), 3, "faults: {faults:?}");
        assert!(faults.iter().any(|f| f.contains("static CFO")));
        assert!(faults.iter().any(|f| f.contains("profile:mobile-urban")));
        assert!(faults.iter().any(|f| f.contains("sensitivity at BER 1e-3")));
        // A vanished row is a regression, not a smaller table.
        let mut missing = sample_table();
        missing.rows.remove(0);
        let faults = compare_tables(&missing, &committed, 0.1).unwrap_err();
        assert!(faults.iter().any(|f| f.contains("missing")));
        // A vanished sensitivity too.
        let mut lost = sample_table();
        lost.sensitivity_db_1e2 = None;
        assert!(compare_tables(&lost, &committed, 0.1).is_err());
        // A swapped criterion is incomparable: flagged, never compared.
        let mut swapped = sample_table();
        swapped.rows[0].criterion = "something else".to_string();
        assert!(compare_tables(&swapped, &committed, 0.1).is_err());
    }

    #[test]
    fn json_round_trips_pretty() {
        let dir = std::env::temp_dir().join(format!("sdrmm-modem-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("table.json");
        let table = sample_table();
        save_json(&table, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Pretty and newline-terminated: the committed-artifact format, same as the curves.
        assert!(text.contains("\n  ") && text.ends_with('\n'));
        assert_eq!(load_json(&path).unwrap(), table);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
