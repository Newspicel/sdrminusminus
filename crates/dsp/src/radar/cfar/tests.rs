use std::f64::consts::TAU;

use super::*;
use crate::fft::FftPair;
use crate::radar::batch::DopplerTaper;

const PFA: f64 = 1e-3;
const STATS: [CfarStatistic; 3] = [
    CfarStatistic::Ca,
    CfarStatistic::Os { rank: 0.75 },
    CfarStatistic::Go,
];

struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn exponential(&mut self) -> f32 {
        -(1.0 - self.uniform()).ln() as f32
    }

    fn complex(&mut self) -> Complex<f32> {
        let radius = (-(1.0 - self.uniform()).ln()).sqrt();
        let angle = TAU * self.uniform();
        Complex::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
    }
}

fn exponential_map(rows: usize, gates: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng(seed);
    (0..rows * gates).map(|_| rng.exponential()).collect()
}

fn shape(stat: CfarStatistic, plane: bool) -> CfarSpec {
    CfarSpec {
        stat,
        plane,
        guard_range: 2,
        train_range: 8,
        guard_doppler: 1,
        train_doppler: 4,
        alpha: 1.0,
        alpha_edge: 1.0,
        min_snr: 0.0,
        min_gate: 0,
        clutter_half_rows: 0,
    }
}

fn designed(stat: CfarStatistic, plane: bool, looks: u32, correlation: f64) -> CfarSpec {
    designed_from(shape(stat, plane), looks, correlation)
}

fn designed_from(spec: CfarSpec, looks: u32, correlation: f64) -> CfarSpec {
    let stat = spec.stat;
    let table =
        AlphaTable::new(stat, spec.statistic_cells(), spec.edge_cells(), PFA, looks).unwrap();
    let (alpha, alpha_edge) = table.pick(looks, correlation);
    CfarSpec {
        alpha,
        alpha_edge,
        ..spec
    }
}

fn measured_pfa(spec: CfarSpec, power: &[f32], rows: usize, gates: usize) -> f64 {
    let mut cfar = Cfar::new(spec, gates, rows).unwrap();
    let mut hits = Vec::new();
    let truncated = cfar.detect(power, 0..rows, &mut hits, 100_000).unwrap();
    assert_eq!(truncated, 0);
    let tested = (rows - (2 * spec.clutter_half_rows + 1)) * gates;
    hits.len() as f64 / tested as f64
}

fn within(ratio: f64, low: f64, high: f64) -> bool {
    (low..=high).contains(&ratio)
}

#[test]
fn measured_pfa_matches_design() {
    let (rows, gates) = (1000, 1000);
    let power = exponential_map(rows, gates, 9);
    for stat in STATS {
        for plane in [false, true] {
            let ratio = measured_pfa(designed(stat, plane, 1, 1.0), &power, rows, gates) / PFA;
            assert!(within(ratio, 0.5, 2.0), "{stat:?} plane {plane}: {ratio}");
        }
    }
}

fn narrow_plane(stat: CfarStatistic) -> CfarSpec {
    CfarSpec {
        guard_range: 3,
        train_range: 1,
        guard_doppler: 1,
        train_doppler: 2,
        ..shape(stat, true)
    }
}

#[test]
fn a_narrow_plane_window_keeps_the_designed_pfa() {
    let (rows, gates) = (1000, 1000);
    let power = exponential_map(rows, gates, 31);
    for stat in STATS {
        let ratio = measured_pfa(
            designed_from(narrow_plane(stat), 1, 1.0),
            &power,
            rows,
            gates,
        ) / PFA;
        assert!(within(ratio, 0.75, 1.33), "{stat:?}: {ratio}");
    }
}

#[test]
fn plane_edges_fall_back_at_the_designed_pfa() {
    let (rows, gates) = (20_000, 9);
    let power = exponential_map(rows, gates, 32);
    for stat in [CfarStatistic::Os { rank: 0.75 }, CfarStatistic::Go] {
        let spec = designed_from(narrow_plane(stat), 1, 1.0);
        let ratio = measured_pfa(spec, &power, rows, gates) / PFA;
        assert!(within(ratio, 0.5, 2.0), "{stat:?}: {ratio}");
    }
}

