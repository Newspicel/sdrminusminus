use std::f32::consts::LN_2;

use num_complex::Complex;

use super::{BeamError, Constraints, MAX_CONSTRAINTS, WeightSet, ZERO, common_len};
use crate::fft::FftPair;
use crate::linalg::{CMat, Eigen, HermitianEigen, LinalgError, MAX_ORDER};
use crate::window::hann;

pub const NOISE_FRAME: usize = 256;

const NOISE_EMA: f32 = 0.3;
const FLAT_RATIO: f32 = 0.5;
const FULL_EIGEN_RATIO: f32 = 10.0;
const SNR_FLOOR: f32 = 1e-6;
const NULL_FLOOR: f32 = 1e-6;
const MEDIAN: usize = NOISE_FRAME / 2;

pub struct LaneNoise {
    n: usize,
    fft: FftPair,
    window: Vec<f32>,
    window_energy: f32,
    frame: Vec<Complex<f32>>,
    bins: Vec<f32>,
    history: Vec<Complex<f32>>,
    seen: usize,
    noise: [f32; MAX_ORDER],
    flat: [f32; MAX_ORDER],
    scale: [f32; MAX_ORDER],
    spread: CMat,
    eigen: HermitianEigen,
    values: Eigen,
    full: bool,
    ready: bool,
}

impl LaneNoise {
    pub fn new(n: usize) -> Result<Self, BeamError> {
        if !(1..=MAX_ORDER).contains(&n) {
            return Err(BeamError::Lanes(n));
        }
        let window = hann(NOISE_FRAME);
        let window_energy = window.iter().map(|w| w * w).sum();
        Ok(Self {
            n,
            fft: FftPair::new(NOISE_FRAME),
            window,
            window_energy,
            frame: vec![ZERO; NOISE_FRAME],
            bins: vec![0.0; NOISE_FRAME],
            history: vec![ZERO; n * NOISE_FRAME],
            seen: 0,
            noise: [0.0; MAX_ORDER],
            flat: [0.0; MAX_ORDER],
            scale: [0.0; MAX_ORDER],
            spread: CMat::zeros(n)?,
            eigen: HermitianEigen::new(n)?,
            values: Eigen::new(),
            full: false,
            ready: false,
        })
    }

    pub fn push(&mut self, lanes: &[&[Complex<f32>]]) -> Result<(), BeamError> {
        let len = common_len(lanes, self.n)?;
        let fresh = len.min(NOISE_FRAME);
        if fresh == 0 {
            return Ok(());
        }
        for (row, lane) in self
            .history
            .as_chunks_mut::<NOISE_FRAME>()
            .0
            .iter_mut()
            .zip(lanes)
        {
            row.copy_within(fresh.., 0);
            row[NOISE_FRAME - fresh..].copy_from_slice(&lane[len - fresh..len]);
        }
        self.seen = self.seen.saturating_add(fresh);
        if self.seen < NOISE_FRAME {
            return Ok(());
        }
        let mut estimates = [0.0f32; MAX_ORDER];
        for (lane, estimate) in estimates.iter_mut().enumerate().take(self.n) {
            let (sigma, flat) = self.measure(lane);
            if !(sigma.is_finite() && flat.is_finite()) {
                return Err(LinalgError::NonFinite.into());
            }
            *estimate = sigma;
            self.flat[lane] = flat;
        }
        let full = self.flat[..self.n].iter().all(|&flat| flat > FLAT_RATIO)
            && self.coherent_ratio()? > FULL_EIGEN_RATIO;
        for (noise, estimate) in self.noise.iter_mut().zip(&estimates).take(self.n) {
            *noise = if self.ready {
                *noise + NOISE_EMA * (estimate - *noise)
            } else {
                *estimate
            };
        }
        self.full = full;
        self.ready = true;
        Ok(())
    }

    #[must_use]
    pub fn noise(&self) -> Option<&[f32]> {
        (self.ready && !self.full).then(|| &self.noise[..self.n])
    }

