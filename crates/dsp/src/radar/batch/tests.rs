use std::f64::consts::TAU;

use super::*;
use crate::manifold::{Direction, Geometry};
use crate::radar::wiener::{GroupPlan, GroupSums, SolveStats, WienerSolver};
use crate::scene::{ArrayScene, SceneEcho, SceneSignal, SceneSource};

pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(crate) fn complex(&mut self) -> C32 {
        let radius = (-(1.0 - self.uniform()).ln()).sqrt();
        let angle = TAU * self.uniform();
        C32::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
    }

    pub(crate) fn noise(&mut self, len: usize) -> Vec<C32> {
        (0..len).map(|_| self.complex()).collect()
    }
}

pub(crate) struct Caf {
    pub(crate) gates: Vec<C32>,
    pub(crate) cube: Vec<C32>,
    pub(crate) stats: Option<SolveStats>,
    pub(crate) table: Option<WeightTable>,
}

impl Caf {
    pub(crate) fn gate(&self, shape: &BatchShape, lane: usize, batch: usize, tau: usize) -> C32 {
        self.gates[(lane * shape.batches + batch) * shape.gates + tau]
    }

    pub(crate) fn cell(&self, shape: &BatchShape, lane: usize, tau: usize, row: usize) -> C32 {
        self.cube[(lane * shape.gates + tau) * shape.batches + row]
    }

    pub(crate) fn peak(&self, shape: &BatchShape, lane: usize, from_gate: usize) -> (usize, usize) {
        let mut best = (0, 0, 0.0f32);
        for tau in from_gate..shape.gates {
            for row in 0..shape.batches {
                let power = self.cell(shape, lane, tau, row).norm_sqr();
                if power > best.2 {
                    best = (tau, row, power);
                }
            }
        }
        (best.0, best.1)
    }
}

pub(crate) fn run_caf(
    shape: BatchShape,
    plan: Option<&GroupPlan>,
    lanes: &[Vec<C32>],
    taper: DopplerTaper,
) -> Caf {
    let (m, nb, k) = (shape.fft_len, shape.batches, shape.lanes);
    let mut kernel = BatchKernel::new(shape);
    let mut spectra = vec![C32::default(); nb * m];
    let mut models = vec![C32::default(); nb * m];
    let mut products = vec![C32::default(); nb * k * m];
    let mut energy = vec![0.0f64; nb * k];
    for b in 0..nb {
        let spectrum = &mut spectra[b * m..(b + 1) * m];
        kernel
            .reference(&lanes[0], b, spectrum, &mut models[b * m..(b + 1) * m])
            .unwrap();
        for lane in 0..k {
            let at = (b * k + lane) * m;
            kernel
                .surveillance(&lanes[lane + 1], b, spectrum, &mut products[at..at + m])
                .unwrap();
            energy[b * k + lane] = shape.energy(&lanes[lane + 1], b);
        }
    }
    let solved = plan.map(|plan| {
        let mut solver = WienerSolver::new(shape, plan, 1e-4).unwrap();
        let mut sums = GroupSums::new(&shape, plan);
        for b in 0..nb {
            solver
                .accumulate(
                    &mut sums,
                    b,
                    &models[b * m..(b + 1) * m],
                    &products[b * k * m..(b + 1) * k * m],
                    &energy[b * k..(b + 1) * k],
                )
                .unwrap();
        }
        let mut table = WeightTable::new(&shape, plan).unwrap();
        let stats = solver.solve(&sums, &mut table).unwrap();
        (stats, table)
    });
    let mut gates = vec![C32::default(); k * nb * shape.gates];
    for lane in 0..k {
        for b in 0..nb {
            let weights = solved
                .as_ref()
                .map_or(WeightsAt::none(), |(_, table)| table.at(lane, b));
            let at = (lane * nb + b) * shape.gates;
            kernel
                .residual(
                    &products[(b * k + lane) * m..(b * k + lane + 1) * m],
                    &models[b * m..(b + 1) * m],
                    weights,
                    &mut gates[at..at + shape.gates],
                )
                .unwrap();
        }
    }
    let mut window = vec![0.0f32; nb];
    taper.fill(&mut window);
    let mut cube = vec![C32::default(); k * shape.gates * nb];
    for lane in 0..k {
        for tau in 0..shape.gates {
            let series =
                &mut cube[(lane * shape.gates + tau) * nb..(lane * shape.gates + tau + 1) * nb];
            for (b, value) in series.iter_mut().enumerate() {
                *value = gates[(lane * nb + b) * shape.gates + tau];
            }
            kernel.doppler(series, &window).unwrap();
        }
    }
    let (stats, table) = match solved {
        Some((stats, table)) => (Some(stats), Some(table)),
        None => (None, None),
    };
    Caf {
        gates,
        cube,
        stats,
        table,
    }
}