#[test]
fn go_on_a_plane_designs_for_its_two_side_rectangles() {
    let spec = narrow_plane(CfarStatistic::Go);
    assert_eq!(spec.cells(), 42);
    assert_eq!(spec.statistic_cells(), 14);
    assert_eq!(spec.edge_cells(), 42);
    let range = shape(CfarStatistic::Go, false);
    assert_eq!(range.statistic_cells(), 16);
    assert_eq!(range.edge_cells(), 16);
    let os = narrow_plane(CfarStatistic::Os { rank: 0.75 });
    assert_eq!(os.statistic_cells(), 42);
}

fn oversampled_rows(rows: usize, gates: usize, seed: u64) -> Vec<Vec<Complex<f32>>> {
    let mut rng = Rng(seed);
    (0..rows)
        .map(|_| {
            let white: Vec<Complex<f32>> = (0..=gates).map(|_| rng.complex()).collect();
            white
                .windows(2)
                .map(|pair| (pair[0] + pair[1]) * std::f32::consts::FRAC_1_SQRT_2)
                .collect()
        })
        .collect()
}

fn normalised_power(rows: &[Vec<Complex<f32>>]) -> Vec<f32> {
    let power: Vec<f32> = rows.iter().flatten().map(Complex::norm_sqr).collect();
    let mean = power.iter().map(|&p| f64::from(p)).sum::<f64>() / power.len() as f64;
    power
        .iter()
        .map(|&p| (f64::from(p) / mean) as f32)
        .collect()
}

fn mean_autocorrelation(rows: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
    (0..=CORRELATION_LAGS)
        .map(|lag| {
            rows.iter()
                .flat_map(|row| (0..row.len() - lag).map(move |n| row[n + lag] * row[n].conj()))
                .sum()
        })
        .collect()
}

#[test]
fn oversampled_noise_keeps_pfa_within_3x() {
    let (rows, gates) = (1000, 1024);
    let data = oversampled_rows(rows, gates, 21);
    let power = normalised_power(&data);
    let rho = range_correlation(&mean_autocorrelation(&data), 8);
    assert!((rho - 1.4375).abs() < 0.02, "{rho}");
    for stat in STATS {
        for plane in [false, true] {
            let spec = designed(stat, plane, 1, rho);
            let ratio = measured_pfa(spec, &power, rows, gates) / PFA;
            assert!(
                within(ratio, 1.0 / 3.0, 3.0),
                "{stat:?} plane {plane}: {ratio}"
            );
        }
    }
}

#[test]
fn correlated_lanes_keep_pfa_within_3x() {
    let (rows, gates, lanes) = (500, 1000, 4);
    let mut rng = Rng(33);
    let share = 0.5f32;
    let mut coherence = LaneCoherence::new(lanes).unwrap();
    let mut power = vec![0.0f32; rows * gates];
    let mut snapshot = [Complex::default(); 4];
    for cell in &mut power {
        let common = rng.complex();
        for value in &mut snapshot {
            *value = common * share.sqrt() + rng.complex() * (1.0 - share).sqrt();
        }
        coherence.add(&snapshot);
        *cell = snapshot.iter().map(Complex::norm_sqr).sum::<f32>() / lanes as f32;
    }
    let looks = coherence.looks();
    assert_eq!(looks, 2);
    for stat in STATS {
        for plane in [false, true] {
            let spec = designed(stat, plane, looks, 1.0);
            let ratio = measured_pfa(spec, &power, rows, gates) / PFA;
            assert!(
                within(ratio, 1.0 / 3.0, 3.0),
                "{stat:?} plane {plane}: {ratio}"
            );
        }
    }
}