    #[must_use]
    pub const fn band_full(&self) -> bool {
        self.full
    }

    pub fn reset(&mut self) {
        self.seen = 0;
        self.ready = false;
        self.full = false;
        self.history.fill(ZERO);
    }

    fn measure(&mut self, lane: usize) -> (f32, f32) {
        let row = &self.history[lane * NOISE_FRAME..(lane + 1) * NOISE_FRAME];
        for ((slot, sample), weight) in self.frame.iter_mut().zip(row).zip(&self.window) {
            *slot = sample * weight;
        }
        self.fft.forward(&mut self.frame);
        for (bin, value) in self.bins.iter_mut().zip(&self.frame) {
            *bin = value.norm_sqr();
        }
        let mean = self.bins.iter().sum::<f32>() / NOISE_FRAME as f32;
        let (_, median, _) = self.bins.select_nth_unstable_by(MEDIAN, f32::total_cmp);
        let median = *median;
        let flat = if mean > 0.0 { median / mean } else { 0.0 };
        (median / (LN_2 * self.window_energy), flat)
    }

    fn coherent_ratio(&mut self) -> Result<f32, BeamError> {
        let n = self.n;
        if n < 2 {
            return Ok(0.0);
        }
        for row in 0..n {
            let left = &self.history[row * NOISE_FRAME..(row + 1) * NOISE_FRAME];
            for col in row..n {
                let right = &self.history[col * NOISE_FRAME..(col + 1) * NOISE_FRAME];
                let sum: Complex<f32> = left.iter().zip(right).map(|(a, b)| a * b.conj()).sum();
                self.spread.set(row, col, sum);
                self.spread.set(col, row, sum.conj());
            }
        }
        for row in 0..n {
            let power = self.spread.get(row, row).re;
            if !(power.is_finite() && power > 0.0) {
                return Ok(0.0);
            }
            self.scale[row] = power.sqrt().recip();
        }
        for row in 0..n {
            for col in 0..n {
                let value = self.spread.get(row, col) * (self.scale[row] * self.scale[col]);
                self.spread.set(row, col, value);
            }
        }
        self.eigen.solve(&self.spread, &mut self.values)?;
        let values = self.values.values();
        let rest = values[..n - 1].iter().sum::<f32>() / (n - 1) as f32;
        Ok(if rest > 0.0 {
            values[n - 1] / rest
        } else {
            f32::INFINITY
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeamMetrics {
    pub output_power: f32,
    pub snr_db: Option<f32>,
    pub gain_db: Option<f32>,
    pub cancelled_db: Option<f32>,
    pub null_depth_db: [f32; MAX_CONSTRAINTS],
    pub nulls: usize,
}

pub fn beam_metrics(
    r: &CMat,
    weights: &WeightSet,
    noise: Option<&[f32]>,
    primary: Option<usize>,
    nulls: &Constraints,
) -> Result<BeamMetrics, BeamError> {
    let n = weights.len();
    if r.order() != n {
        return Err(BeamError::Lanes(r.order()));
    }
    if nulls.count() > 0 && nulls.order() != n {
        return Err(BeamError::Lanes(nulls.order()));
    }
    let mut textbook = [ZERO; MAX_ORDER];
    weights.textbook(&mut textbook[..n]);
    let output = r.quad(&textbook[..n]).max(0.0);
    let mut metrics = BeamMetrics {
        output_power: output,
        ..BeamMetrics::default()
    };
    if let Some(noise) = noise {
        if noise.len() != n {
            return Err(BeamError::Lanes(noise.len()));
        }
        (metrics.snr_db, metrics.gain_db) = snr_and_gain(r, weights, noise, output);
    }
    if let Some(primary) = primary {
        if primary >= n {
            return Err(BeamError::Lanes(primary + 1));
        }
        let lane = r.get(primary, primary).re;
        metrics.cancelled_db = (output > 0.0 && lane > 0.0).then(|| db(lane / output));
    }
    fill_null_depths(weights, nulls, &mut metrics);
    Ok(metrics)
}

fn snr_and_gain(
    r: &CMat,
    weights: &WeightSet,
    noise: &[f32],
    output: f32,
) -> (Option<f32>, Option<f32>) {
    let floor: f32 = weights
        .as_slice()
        .iter()
        .zip(noise)
        .map(|(w, sigma)| w.norm_sqr() * sigma)
        .sum();
    if !(floor.is_finite() && floor > 0.0) {
        return (None, None);
    }
    let snr = db(((output - floor) / floor).max(SNR_FLOOR));
    let best = noise
        .iter()
        .enumerate()
        .filter(|(_, sigma)| sigma.is_finite() && **sigma > 0.0)
        .map(|(lane, sigma)| ((r.get(lane, lane).re - sigma) / sigma).max(SNR_FLOOR))
        .fold(None, |best: Option<f32>, value| {
            Some(best.map_or(value, |best| best.max(value)))
        });
    (Some(snr), best.map(|best| snr - db(best)))
}

fn fill_null_depths(weights: &WeightSet, nulls: &Constraints, metrics: &mut BeamMetrics) {
    let count = nulls.count();
    if count < 2 {
        return;
    }
    let main = weights.response(nulls.vector(0)).norm();
    metrics.nulls = count - 1;
    for (slot, index) in metrics.null_depth_db.iter_mut().zip(1..count) {
        let depth = weights.response(nulls.vector(index)).norm();
        let ratio = if main > 0.0 { depth / main } else { 1.0 };
        *slot = 20.0 * ratio.max(NULL_FLOOR).log10();
    }
}

fn db(ratio: f32) -> f32 {
    10.0 * ratio.log10()
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::BlockSolver;
    use crate::beamform::testing::{FREQ, covariance, kraken, steering, views};
    use crate::manifold::Direction;
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};

    fn scene(signal: SceneSignal, power_db: f32, noise_db: &[f32]) -> ArrayScene {
        let mut scene = ArrayScene::new(kraken().geometry().clone(), FREQ, 1e6)
            .with_seed(3)
            .with_source(SceneSource::new(
                Direction::horizon(137.0),
                power_db,
                signal,
            ));
        scene.noise_db = noise_db.to_vec();
        scene
    }

    fn settle(scene: &mut ArrayScene, noise: &mut LaneNoise) -> Vec<Vec<Complex<f32>>> {
        let mut last = Vec::new();
        for _ in 0..20 {
            last = scene.render(2_048).unwrap();
            noise.push(&views(&last)).unwrap();
        }
        last
    }

    const TONE: SceneSignal = SceneSignal::Tone { offset_hz: 3.1e4 };

    #[test]
    fn lane_noise_measures_unequal_lanes_under_a_tone() {
        let noise_db = [0.0, 10.0, -3.0, 10.0, 5.0];
        let mut scene = scene(TONE, 20.0, &noise_db);
        let mut noise = LaneNoise::new(5).unwrap();
        assert_eq!(noise.noise(), None);
        settle(&mut scene, &mut noise);
        assert!(!noise.band_full());
        let measured = noise.noise().unwrap();
        for (sigma, want) in measured.iter().zip(noise_db) {
            let error = 10.0 * sigma.log10() - want;
            assert!(error.abs() < 1.0, "{sigma} vs {want} dB");
        }
        assert_eq!(
            noise.push(&views(&scene.render(10).unwrap()[..2])),
            Err(BeamError::Lanes(2))
        );
    }

    #[test]
    fn gain_readout_is_ten_log_n_for_equal_lanes() {
        let manifold = kraken();
        let mut scene = scene(TONE, 10.0, &[0.0; 5]);
        let mut noise = LaneNoise::new(5).unwrap();
        settle(&mut scene, &mut noise);
        let lanes = scene.render(40_000).unwrap();
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        solver
            .das(&steering(&manifold, 137.0), &mut weights)
            .unwrap();
        let metrics =
            beam_metrics(&r, &weights, noise.noise(), None, &Constraints::new(5)).unwrap();
        let gain = metrics.gain_db.unwrap();
        assert!((gain - 7.0).abs() < 0.5, "gain {gain} dB");
        assert!((metrics.snr_db.unwrap() - 17.0).abs() < 0.5);
        assert!(metrics.cancelled_db.is_none());
        assert_eq!(metrics.nulls, 0);
    }

    #[test]
    fn band_full_hides_snr_readouts() {
        let mut scene = scene(SceneSignal::Broadband, 20.0, &[0.0; 5]);
        let mut noise = LaneNoise::new(5).unwrap();
        let lanes = settle(&mut scene, &mut noise);
        assert!(noise.band_full());
        assert_eq!(noise.noise(), None);
        let r = covariance(&lanes);
        let weights = WeightSet::unit(5, 0);
        let metrics =
            beam_metrics(&r, &weights, noise.noise(), None, &Constraints::new(5)).unwrap();
        assert_eq!(metrics.snr_db, None);
        assert_eq!(metrics.gain_db, None);
        assert!(metrics.output_power > 50.0);
        noise.reset();
        assert_eq!(noise.noise(), None);
        assert!(!noise.band_full());
    }

    #[test]
    fn a_louder_noise_lane_does_not_fill_the_band() {
        let noise_db = [20.0, 0.0, 0.0, 0.0, 0.0];
        let mut scene = scene(TONE, -30.0, &noise_db);
        let mut noise = LaneNoise::new(5).unwrap();
        settle(&mut scene, &mut noise);
        assert!(!noise.band_full());
        let measured = noise.noise().unwrap();
        for (sigma, want) in measured.iter().zip(noise_db) {
            let error = 10.0 * sigma.log10() - want;
            assert!(error.abs() < 1.0, "{sigma} vs {want} dB");
        }
    }

    #[test]
    fn a_non_finite_block_is_refused_and_leaves_the_estimate_intact() {
        let mut scene = scene(TONE, 10.0, &[0.0; 5]);
        let mut noise = LaneNoise::new(5).unwrap();
        settle(&mut scene, &mut noise);
        let before = noise.noise().unwrap().to_vec();
        let mut lanes = scene.render(512).unwrap();
        lanes[2][500] = Complex::new(f32::NAN, 0.0);
        assert_eq!(
            noise.push(&views(&lanes)),
            Err(BeamError::Linalg(LinalgError::NonFinite))
        );
        assert_eq!(noise.noise().unwrap(), before.as_slice());
        noise.push(&views(&scene.render(512).unwrap())).unwrap();
        assert!(noise.noise().unwrap().iter().all(|sigma| sigma.is_finite()));
        lanes[2].truncate(100);
        assert_eq!(noise.push(&views(&lanes)), Err(BeamError::LaneLength));
    }

    #[test]
    fn null_depths_and_cancellation_are_read_from_the_weights() {
        let manifold = kraken();
        let mut constraints = Constraints::new(5);
        constraints
            .push(&steering(&manifold, 10.0), Complex::new(1.0, 0.0))
            .unwrap();
        constraints
            .push(&steering(&manifold, 140.0), Complex::new(0.0, 0.0))
            .unwrap();
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        solver.lcmv(None, &constraints, 0.0, &mut weights).unwrap();
        let r = CMat::identity(5).unwrap();
        let metrics = beam_metrics(&r, &weights, None, Some(0), &constraints).unwrap();
        assert_eq!(metrics.nulls, 1);
        assert!(metrics.null_depth_db[0] < -40.0);
        let expected = 10.0 * (1.0 / weights.norm_sqr()).log10();
        assert!((metrics.cancelled_db.unwrap() - expected).abs() < 1e-3);
        assert_eq!(
            beam_metrics(&r, &WeightSet::zeros(3), None, None, &constraints),
            Err(BeamError::Lanes(5))
        );
        assert_eq!(
            beam_metrics(&r, &weights, None, Some(5), &constraints),
            Err(BeamError::Lanes(6))
        );
    }
}