pub(crate) fn correlate(
    shape: &BatchShape,
    reference: &[C32],
    surveillance: &[C32],
    batch: usize,
    lag: i64,
) -> C32 {
    let pre = shape.pre() as i64;
    (0..shape.batch_len)
        .map(|n| {
            let at = pre + (batch * shape.batch_len + n) as i64;
            surveillance[(at + lag) as usize] * reference[at as usize].conj()
        })
        .sum()
}

#[test]
fn batch_correlation_equals_direct_sum_at_every_gate() {
    let shape = BatchShape::new(32, 100, 40, 0, 0, 1).unwrap();
    let mut rng = Rng(3);
    let lanes = vec![rng.noise(shape.window()), rng.noise(shape.window())];
    let caf = run_caf(shape, None, &lanes, DopplerTaper::Rectangular);
    let mut worst = 0.0f32;
    let mut scale = 0.0f32;
    for b in 0..shape.batches {
        for tau in 0..shape.gates {
            let direct = correlate(&shape, &lanes[0], &lanes[1], b, tau as i64);
            worst = worst.max((caf.gate(&shape, 0, b, tau) - direct).norm());
            scale = scale.max(direct.norm());
        }
    }
    assert!(worst < 1e-4 * scale, "{worst} {scale}");
}

fn two_element_scene(fs: f64, echo: SceneEcho) -> Vec<Vec<C32>> {
    let geometry = Geometry::ula(0.5, 2, 90.0).unwrap();
    let mut scene = ArrayScene::new(geometry, 100e6, fs)
        .with_source(SceneSource::new(
            Direction::horizon(0.0),
            0.0,
            SceneSignal::Broadband,
        ))
        .with_echo(echo)
        .with_noise_db(-30.0)
        .with_seed(11);
    scene.render(20_000).unwrap()
}

#[test]
fn an_echo_lands_on_its_delay_and_doppler() {
    let fs = 200e3;
    let shape = BatchShape::new(64, 250, 40, 0, 0, 1).unwrap();
    let echo = SceneEcho::new(0, Direction::horizon(60.0), -20.0, 23.0, 37.5);
    let lanes = two_element_scene(fs, echo);
    let caf = run_caf(shape, None, &lanes, DopplerTaper::Hann);
    assert_eq!(caf.peak(&shape, 0, 3), (23, 32 + 3));
    assert_eq!(caf.peak(&shape, 0, 0), (0, 32));
}

#[test]
fn closing_targets_have_positive_doppler() {
    let fs = 200e3;
    let shape = BatchShape::new(64, 250, 40, 0, 0, 1).unwrap();
    for (rate, above) in [(-90.0, true), (90.0, false)] {
        let echo = SceneEcho::bistatic(
            0,
            Direction::horizon(60.0),
            -20.0,
            30_000.0,
            rate,
            fs,
            100e6,
        );
        let lanes = two_element_scene(fs, echo);
        let caf = run_caf(shape, None, &lanes, DopplerTaper::Hann);
        let (tau, row) = caf.peak(&shape, 0, 5);
        assert_eq!(tau, 20);
        assert_eq!(row > 32, above, "{rate}: {row}");
        assert_eq!(row.abs_diff(32), 2);
    }
}

