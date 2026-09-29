use super::*;
use crate::radar::batch::DopplerTaper;
use crate::radar::batch::tests::{Caf, Rng, correlate, run_caf};
use crate::radar::nlms::tests::fm_reference;

type Path = (usize, C32);

fn channel(
    reference: &[C32],
    paths: &[Path],
    drift_hz: f64,
    fs: f64,
    noise: f32,
    seed: u64,
) -> Vec<C32> {
    let mut rng = Rng(seed);
    (0..reference.len())
        .map(|n| {
            let direct: C32 = paths
                .iter()
                .filter(|(delay, _)| n >= *delay)
                .map(|(delay, gain)| reference[n - delay] * gain)
                .sum();
            let turn = C32::from_polar(1.0, (TAU * drift_hz * n as f64 / fs) as f32);
            direct * turn + rng.complex() * noise
        })
        .collect()
}

fn direct_paths() -> Vec<Path> {
    vec![
        (0, C32::new(3.0, 1.0)),
        (1, C32::new(-0.8, 0.5)),
        (4, C32::new(0.3, -0.6)),
        (9, C32::new(0.1, 0.2)),
    ]
}

fn gate_energy(caf: &Caf, shape: &BatchShape, lane: usize, gates: Range<usize>) -> f64 {
    (0..shape.batches)
        .flat_map(|b| gates.clone().map(move |tau| (b, tau)))
        .map(|(b, tau)| f64::from(caf.gate(shape, lane, b, tau).norm_sqr()))
        .sum()
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

fn phase(plan: &GroupPlan, group: usize, tap: usize, batch: usize) -> C64 {
    rotation((tap as f64 - plan.doppler_taps as f64) * plan.cycles(group, batch))
}

fn direct_residual(
    shape: &BatchShape,
    plan: &GroupPlan,
    table: &WeightTable,
    reference: &[C32],
    surveillance: &[C32],
    batch: usize,
    tau: usize,
) -> C64 {
    let widen = |value: C32| C64::new(f64::from(value.re), f64::from(value.im));
    let mut value = widen(correlate(shape, reference, surveillance, batch, tau as i64));
    let (first, second, share) = plan.blend(batch);
    let share = f64::from(share);
    for (group, weight) in [(first, 1.0 - share), (second, share)] {
        if weight == 0.0 {
            continue;
        }
        let w = table.weights(group, 0);
        for tap in 0..plan.taps() {
            for index in 0..shape.order() {
                let delay = index as i64 - shape.lead as i64;
                let auto = widen(correlate(
                    shape,
                    reference,
                    reference,
                    batch,
                    tau as i64 - delay,
                ));
                value -=
                    weight * phase(plan, group, tap, batch) * w[tap * shape.order() + index] * auto;
            }
        }
    }
    value
}

#[test]
fn batch_domain_eca_equals_time_domain_eca() {
    let shape = BatchShape::new(32, 128, 24, 2, 6, 1).unwrap();
    let fs = 100e3;
    let reference = fm_reference(shape.window(), 21);
    let mut paths = direct_paths();
    paths.push((14, C32::new(0.05, 0.0)));
    let surveillance = channel(&reference, &paths, 0.0, fs, 0.01, 4);
    let plans = [
        GroupPlan::split(32, 8, 0, 0, false, false).unwrap(),
        GroupPlan::split(32, 8, 0, 1, true, false).unwrap(),
        GroupPlan::split(32, 5, 3, 0, false, true).unwrap(),
    ];
    for plan in &plans {
        let caf = run_caf(
            shape,
            Some(plan),
            &[reference.clone(), surveillance.clone()],
            DopplerTaper::Hann,
        );
        let table = caf.table.as_ref().unwrap();
        let mut worst = 0.0f64;
        let mut peak = 0.0f64;
        for batch in 0..shape.batches {
            for tau in 0..shape.gates {
                let direct =
                    direct_residual(&shape, plan, table, &reference, &surveillance, batch, tau);
                let fast = caf.gate(&shape, 0, batch, tau);
                let fast = C64::new(f64::from(fast.re), f64::from(fast.im));
                worst = worst.max((fast - direct).norm());
                peak = peak.max(f64::from(
                    correlate(&shape, &reference, &surveillance, batch, tau as i64).norm(),
                ));
            }
        }
        assert!(worst < 1e-3 * peak, "{plan:?}: {worst} vs {peak}");
    }
}

fn fm_scene(shape: &BatchShape, lanes: &[Vec<Path>], drift_hz: f64, fs: f64) -> Vec<Vec<C32>> {
    let reference = fm_reference(shape.window(), 5);
    let mut out = vec![reference.clone()];
    for (lane, paths) in lanes.iter().enumerate() {
        out.push(channel(
            &reference,
            paths,
            drift_hz,
            fs,
            1e-3,
            40 + lane as u64,
        ));
    }
    out
}

#[test]
fn a_direct_path_is_suppressed_by_40_db() {
    let fs = 266_666.67;
    let shape = BatchShape::new(64, 260, 73, 2, 14, 1).unwrap();
    let lanes = fm_scene(&shape, &[direct_paths()], 0.0, fs);
    let plan = GroupPlan::split(64, 16, 0, 0, true, false).unwrap();
    let cleaned = run_caf(shape, Some(&plan), &lanes, DopplerTaper::Hann);
    let raw = run_caf(shape, None, &lanes, DopplerTaper::Hann);
    let stats = cleaned.stats.unwrap();
    assert!(
        stats.suppression_db[0] >= 40.0,
        "{}",
        stats.suppression_db[0]
    );
    assert_eq!(stats.unsuppressed_groups, 0);
    let drop = db(gate_energy(&raw, &shape, 0, 0..15) / gate_energy(&cleaned, &shape, 0, 0..15));
    assert!(drop >= 40.0, "{drop}");
}

#[test]
fn shared_gram_serves_every_lane() {
    let fs = 266_666.67;
    let shape = BatchShape::new(32, 260, 40, 2, 10, 3).unwrap();
    let channels = vec![
        direct_paths(),
        vec![(0, C32::new(-1.0, 2.0)), (2, C32::new(0.4, 0.1))],
        vec![(1, C32::new(0.5, 0.5)), (7, C32::new(-0.2, 0.3))],
    ];
    let lanes = fm_scene(&shape, &channels, 0.0, fs);
    let plan = GroupPlan::split(32, 8, 0, 0, false, false).unwrap();
    let joint = run_caf(shape, Some(&plan), &lanes, DopplerTaper::Hann);
    let stats = joint.stats.unwrap();
    let table = joint.table.as_ref().unwrap();
    for lane in 0..3 {
        assert!(
            stats.suppression_db[lane] >= 40.0,
            "{lane}: {}",
            stats.suppression_db[lane]
        );
        let single = BatchShape { lanes: 1, ..shape };
        let alone = run_caf(
            single,
            Some(&plan),
            &[lanes[0].clone(), lanes[lane + 1].clone()],
            DopplerTaper::Hann,
        );
        let own = alone.table.as_ref().unwrap();
        for group in 0..plan.groups() {
            for (a, b) in table.weights(group, lane).iter().zip(own.weights(group, 0)) {
                assert!((a - b).norm() <= 1e-9 * (1.0 + b.norm()), "{lane} {group}");
            }
        }
    }
}

fn drifting(shape: &BatchShape, fs: f64) -> Vec<Vec<C32>> {
    let reference = fm_reference(shape.window(), 9);
    let direct = channel(&reference, &[(0, C32::new(3.0, 0.0))], 0.5, fs, 0.0, 1);
    let still = channel(&reference, &[(2, C32::new(0.5, 0.2))], 0.0, fs, 1e-4, 2);
    let surveillance = direct.iter().zip(&still).map(|(a, b)| a + b).collect();
    vec![reference, surveillance]
}

#[test]
fn eca_s_follows_a_drifting_direct_path() {
    let fs = 64_000.0;
    let shape = BatchShape::new(128, 250, 20, 2, 6, 1).unwrap();
    let lanes = drifting(&shape, fs);
    let batch = GroupPlan::split(128, 13, 0, 0, false, false).unwrap();
    let sliding = GroupPlan::split(128, 3, 6, 0, false, true).unwrap();
    let batched = run_caf(shape, Some(&batch), &lanes, DopplerTaper::Hann);
    let slid = run_caf(shape, Some(&sliding), &lanes, DopplerTaper::Hann);
    let gain = db(gate_energy(&batched, &shape, 0, 0..20) / gate_energy(&slid, &shape, 0, 0..20));
    assert!(gain >= 6.0, "{gain}");
}

fn off_zero_doppler(caf: &Caf, shape: &BatchShape, gates: Range<usize>) -> f64 {
    let centre = shape.batches / 2;
    gates
        .flat_map(|tau| (0..shape.batches).map(move |row| (tau, row)))
        .filter(|(_, row)| row.abs_diff(centre) > 2)
        .map(|(tau, row)| f64::from(caf.cell(shape, 0, tau, row).norm_sqr()))
        .sum()
}

#[test]
fn taper_removes_group_edge_residue() {
    let fs = 64_000.0;
    let shape = BatchShape::new(128, 250, 20, 2, 6, 1).unwrap();
    let lanes = drifting(&shape, fs);
    let stepped = GroupPlan::split(128, 13, 0, 0, false, false).unwrap();
    let smooth = GroupPlan {
        taper: true,
        ..stepped.clone()
    };
    let hard = run_caf(shape, Some(&stepped), &lanes, DopplerTaper::Hann);
    let soft = run_caf(shape, Some(&smooth), &lanes, DopplerTaper::Hann);
    let gain = db(off_zero_doppler(&hard, &shape, 0..4) / off_zero_doppler(&soft, &shape, 0..4));
    assert!(gain >= 6.0, "{gain}");
}

#[test]
fn doppler_taps_cancel_swaying_clutter() {
    let fs = 64_000.0;
    let shape = BatchShape::new(128, 250, 20, 2, 6, 1).unwrap();
    let plan = GroupPlan::split(128, 32, 0, 1, false, false).unwrap();
    let group_s = 32.0 * 250.0 / fs;
    let sway = 1.0 / group_s;
    let reference = fm_reference(shape.window(), 13);
    let direct = channel(&reference, &[(0, C32::new(3.0, 0.0))], 0.0, fs, 1e-4, 3);
    let ahead = channel(&reference, &[(3, C32::new(0.5, 0.0))], sway, fs, 0.0, 4);
    let behind = channel(&reference, &[(3, C32::new(0.0, 0.5))], -sway, fs, 0.0, 5);
    let surveillance: Vec<C32> = (0..reference.len())
        .map(|n| direct[n] + ahead[n] + behind[n])
        .collect();
    let lanes = [reference, surveillance];
    let raw = run_caf(shape, None, &lanes, DopplerTaper::Hann);
    let cleaned = run_caf(shape, Some(&plan), &lanes, DopplerTaper::Hann);
    let rows = (sway * shape.samples() as f64 / fs).round() as usize;
    for row in [64 - rows, 64 + rows] {
        let before = f64::from(raw.cell(&shape, 0, 3, row).norm_sqr());
        let after = f64::from(cleaned.cell(&shape, 0, 3, row).norm_sqr());
        assert!(db(before / after) >= 20.0, "{row}: {}", db(before / after));
    }
}

#[test]
fn a_singular_gram_is_loaded_then_counted() {
    let shape = BatchShape::new(16, 64, 10, 1, 4, 1).unwrap();
    let mut rng = Rng(2);
    let lanes = [
        vec![C32::default(); shape.window()],
        rng.noise(shape.window()),
    ];
    let plan = GroupPlan::split(16, 4, 0, 0, true, false).unwrap();
    let cleaned = run_caf(shape, Some(&plan), &lanes, DopplerTaper::Hann);
    let raw = run_caf(shape, None, &lanes, DopplerTaper::Hann);
    let stats = cleaned.stats.unwrap();
    assert_eq!(stats.unsuppressed_groups, plan.groups() as u32);
    assert_eq!(stats.suppression_db[0], 0.0);
    assert_eq!(cleaned.gates, raw.gates);
}

#[test]
fn an_order_over_512_is_refused() {
    let wide = BatchShape::new(16, 64, 10, 16, 200, 1).unwrap();
    let still = GroupPlan::split(16, 4, 0, 0, false, false).unwrap();
    assert!(WienerSolver::new(wide, &still, 1e-4).is_ok());
    let swaying = GroupPlan::split(16, 4, 0, 1, false, false).unwrap();
    assert_eq!(
        WienerSolver::new(wide, &swaying, 1e-4).err(),
        Some(RadarDspError::Order)
    );
    let plain = BatchShape::new(16, 64, 10, 0, 0, 1).unwrap();
    assert_eq!(
        WienerSolver::new(plain, &still, 1e-4).err(),
        Some(RadarDspError::Shape)
    );
    let other = GroupPlan::split(8, 4, 0, 0, false, false).unwrap();
    assert_eq!(
        WienerSolver::new(wide, &other, 1e-4).err(),
        Some(RadarDspError::Shape)
    );
    assert_eq!(
        WienerSolver::new(wide, &still, f32::NAN).err(),
        Some(RadarDspError::Setting)
    );
    assert!(GroupPlan::split(16, 4, 0, 3, false, false).is_err());
    assert_eq!(
        WeightTable::new(&wide, &other).err(),
        Some(RadarDspError::Shape)
    );
    assert_eq!(
        WeightTable::new(&plain, &still).err(),
        Some(RadarDspError::Shape)
    );
}

#[test]
fn mismatched_sums_and_tables_are_refused() {
    let shape = BatchShape::new(16, 64, 10, 1, 4, 2).unwrap();
    let plan = GroupPlan::split(16, 4, 0, 0, false, false).unwrap();
    let mut solver = WienerSolver::new(shape, &plan, 1e-4).unwrap();
    let single = BatchShape { lanes: 1, ..shape };
    let mut table = WeightTable::new(&shape, &plan).unwrap();
    let mut narrow = WeightTable::new(&single, &plan).unwrap();
    let sums = GroupSums::new(&shape, &plan);
    let few = GroupSums::new(&single, &plan);
    assert!(solver.solve(&sums, &mut table).is_ok());
    assert_eq!(
        solver.solve(&sums, &mut narrow).err(),
        Some(RadarDspError::Shape)
    );
    assert_eq!(
        solver.solve(&few, &mut table).err(),
        Some(RadarDspError::Shape)
    );
    let mut fresh = GroupSums::new(&shape, &plan);
    let m = shape.fft_len;
    let short = vec![C32::default(); m - 1];
    let full = vec![C32::default(); 2 * m];
    assert_eq!(
        solver.accumulate(&mut fresh, 0, &short, &full, &[0.0; 2]),
        Err(RadarDspError::Shape)
    );
    assert_eq!(
        solver.accumulate(&mut fresh, 16, &full, &full, &[0.0; 2]),
        Err(RadarDspError::Shape)
    );
}

#[test]
fn group_plans_follow_the_worked_example() {
    for (batches, per_group, small, large) in [(512, 51, 51, 52), (1024, 102, 102, 103)] {
        let plan = GroupPlan::split(batches, per_group, 0, 0, true, false).unwrap();
        assert_eq!(plan.groups(), 10);
        let sizes: Vec<usize> = plan
            .bounds
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect();
        assert!(
            sizes.iter().all(|&size| size == small || size == large),
            "{sizes:?}"
        );
        assert_eq!(plan.bounds[10], batches);
    }
}

#[test]
fn taper_blends_between_group_centres() {
    let plan = GroupPlan::split(40, 10, 0, 0, true, false).unwrap();
    assert_eq!(plan.blend(0), (0, 0, 0.0));
    assert_eq!(plan.blend(39), (3, 3, 0.0));
    let (first, second, share) = plan.blend(12);
    assert_eq!((first, second), (0, 1));
    assert!((share - 0.75).abs() < 1e-6);
    let sliding = GroupPlan::split(40, 10, 5, 0, true, true).unwrap();
    assert_eq!(sliding.blend(12), (1, 1, 0.0));
    assert_eq!(sliding.estimation(1), 5..25);
    assert_eq!(sliding.estimation(0), 0..15);
}

struct Spectra {
    models: Vec<C32>,
    products: Vec<C32>,
    energy: Vec<f64>,
}

fn spectra_of(shape: &BatchShape, lanes: &[Vec<C32>]) -> Spectra {
    let (m, nb, k) = (shape.fft_len, shape.batches, shape.lanes);
    let mut kernel = crate::radar::batch::BatchKernel::new(*shape);
    let mut spectrum = vec![C32::default(); m];
    let mut out = Spectra {
        models: vec![C32::default(); nb * m],
        products: vec![C32::default(); nb * k * m],
        energy: vec![0.0; nb * k],
    };
    for b in 0..nb {
        kernel
            .reference(
                &lanes[0],
                b,
                &mut spectrum,
                &mut out.models[b * m..(b + 1) * m],
            )
            .unwrap();
        for lane in 0..k {
            let at = (b * k + lane) * m;
            kernel
                .surveillance(
                    &lanes[lane + 1],
                    b,
                    &spectrum,
                    &mut out.products[at..at + m],
                )
                .unwrap();
            out.energy[b * k + lane] = shape.energy(&lanes[lane + 1], b);
        }
    }
    out
}

fn summed_on_f32(
    solver: &WienerSolver,
    shape: &BatchShape,
    plan: &GroupPlan,
    spectra: &Spectra,
) -> GroupSums {
    let (m, k, shifts, taps) = (shape.fft_len, shape.lanes, plan.shifts(), plan.taps());
    let mut sums = GroupSums::new(shape, plan);
    let mut turns = vec![C32::default(); shifts + taps];
    let mut vectors = vec![C32::default(); sums.vectors() * m];
    for group in 0..plan.groups() {
        vectors.fill(C32::default());
        for b in 0..shape.batches {
            solver.turns(group, b, &mut turns).unwrap();
            for (v, out) in vectors.chunks_exact_mut(m).enumerate() {
                let (source, turn) = if v < shifts {
                    (&spectra.models[b * m..(b + 1) * m], turns[v])
                } else {
                    let (lane, tap) = ((v - shifts) / taps, (v - shifts) % taps);
                    let at = (b * k + lane) * m;
                    (&spectra.products[at..at + m], turns[shifts + tap])
                };
                for (slot, value) in out.iter_mut().zip(source) {
                    *slot += turn * value;
                }
            }
        }
        sums.load(group, &vectors).unwrap();
    }
    for b in 0..shape.batches {
        solver
            .accumulate_energy(&mut sums, b, &spectra.energy[b * k..(b + 1) * k])
            .unwrap();
    }
    sums
}

#[test]
fn group_sums_loaded_from_f32_solve_like_accumulated_ones() {
    let fs = 266_666.67;
    let shape = BatchShape::new(32, 260, 40, 2, 10, 2).unwrap();
    let channels = [direct_paths(), vec![(0, C32::new(-1.0, 2.0))]];
    let lanes = fm_scene(&shape, &channels, 0.3, fs);
    let spectra = spectra_of(&shape, &lanes);
    let (m, k) = (shape.fft_len, shape.lanes);
    for plan in [
        GroupPlan::split(32, 8, 0, 1, true, false).unwrap(),
        GroupPlan::split(32, 8, 3, 1, false, true).unwrap(),
    ] {
        let mut solver = WienerSolver::new(shape, &plan, 1e-4).unwrap();
        let mut accumulated = GroupSums::new(&shape, &plan);
        for b in 0..shape.batches {
            solver
                .accumulate(
                    &mut accumulated,
                    b,
                    &spectra.models[b * m..(b + 1) * m],
                    &spectra.products[b * k * m..(b + 1) * k * m],
                    &spectra.energy[b * k..(b + 1) * k],
                )
                .unwrap();
        }
        let loaded = summed_on_f32(&solver, &shape, &plan, &spectra);
        let mut want = WeightTable::new(&shape, &plan).unwrap();
        let mut got = WeightTable::new(&shape, &plan).unwrap();
        let expected = solver.solve(&accumulated, &mut want).unwrap();
        let actual = solver.solve(&loaded, &mut got).unwrap();
        assert_eq!(actual.unsuppressed_groups, expected.unsuppressed_groups);
        for lane in 0..k {
            let (a, b) = (actual.suppression_db[lane], expected.suppression_db[lane]);
            assert!((a - b).abs() < 0.1, "{plan:?} lane {lane}: {a} vs {b}");
            for group in 0..plan.groups() {
                let reference = want.weights(group, lane);
                let scale = reference.iter().map(|w| w.norm()).fold(0.0, f64::max);
                for (a, b) in got.weights(group, lane).iter().zip(reference) {
                    assert!((a - b).norm() <= 1e-3 * scale, "{plan:?} {group} {lane}");
                }
            }
        }
    }
}

#[test]
fn turns_are_zero_outside_the_estimation_set() {
    let shape = BatchShape::new(40, 64, 10, 1, 4, 1).unwrap();
    let plan = GroupPlan::split(40, 10, 5, 1, false, true).unwrap();
    let solver = WienerSolver::new(shape, &plan, 1e-4).unwrap();
    let mut turns = vec![C32::new(9.0, 9.0); plan.shifts() + plan.taps()];
    assert_eq!(solver.estimation(1), Ok(5..25));
    solver.turns(1, 30, &mut turns).unwrap();
    assert!(turns.iter().all(|turn| *turn == C32::default()));
    solver.turns(1, 12, &mut turns).unwrap();
    assert!(turns.iter().all(|turn| (turn.norm() - 1.0).abs() < 1e-6));
    assert_eq!(solver.turns(4, 0, &mut turns), Err(RadarDspError::Shape));
    assert_eq!(
        solver.turns(0, 0, &mut turns[..2]),
        Err(RadarDspError::Shape)
    );
    assert_eq!(solver.estimation(4), Err(RadarDspError::Shape));
}

#[test]
fn mix_terms_rebuild_the_combined_kernel() {
    let fs = 266_666.67;
    let shape = BatchShape::new(32, 260, 40, 2, 10, 1).unwrap();
    let lanes = fm_scene(&shape, &[direct_paths()], 0.3, fs);
    let plan = GroupPlan::split(32, 8, 0, 1, true, false).unwrap();
    let caf = run_caf(shape, Some(&plan), &lanes, DopplerTaper::Hann);
    let table = caf.table.as_ref().unwrap();
    let m = shape.fft_len;
    let mut terms = vec![MixTerm::default(); 2 * plan.taps()];
    let mut combined = vec![C32::default(); m];
    let mut rebuilt = vec![C32::default(); m];
    for batch in 0..shape.batches {
        assert!(table.combine(0, batch, &mut combined).unwrap());
        table.terms(batch, &mut terms).unwrap();
        rebuilt.fill(C32::default());
        for term in terms.iter().filter(|term| term.factor != C32::default()) {
            let at = term.group * plan.taps() + term.tap;
            for (out, w) in rebuilt
                .iter_mut()
                .zip(&table.spectra()[at * m..(at + 1) * m])
            {
                *out += term.factor * w;
            }
        }
        assert_eq!(rebuilt, combined, "batch {batch}");
    }
    assert_eq!(
        table.terms(shape.batches, &mut terms),
        Err(RadarDspError::Shape)
    );
}
