use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::{
    Curve, FAILURE_BER, SENSITIVITY_MARGIN_DB,
    impair::{Cfo, ChannelSpec, Drift, Multipath, MultipathProfile, PhaseNoise},
    sweep::{Link, sweep_ber},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitRow {
    pub axis: String,
    pub unit: String,
    pub threshold: f64,
    pub criterion: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitsTable {
    pub entry: String,
    pub seed: u64,
    pub sensitivity_db_1e2: Option<f64>,
    pub sensitivity_db_1e3: Option<f64>,
    pub sensitivity_db_1e4: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sinad_sensitivity_db: Option<f64>,
    pub rows: Vec<LimitRow>,
}

impl LimitsTable {
    #[must_use]
    pub fn new(entry: impl Into<String>, seed: u64, sensitivity: &Sensitivity) -> Self {
        Self {
            entry: entry.into(),
            seed,
            sensitivity_db_1e2: sensitivity.db_at_1e2,
            sensitivity_db_1e3: sensitivity.db_at_1e3,
            sensitivity_db_1e4: sensitivity.db_at_1e4,
            sinad_sensitivity_db: None,
            rows: Vec::new(),
        }
    }

    #[must_use]
    pub fn analog(entry: impl Into<String>, seed: u64, sinad_sensitivity_db: Option<f64>) -> Self {
        Self {
            entry: entry.into(),
            seed,
            sensitivity_db_1e2: None,
            sensitivity_db_1e3: None,
            sensitivity_db_1e4: None,
            sinad_sensitivity_db,
            rows: Vec::new(),
        }
    }

    #[must_use]
    pub fn operating_point_db(&self) -> Option<f64> {
        self.sensitivity_db_1e3
            .or(self.sinad_sensitivity_db)
            .map(|db| db + SENSITIVITY_MARGIN_DB)
    }
}

pub fn save_json(table: &LimitsTable, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(table).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<LimitsTable> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

#[derive(Clone, Debug)]
pub struct Sensitivity {
    pub curve: Curve,
    pub db_at_1e2: Option<f64>,
    pub db_at_1e3: Option<f64>,
    pub db_at_1e4: Option<f64>,
}

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

#[must_use]
pub fn ber_at_ebn0(curve: &Curve, ebn0_db: f64) -> Option<f64> {
    let pts = usable_log_points(curve);
    for pair in pts.windows(2) {
        let (db_a, la) = pair[0];
        let (db_b, lb) = pair[1];
        if (ebn0_db - db_a) * (ebn0_db - db_b) <= 0.0 {
            if (db_b - db_a).abs() < 1e-12 {
                return Some(10f64.powf(la));
            }
            let t = (ebn0_db - db_a) / (db_b - db_a);
            return Some(10f64.powf(la + t * (lb - la)));
        }
    }
    None
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Criterion {
    FailureBer,
    MaxPenalty { penalty_db: f64, max_ber: f64 },
    MinSinad { min_sinad_db: f64 },
    SinadPenalty { penalty_db: f64, min_sinad_db: f64 },
}

pub const ANALOG_SINAD_DB: f64 = 12.0;

impl Criterion {
    #[must_use]
    pub fn limit(self) -> f64 {
        match self {
            Self::FailureBer => FAILURE_BER,
            Self::MaxPenalty { max_ber, .. } => max_ber,
            Self::MinSinad { min_sinad_db } | Self::SinadPenalty { min_sinad_db, .. } => {
                -min_sinad_db
            }
        }
    }

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
            Self::MinSinad { min_sinad_db } => {
                format!(
                    "SINAD >= {min_sinad_db} dB at sensitivity({ANALOG_SINAD_DB} dB SINAD) \
                     + {SENSITIVITY_MARGIN_DB} dB"
                )
            }
            Self::SinadPenalty { penalty_db, .. } => {
                format!(
                    "<= {penalty_db} dB SINAD penalty at sensitivity({ANALOG_SINAD_DB} dB SINAD) \
                     + {SENSITIVITY_MARGIN_DB} dB"
                )
            }
        }
    }
}

#[must_use]
pub fn penalty_criterion(clean: &Curve, op_ebn0_db: f64, penalty_db: f64) -> Option<Criterion> {
    let max_ber = ber_at_ebn0(clean, op_ebn0_db - penalty_db)?;
    Some(Criterion::MaxPenalty {
        penalty_db,
        max_ber,
    })
}

#[must_use]
pub fn sinad_penalty_criterion(clean_sinad_db: f64, penalty_db: f64) -> Criterion {
    Criterion::SinadPenalty {
        penalty_db,
        min_sinad_db: clean_sinad_db - penalty_db,
    }
}

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

const MAX_SEARCH_ITERS: u32 = 64;

pub fn search_axis_limit(
    criterion: Criterion,
    max_axis: f64,
    tolerance: f64,
    metric_at: impl Fn(f64) -> f64,
) -> f64 {
    let limit = criterion.limit();
    let passes = |value: f64| metric_at(value) <= limit;
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

pub fn measure_axis_row(
    axis: impl Into<String>,
    unit: impl Into<String>,
    criterion: Criterion,
    max_axis: f64,
    tolerance: f64,
    metric_at: impl Fn(f64) -> f64,
) -> LimitRow {
    LimitRow {
        axis: axis.into(),
        unit: unit.into(),
        threshold: search_axis_limit(criterion, max_axis, tolerance, metric_at),
        criterion: criterion.label(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositeProfile {
    MobileUrban,
    StaticIndoor,
}

impl CompositeProfile {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::MobileUrban => "mobile-urban",
            Self::StaticIndoor => "static-indoor",
        }
    }

    #[must_use]
    pub fn apply(self, base: ChannelSpec) -> ChannelSpec {
        match self {
            Self::MobileUrban => base
                .multipath(Multipath::new(MultipathProfile::ExponentialPdp {
                    rms_delay_spread_samples: 2.0,
                    taps: 8,
                }))
                .cfo(Cfo::from_cycles_per_sample(1e-4))
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

pub const DEGRADATION_CRITERION: &str = "Eb/N0 degradation at BER 1e-3";

#[must_use]
pub fn degradation_db(impaired: &Curve, clean: &Curve, at_ber: f64) -> f64 {
    match (ebn0_at_ber(impaired, at_ber), ebn0_at_ber(clean, at_ber)) {
        (Some(i), Some(c)) => i - c,
        _ => f64::INFINITY,
    }
}

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
        (
            "sensitivity at SINAD 12 dB",
            measured.sinad_sensitivity_db,
            committed.sinad_sensitivity_db,
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
            sinad_sensitivity_db: None,
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
        assert!(sens.db_at_1e4.is_none());
        let table = LimitsTable::new("ideal-bpsk", 0x11317, &sens);
        assert_eq!(table.sensitivity_db_1e3, sens.db_at_1e3);
        let op = table.operating_point_db().unwrap();
        assert!((op - s3 - SENSITIVITY_MARGIN_DB).abs() < 1e-12);
    }

    #[test]
    fn crossings_interpolate_in_log_ber() {
        let curve = synth_bpsk_curve();
        let db3 = ebn0_at_ber(&curve, 1e-3).unwrap();
        assert!((db3 - 6.7895).abs() < 0.03, "1e-3 at {db3} dB");
        let back = ber_at_ebn0(&curve, db3).unwrap();
        assert!((back.log10() + 3.0).abs() < 1e-9, "round trip {back:e}");
        assert!(ebn0_at_ber(&curve, 1e-12).is_none());
        assert!(ebn0_at_ber(&curve, 0.9).is_none());
        assert!(ber_at_ebn0(&curve, -1.0).is_none());
        assert!(ber_at_ebn0(&curve, 12.5).is_none());
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
        let db = ebn0_at_ber(&curve, 1e-4).unwrap();
        assert!(db > 8.0 && db < 10.0, "crossing {db} dB");
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
        assert!((Criterion::FailureBer.limit() - FAILURE_BER).abs() < 1e-18);
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
        let want = theory::bpsk_ber(8.79);
        assert!(
            (max_ber.log10() - want.log10()).abs() < 0.05,
            "max_ber {max_ber:e}, want {want:e}"
        );
        assert_eq!(
            crit.label(),
            "<= 1 dB Eb/N0 penalty at sensitivity(1e-3) + 3 dB"
        );
        assert!(penalty_criterion(&synth_bpsk_curve(), 0.5, 1.0).is_none());
        assert!(penalty_criterion(&synth_bpsk_curve(), 40.0, 1.0).is_none());
    }

    #[test]
    fn search_handles_degenerate_predicates() {
        let unbounded = search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| 0.0);
        assert!((unbounded - 8.0).abs() < 1e-15);
        assert!(search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| 1.0) == 0.0);
        assert!(search_axis_limit(Criterion::FailureBer, 8.0, 1e-3, |_| f64::NAN) == 0.0);
        let step = |v: f64| if v <= 0.37 { 0.0 } else { 1.0 };
        let found = search_axis_limit(Criterion::FailureBer, 1.0, 1e-6, step);
        assert!(found <= 0.37 && 0.37 - found < 1e-6, "found {found}");
        let exact = search_axis_limit(Criterion::FailureBer, 1.0, 0.0, step);
        assert!((exact - 0.37).abs() < 1e-15, "exact {exact}");
    }

    #[test]
    fn stricter_criterion_gives_smaller_threshold() {
        let ber = |v: f64| v;
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
        assert!(mu.awgn.is_none());
        let si = CompositeProfile::StaticIndoor.apply(ChannelSpec::default());
        assert!(si.multipath.is_some() && si.phase_noise.is_some());
        assert!(si.cfo.is_none() && si.drift.is_none() && si.awgn.is_none());
    }

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
        better.rows[0].threshold = 150.0e-6;
        better.rows[1].threshold = 0.9;
        better.sensitivity_db_1e3 = Some(6.5);
        better.rows.push(LimitRow {
            axis: "frequency drift".to_string(),
            unit: "cycles/sample^2".to_string(),
            threshold: 1e-9,
            criterion: Criterion::FailureBer.label(),
        });
        assert!(compare_tables(&better, &committed, 0.1).is_ok());
        let mut wobble = sample_table();
        wobble.rows[0].threshold = 115.0e-6;
        assert!(compare_tables(&wobble, &committed, 0.1).is_ok());
    }

    #[test]
    fn comparator_flags_doctored_and_missing_rows() {
        let committed = sample_table();
        let mut worse = sample_table();
        worse.rows[0].threshold = 80.0e-6;
        worse.rows[1].threshold = 2.5;
        worse.sensitivity_db_1e3 = Some(7.9);
        let faults = compare_tables(&worse, &committed, 0.1).unwrap_err();
        assert_eq!(faults.len(), 3, "faults: {faults:?}");
        assert!(faults.iter().any(|f| f.contains("static CFO")));
        assert!(faults.iter().any(|f| f.contains("profile:mobile-urban")));
        assert!(faults.iter().any(|f| f.contains("sensitivity at BER 1e-3")));
        let mut missing = sample_table();
        missing.rows.remove(0);
        let faults = compare_tables(&missing, &committed, 0.1).unwrap_err();
        assert!(faults.iter().any(|f| f.contains("missing")));
        let mut lost = sample_table();
        lost.sensitivity_db_1e2 = None;
        assert!(compare_tables(&lost, &committed, 0.1).is_err());
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
        assert!(text.contains("\n  ") && text.ends_with('\n'));
        assert_eq!(load_json(&path).unwrap(), table);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