fn shifted_copy(reference: &[C32], delay: usize, doppler: f64, fs: f64) -> Vec<C32> {
    (0..reference.len())
        .map(|n| {
            let source = if n >= delay {
                reference[n - delay]
            } else {
                C32::default()
            };
            source * C32::from_polar(1.0, (TAU * doppler * n as f64 / fs) as f32)
        })
        .collect()
}

#[test]
fn straddle_loss_follows_sinc() {
    let fs = 100e3;
    let shape = BatchShape::new(64, 100, 16, 0, 0, 1).unwrap();
    let mut rng = Rng(5);
    let reference = rng.noise(shape.window() + 64);
    let doppler = shape.batches as f64 / (4.0 * shape.samples() as f64 / fs);
    let still = run_caf(
        shape,
        None,
        &[reference.clone(), shifted_copy(&reference, 7, 0.0, fs)],
        DopplerTaper::Rectangular,
    );
    let moving = run_caf(
        shape,
        None,
        &[reference.clone(), shifted_copy(&reference, 7, doppler, fs)],
        DopplerTaper::Rectangular,
    );
    let row = 32 + shape.batches / 4;
    assert_eq!(moving.peak(&shape, 0, 0), (7, row));
    let loss = 20.0
        * (moving.cell(&shape, 0, 7, row).norm() / still.cell(&shape, 0, 7, 32).norm()).log10();
    let expected =
        20.0 * ((std::f32::consts::PI * 0.25).sin() / (std::f32::consts::PI * 0.25)).log10();
    assert!((loss - expected).abs() < 0.3, "{loss} vs {expected}");
}

#[test]
fn hann_sidelobes_stay_below_31_db() {
    let shape = BatchShape::new(64, 16, 4, 0, 0, 1).unwrap();
    let mut kernel = BatchKernel::new(shape);
    let mut window = vec![0.0f32; 64];
    DopplerTaper::Hann.fill(&mut window);
    for offset in [5.4f64, 11.25, -20.4] {
        let mut series: Vec<C32> = (0..64)
            .map(|b| C32::from_polar(1.0, (TAU * offset * b as f64 / 64.0) as f32))
            .collect();
        kernel.doppler(&mut series, &window).unwrap();
        let (peak, top) = series
            .iter()
            .enumerate()
            .map(|(row, value)| (row, value.norm()))
            .fold(
                (0, 0.0f32),
                |best, next| if next.1 > best.1 { next } else { best },
            );
        assert_eq!(peak, (32.0 + offset).round() as usize);
        let sidelobe = series
            .iter()
            .enumerate()
            .filter(|(row, _)| row.abs_diff(peak) > 2)
            .map(|(_, value)| value.norm())
            .fold(0.0f32, f32::max);
        assert!(20.0 * (sidelobe / top).log10() < -31.0, "{offset}");
    }
    assert_eq!(DopplerTaper::Hann.enbw(), 1.5);
    let mut flat = vec![0.0f32; 8];
    DopplerTaper::Rectangular.fill(&mut flat);
    assert!(flat.iter().all(|&w| w == 1.0));
    let mut harris = vec![0.0f32; 64];
    DopplerTaper::BlackmanHarris.fill(&mut harris);
    let enbw =
        64.0 * harris.iter().map(|w| w * w).sum::<f32>() / harris.iter().sum::<f32>().powi(2);
    assert!((f64::from(enbw) - DopplerTaper::BlackmanHarris.enbw()).abs() < 0.01);
}

fn inverse(kernel: &mut BatchKernel, spectrum: &[C32]) -> Vec<C32> {
    let mut buffer = spectrum.to_vec();
    kernel.fft.inverse_scaled(&mut buffer);
    buffer
}