fn fm_reference(len: usize, seed: u64) -> Vec<Complex<f32>> {
    let mut rng = Rng(seed);
    let mut phase = 0.0f64;
    let mut drive = 0.0f64;
    (0..len)
        .map(|_| {
            drive = 0.995 * drive + 0.1 * (rng.uniform() - 0.5);
            phase += drive;
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

#[test]
fn a_narrow_fm_reference_keeps_pfa_within_3x() {
    let (rows, gates, batch) = (1000, 256, 512);
    let reference = fm_reference(rows * batch + 1, 5);
    let mut rng = Rng(77);
    let mut fft = FftPair::new(1024);
    let mut spectrum = vec![Complex::default(); 1024];
    let mut profile = vec![Complex::default(); 1024];
    let mut data = Vec::with_capacity(rows);
    for row in 0..rows {
        spectrum.fill(Complex::default());
        spectrum[..batch].copy_from_slice(&reference[row * batch..(row + 1) * batch]);
        fft.forward(&mut spectrum);
        for value in &mut profile[..batch + gates] {
            *value = rng.complex();
        }
        profile[batch + gates..].fill(Complex::default());
        fft.forward(&mut profile);
        for (value, r) in profile.iter_mut().zip(&spectrum) {
            *value *= r.conj();
        }
        fft.inverse_scaled(&mut profile);
        data.push(profile[..gates].to_vec());
    }
    let power = normalised_power(&data);
    let lags: Vec<Complex<f32>> = (0..=CORRELATION_LAGS)
        .map(|lag| {
            (0..reference.len() - lag)
                .map(|n| reference[n + lag] * reference[n].conj())
                .sum()
        })
        .collect();
    let rho = range_correlation(&lags, 8);
    assert!(rho > 4.0, "{rho}");
    for stat in STATS {
        for plane in [false, true] {
            let spec = designed(stat, plane, 1, rho);
            let ratio = measured_pfa(spec, &power, rows, gates) / PFA;
            let floor = if plane { 1.0 / 3.0 } else { 0.0 };
            assert!(within(ratio, floor, 3.0), "{stat:?} plane {plane}: {ratio}");
        }
    }
}

fn detected(hits: &[Hit], row: u32, gate: u32) -> bool {
    hits.iter().any(|hit| hit.row == row && hit.gate == gate)
}

#[test]
fn edge_targets_are_detected() {
    let (rows, gates) = (64, 128);
    let mut power = exponential_map(rows, gates, 3);
    for gate in [0, 1, gates - 1] {
        power[10 * gates + gate] = 400.0;
    }
    for stat in STATS {
        for plane in [false, true] {
            let mut cfar = Cfar::new(designed(stat, plane, 1, 1.0), gates, rows).unwrap();
            let mut hits = Vec::new();
            cfar.detect(&power, 0..rows, &mut hits, 1000).unwrap();
            for gate in [0, 1, gates as u32 - 1] {
                assert!(
                    detected(&hits, 10, gate),
                    "{stat:?} plane {plane} gate {gate}"
                );
            }
        }
    }
}

#[test]
fn clutter_rows_are_never_reported() {
    let (rows, gates) = (64, 128);
    let mut power = exponential_map(rows, gates, 4);
    for row in 28..=36 {
        power[row * gates + 50] = 1e4;
    }
    for stat in STATS {
        for plane in [false, true] {
            let spec = CfarSpec {
                clutter_half_rows: 2,
                ..designed(stat, plane, 1, 1.0)
            };
            let mut cfar = Cfar::new(spec, gates, rows).unwrap();
            let mut hits = Vec::new();
            cfar.detect(&power, 20..35, &mut hits, 1000).unwrap();
            assert!(
                hits.iter()
                    .all(|hit| hit.row.abs_diff(32) > 2 && hit.row < 35)
            );
            for row in [28, 29] {
                assert!(detected(&hits, row, 50), "{stat:?} plane {plane} row {row}");
            }
        }
    }
}

#[test]
fn the_zero_doppler_ridge_does_not_mask_its_neighbours() {
    let (rows, gates) = (64, 128);
    let mut power = exponential_map(rows, gates, 5);
    for row in 31..=33 {
        power[row * gates..(row + 1) * gates].fill(1e5);
    }
    power[35 * gates + 60] = 200.0;
    for stat in STATS {
        let spec = CfarSpec {
            clutter_half_rows: 1,
            ..designed(stat, true, 1, 1.0)
        };
        let mut cfar = Cfar::new(spec, gates, rows).unwrap();
        let mut hits = Vec::new();
        cfar.detect(&power, 0..rows, &mut hits, 1000).unwrap();
        assert!(detected(&hits, 35, 60), "{stat:?}");
    }
}

fn brute_plane_mean(
    spec: &CfarSpec,
    power: &[f32],
    rows: usize,
    gates: usize,
    row: usize,
    gate: usize,
) -> f64 {
    let pad = (spec.guard_doppler + spec.train_doppler) as i64;
    let reach = spec.guard_range + spec.train_range;
    let width = (2 * reach + 1).min(gates);
    let start = gate.saturating_sub(reach).min(gates - width);
    let (mut sum, mut count) = (0.0f64, 0usize);
    for offset in -pad..=pad {
        let source = (row as i64 + offset).rem_euclid(rows as i64) as usize;
        for g in start..start + width {
            let guarded = offset.unsigned_abs() as usize <= spec.guard_doppler
                && g.abs_diff(gate) <= spec.guard_range;
            if guarded {
                continue;
            }
            let clutter = source.abs_diff(rows / 2) <= spec.clutter_half_rows;
            sum += if clutter {
                1.0
            } else {
                f64::from(power[source * gates + g])
            };
            count += 1;
        }
    }
    sum / count as f64
}

#[test]
fn integral_image_equals_brute_force() {
    let (rows, gates) = (40, 60);
    let power = exponential_map(rows, gates, 6);
    let spec = CfarSpec {
        clutter_half_rows: 2,
        ..designed(CfarStatistic::Ca, true, 1, 1.0)
    };
    let mut cfar = Cfar::new(spec, gates, rows).unwrap();
    cfar.build_table(&power);
    for row in [0, 1, 17, 22, 38, 39] {
        for gate in [0, 3, 10, 30, 57, 59] {
            let (fast, _) = cfar.plane_noise(&power, row, gate);
            let slow = brute_plane_mean(&spec, &power, rows, gates, row, gate);
            assert!(
                (f64::from(fast) - slow).abs() < 1e-4 * slow,
                "{row} {gate}: {fast} {slow}"
            );
        }
    }
}

#[test]
fn doppler_training_wraps() {
    let (rows, gates) = (32, 64);
    let mut power = vec![1.0f32; rows * gates];
    for row in 29..32 {
        power[row * gates..(row + 1) * gates].fill(50.0);
    }
    power[30] = 40.0;
    power[8 * gates + 30] = 40.0;
    let mut cfar = Cfar::new(designed(CfarStatistic::Ca, true, 1, 1.0), gates, rows).unwrap();
    let mut hits = Vec::new();
    cfar.detect(&power, 0..rows, &mut hits, 1000).unwrap();
    assert!(!detected(&hits, 0, 30));
    assert!(detected(&hits, 8, 30));
    let (noise, _) = cfar.plane_noise(&power, 0, 30);
    let expected = brute_plane_mean(&cfar.spec(), &power, rows, gates, 0, 30);
    assert!(noise > 5.0);
    assert!((f64::from(noise) - expected).abs() < 1e-4 * expected);
}

#[test]
fn too_many_hits_are_truncated_and_counted() {
    let (rows, gates) = (64, 128);
    let mut power = vec![1.0f32; rows * gates];
    for k in 0..100usize {
        let row = 4 + (k / 10) * 5;
        let gate = 5 + (k % 10) * 12;
        power[row * gates + gate] = 100.0 + k as f32;
    }
    let mut cfar = Cfar::new(designed(CfarStatistic::Ca, false, 1, 1.0), gates, rows).unwrap();
    let mut hits = Vec::new();
    let truncated = cfar.detect(&power, 0..rows, &mut hits, 40).unwrap();
    assert_eq!(truncated, 60);
    assert_eq!(hits.len(), 40);
    assert!(hits.iter().all(|hit| hit.power >= 160.0));
    assert!(
        hits.windows(2)
            .all(|pair| (pair[0].row, pair[0].gate) < (pair[1].row, pair[1].gate))
    );
}

#[test]
fn min_gate_and_min_snr_hold_back_weak_or_near_cells() {
    let (rows, gates) = (16, 128);
    let mut power = vec![1.0f32; rows * gates];
    power[3 * gates + 2] = 1e3;
    power[3 * gates + 40] = 6.0;
    power[3 * gates + 80] = 1e3;
    let spec = CfarSpec {
        min_gate: 10,
        min_snr: 8.0,
        alpha: 2.0,
        ..shape(CfarStatistic::Ca, false)
    };
    let mut cfar = Cfar::new(spec, gates, rows).unwrap();
    let mut hits = Vec::new();
    cfar.detect(&power, 0..rows, &mut hits, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].row, hits[0].gate), (3, 80));
    assert!((hits[0].noise - 1.0).abs() < 1e-6);
}

#[test]
fn invalid_specs_are_refused() {
    let spec = shape(CfarStatistic::Ca, true);
    assert!(
        Cfar::new(
            CfarSpec {
                train_range: 0,
                ..spec
            },
            10,
            10
        )
        .is_err()
    );
    assert!(
        Cfar::new(
            CfarSpec {
                train_doppler: 0,
                ..spec
            },
            10,
            10
        )
        .is_err()
    );
    assert!(
        Cfar::new(
            CfarSpec {
                alpha: f32::NAN,
                ..spec
            },
            10,
            10
        )
        .is_err()
    );
    assert!(Cfar::new(spec, 0, 10).is_err());
    let mut cfar = Cfar::new(spec, 10, 10).unwrap();
    assert!(
        cfar.set_spec(CfarSpec {
            guard_range: 17,
            ..spec
        })
        .is_err()
    );
    let mut hits = Vec::new();
    assert_eq!(
        cfar.detect(&[1.0; 99], 0..10, &mut hits, 10),
        Err(RadarDspError::Shape)
    );
}

#[test]
fn the_alpha_table_never_picks_below_the_measured_correlation() {
    let table = AlphaTable::new(CfarStatistic::Ca, 16, 16, PFA, 2).unwrap();
    let column = |rho: f64| alpha(CfarStatistic::Ca, 16, PFA, 1, rho).unwrap() as f32;
    assert_eq!(table.pick(1, 1.0).0, column(1.0));
    assert_eq!(table.pick(1, 1.34).0, column(1.6));
    assert_eq!(table.pick(1, 1.6).0, column(1.6));
    assert_eq!(
        table.pick(1, 50.0).0,
        column(RHO_TABLE[RHO_TABLE.len() - 1])
    );
    assert_eq!(table.pick(0, 1.0), table.pick(1, 1.0));
    assert_eq!(table.pick(9, 1.0), table.pick(2, 1.0));
    assert!(table.pick(2, 1.0).0 < table.pick(1, 1.0).0);
}

#[test]
fn the_alpha_table_covers_the_largest_plane_correlation() {
    let widest_range = 1.0 + 2.0 * CORRELATION_LAGS as f64;
    let widest_plane = widest_range * DopplerTaper::BlackmanHarris.enbw();
    assert!(RHO_TABLE[RHO_TABLE.len() - 1] >= widest_plane);
    let table = AlphaTable::new(CfarStatistic::Ca, 216, 216, PFA, 1).unwrap();
    let needed = alpha(CfarStatistic::Ca, 216, PFA, 1, widest_plane).unwrap() as f32;
    let below = alpha(CfarStatistic::Ca, 216, PFA, 1, 7.0).unwrap() as f32;
    assert_eq!(table.pick(1, widest_plane).0, needed);
    assert_eq!(table.pick(1, 13.0).0, needed);
    assert!(below < needed);
    let go = AlphaTable::new(CfarStatistic::Go, 16, 16, PFA, 1).unwrap();
    let one_cell_a_side = alpha(CfarStatistic::Go, 2, PFA, 1, 1.0).unwrap() as f32;
    assert_eq!(go.pick(1, widest_plane).0, one_cell_a_side);
    assert!(AlphaTable::new(CfarStatistic::Go, 1, 1, PFA, 1).is_err());
}

#[test]
fn an_unreachable_correlated_column_holds_the_strictest_alpha() {
    let strictest = crate::radar::threshold::MAX_ALPHA as f32;
    let table = AlphaTable::new(CfarStatistic::Ca, 2, 2, 1e-9, 1).unwrap();
    assert!(table.pick(1, 1.0).0 < strictest);
    assert_eq!(table.pick(1, 1.33), (strictest, strictest));
    assert!(AlphaTable::new(CfarStatistic::Ca, 1, 1, 1e-9, 1).is_err());
}

#[test]
fn lane_coherence_counts_independent_lanes() {
    let mut rng = Rng(8);
    let mut independent = LaneCoherence::new(4).unwrap();
    let mut identical = LaneCoherence::new(4).unwrap();
    for _ in 0..20_000 {
        let lanes = [rng.complex(), rng.complex(), rng.complex(), rng.complex()];
        independent.add(&lanes);
        identical.add(&[lanes[0]; 4]);
    }
    assert_eq!(independent.looks(), 4);
    assert_eq!(identical.looks(), 1);
    let lags = [Complex::new(2.0, 0.0), Complex::new(1.0, 0.0)];
    assert!((range_correlation(&lags, 1_000_000) - 1.5).abs() < 1e-6);
    assert!((range_correlation(&lags, 2) - 1.25).abs() < 1e-9);
}