#[test]
fn sizes_cover_every_lag_without_aliasing() {
    let mut rng = Rng(17);
    let mut checked = 0;
    for lead in 0..=3usize {
        for taps in 0..=5usize {
            if lead > 0 && taps == 0 {
                assert!(BatchShape::new(2, 4, 4, lead, taps, 1).is_err());
                continue;
            }
            for gates in [1usize, 2, 5, 9] {
                for batch_len in [1usize, 3, 8] {
                    let shape = BatchShape::new(3, batch_len, gates, lead, taps, 1).unwrap();
                    assert!(shape.fft_len.is_power_of_two() && shape.fft_len >= MIN_FFT);
                    let reference = rng.noise(shape.window());
                    let surveillance = rng.noise(shape.window());
                    let mut kernel = BatchKernel::new(shape);
                    let m = shape.fft_len;
                    let (mut spectrum, mut model, mut product) = (
                        vec![C32::default(); m],
                        vec![C32::default(); m],
                        vec![C32::default(); m],
                    );
                    for b in 0..shape.batches {
                        kernel
                            .reference(&reference, b, &mut spectrum, &mut model)
                            .unwrap();
                        kernel
                            .surveillance(&surveillance, b, &spectrum, &mut product)
                            .unwrap();
                        let cross = inverse(&mut kernel, &product);
                        for (index, value) in cross.iter().enumerate().take(shape.span()) {
                            let direct = correlate(
                                &shape,
                                &reference,
                                &surveillance,
                                b,
                                index as i64 - lead as i64,
                            );
                            assert!((value - direct).norm() < 1e-4 * (1.0 + direct.norm()));
                        }
                        if shape.eca() {
                            let auto = inverse(&mut kernel, &model);
                            let used = shape.span() + shape.order() - 1;
                            for (index, value) in auto.iter().enumerate().take(used) {
                                let lag = index as i64 - (shape.order() as i64 - 1);
                                let direct = correlate(&shape, &reference, &reference, b, lag);
                                assert!((value - direct).norm() < 1e-4 * (1.0 + direct.norm()));
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 100);
}

#[test]
fn worked_example_shapes_match_the_plan() {
    let fm = BatchShape::new(512, 260, 73, 2, 14, 4).unwrap();
    assert_eq!((fm.span(), fm.fft_len), (75, 512));
    let dab = BatchShape::new(1024, 1000, 548, 2, 103, 4).unwrap();
    assert_eq!((dab.span(), dab.fft_len), (550, 2048));
    let plain = BatchShape::new(512, 260, 73, 0, 0, 4).unwrap();
    assert_eq!((plain.pre(), plain.post(), plain.span()), (0, 72, 73));
    assert_eq!(fm.window(), 512 * 260 + 15 + 74);
    assert_eq!(
        BatchShape::new(4, 1 << 20, 4, 0, 0, 1),
        Err(RadarDspError::Size)
    );
    assert_eq!(
        BatchShape::new(4, 16, 4, 0, 0, 16),
        Err(RadarDspError::Shape)
    );
}

#[test]
fn mismatched_buffers_are_refused() {
    let shape = BatchShape::new(4, 16, 4, 1, 2, 1).unwrap();
    let mut kernel = BatchKernel::new(shape);
    let window = vec![C32::default(); shape.window() - 1];
    let mut buffer = vec![C32::default(); shape.fft_len];
    let mut model = vec![C32::default(); shape.fft_len];
    assert_eq!(
        kernel.reference(&window, 0, &mut buffer, &mut model),
        Err(RadarDspError::Shape)
    );
    let window = vec![C32::default(); shape.window()];
    assert_eq!(
        kernel.reference(&window, 4, &mut buffer, &mut model),
        Err(RadarDspError::Shape)
    );
    let mut gates = vec![C32::default(); 3];
    assert_eq!(
        kernel.residual(&buffer, &model, WeightsAt::none(), &mut gates),
        Err(RadarDspError::Shape)
    );
    let mut series = vec![C32::default(); 3];
    assert_eq!(
        kernel.doppler(&mut series, &[1.0; 4]),
        Err(RadarDspError::Shape)
    );
}
