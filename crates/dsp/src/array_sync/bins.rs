use std::{cmp::Ordering, f64::consts::TAU};

use num_complex::Complex;

use super::eigen::{POWER_ITERATIONS, dominant};
use crate::{fft::FftPair, window::hann};

pub const EQ_POINTS: usize = 64;
pub const FIT_BAND: f64 = 0.4;
pub const MIN_BINS: usize = 64;

const EQ_SMOOTH: usize = 9;
const EQ_LIMIT_DB: f32 = 6.0;
const REFERENCE_SHARE: f32 = 0.05;

#[derive(Clone, Debug, PartialEq)]
pub struct LaneResponse {
    pub delay_frac: f32,
    pub phase_rad: f32,
    pub gain: f32,
    pub coherence: f32,
    pub equaliser: Vec<Complex<f32>>,
}

impl LaneResponse {
    fn reference() -> Self {
        Self {
            delay_frac: 0.0,
            phase_rad: 0.0,
            gain: 1.0,
            coherence: 1.0,
            equaliser: vec![Complex::new(1.0, 0.0); EQ_POINTS],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BinSolution {
    pub lanes: Vec<LaneResponse>,
    pub purity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum BinError {
    #[error("lane {lane} coherence {coherence}")]
    LowCoherence { lane: usize, coherence: f32 },
    #[error("too few usable bins")]
    FewBins,
    #[error("capture too short")]
    Short,
    #[error("expected {expected} lanes, got {got}")]
    LaneCount { expected: usize, got: usize },
}

#[must_use]
pub fn equaliser_frequency(point: usize, points: usize) -> f32 {
    -0.5 + point as f32 / points.max(1) as f32
}

#[must_use]
pub fn equaliser_at(equaliser: &[Complex<f32>], frequency: f32) -> Complex<f32> {
    let points = equaliser.len();
    if points == 0 {
        return Complex::new(1.0, 0.0);
    }
    let position = (f64::from(frequency) + 0.5).rem_euclid(1.0) * points as f64;
    let below = position.floor();
    let fraction = (position - below) as f32;
    let below = below as usize % points;
    let above = (below + 1) % points;
    equaliser[below] * (1.0 - fraction) + equaliser[above] * fraction
}

#[derive(Clone, Copy, Debug, Default)]
struct BinFit {
    lambda: f32,
    trace: f32,
    used: bool,
}

pub struct BinSolver {
    fft: FftPair,
    len: usize,
    window: Vec<f32>,
    spectra: Vec<Vec<Complex<f32>>>,
    cov: Vec<Complex<f32>>,
    lanes: usize,
    fits: Vec<BinFit>,
    responses: Vec<Complex<f32>>,
    phase: Vec<f64>,
    residual: Vec<Complex<f32>>,
    smoothed: Vec<Complex<f32>>,
}

impl BinSolver {
    #[must_use]
    pub fn new(lanes: usize, len: usize) -> Self {
        let len = len.max(2);
        Self {
            fft: FftPair::new(len),
            len,
            window: hann(len),
            spectra: vec![vec![Complex::default(); len]; lanes],
            cov: vec![Complex::default(); len * lanes * lanes],
            lanes,
            fits: vec![BinFit::default(); len],
            responses: vec![Complex::default(); len * lanes],
            phase: vec![0.0; len],
            residual: vec![Complex::default(); len],
            smoothed: vec![Complex::default(); len],
        }
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.lanes
    }

    #[must_use]
    pub const fn bins(&self) -> usize {
        self.len
    }

    pub fn solve(
        &mut self,
        lanes: &[&[Complex<f32>]],
        purity_min: f32,
        coherence_min: f32,
    ) -> Result<BinSolution, BinError> {
        if lanes.len() != self.lanes {
            return Err(BinError::LaneCount {
                expected: self.lanes,
                got: lanes.len(),
            });
        }
        let samples = lanes.iter().map(|lane| lane.len()).min().unwrap_or(0);
        if samples < self.len {
            return Err(BinError::Short);
        }
        self.accumulate(lanes, samples);
        self.decompose(purity_min);
        if self.fits.iter().filter(|fit| fit.used).count() < MIN_BINS {
            return Err(BinError::FewBins);
        }
        let mut responses = Vec::with_capacity(self.lanes);
        responses.push(LaneResponse::reference());
        for lane in 1..self.lanes {
            responses.push(self.lane_response(lane));
        }
        let weakest = responses
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, response)| {
                matches!(
                    response.coherence.partial_cmp(&coherence_min),
                    None | Some(Ordering::Less)
                )
            })
            .min_by(|a, b| a.1.coherence.total_cmp(&b.1.coherence));
        if let Some((lane, response)) = weakest {
            return Err(BinError::LowCoherence {
                lane,
                coherence: response.coherence,
            });
        }
        Ok(BinSolution {
            lanes: responses,
            purity: self.purity(),
        })
    }

    fn accumulate(&mut self, lanes: &[&[Complex<f32>]], samples: usize) {
        let order = self.lanes;
        let hop = self.len / 2;
        let blocks = (samples - self.len) / hop + 1;
        self.cov.fill(Complex::default());
        for block in 0..blocks {
            let start = block * hop;
            for (spectrum, lane) in self.spectra.iter_mut().zip(lanes) {
                for ((value, sample), weight) in spectrum
                    .iter_mut()
                    .zip(&lane[start..start + self.len])
                    .zip(&self.window)
                {
                    *value = sample * weight;
                }
                self.fft.forward(spectrum);
            }
            for (bin, matrix) in self.cov.chunks_exact_mut(order * order).enumerate() {
                for (row, cells) in matrix.chunks_exact_mut(order).enumerate() {
                    let left = self.spectra[row][bin];
                    for (cell, right) in cells.iter_mut().zip(&self.spectra) {
                        *cell += left * right[bin].conj();
                    }
                }
            }
        }
        let scale = 1.0 / blocks as f32;
        for cell in &mut self.cov {
            *cell *= scale;
        }
    }

    fn decompose(&mut self, purity_min: f32) {
        let order = self.lanes;
        let len = self.len;
        for (bin, ((matrix, vector), fit)) in self
            .cov
            .chunks_exact(order * order)
            .zip(self.responses.chunks_exact_mut(order))
            .zip(&mut self.fits)
            .enumerate()
        {
            let lambda = dominant(matrix, order, POWER_ITERATIONS, vector);
            let trace: f32 = (0..order).map(|lane| matrix[lane * order + lane].re).sum();
            let reference = vector[0];
            let usable = trace > 0.0
                && lambda > 0.0
                && reference.norm_sqr() * order as f32 >= REFERENCE_SHARE;
            if usable {
                for value in vector.iter_mut() {
                    *value /= reference;
                }
            }
            let purity = if usable { lambda / trace } else { 0.0 };
            *fit = BinFit {
                lambda,
                trace,
                used: usable && frequency(bin, len).abs() <= FIT_BAND && purity >= purity_min,
            };
        }
    }

    fn lane_response(&mut self, lane: usize) -> LaneResponse {
        self.unwrap_phase(lane);
        let (phase, delay) = self.fit_phase_slope();
        let gain = self.mean_gain(lane);
        let coherence = self.coherence(lane, phase, delay);
        let equaliser = self.equaliser(lane, phase, delay, gain);
        LaneResponse {
            delay_frac: delay as f32,
            phase_rad: wrap(phase) as f32,
            gain: gain as f32,
            coherence,
            equaliser,
        }
    }

    fn bin_of(&self, centred: usize) -> usize {
        (centred + self.len - self.len / 2) % self.len
    }

    fn is_used(&self, centred: usize) -> bool {
        self.fits[self.bin_of(centred)].used
    }

    fn centred_arg(&self, centred: usize, lane: usize) -> f64 {
        f64::from(self.responses[self.bin_of(centred) * self.lanes + lane].arg())
    }

    fn unwrap_phase(&mut self, lane: usize) {
        let half = self.len / 2;
        let Some(anchor) = (0..self.len)
            .filter(|&centred| self.is_used(centred))
            .min_by_key(|&centred| centred.abs_diff(half))
        else {
            return;
        };
        let start = self.centred_arg(anchor, lane);
        let mut phase = std::mem::take(&mut self.phase);
        phase[self.bin_of(anchor)] = start;
        self.unwrap_run(anchor + 1..self.len, start, lane, &mut phase);
        self.unwrap_run((0..anchor).rev(), start, lane, &mut phase);
        self.phase = phase;
    }

    fn unwrap_run(
        &self,
        run: impl Iterator<Item = usize>,
        start: f64,
        lane: usize,
        phase: &mut [f64],
    ) {
        let mut previous = start;
        for centred in run.filter(|&centred| self.is_used(centred)) {
            previous += wrap(self.centred_arg(centred, lane) - previous);
            phase[self.bin_of(centred)] = previous;
        }
    }

    fn fit_phase_slope(&self) -> (f64, f64) {
        let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (bin, fit) in self.fits.iter().enumerate().filter(|(_, fit)| fit.used) {
            let weight = f64::from(fit.lambda);
            let x = -TAU * frequency(bin, self.len);
            let y = self.phase[bin];
            sw += weight;
            sx += weight * x;
            sy += weight * y;
            sxx += weight * x * x;
            sxy += weight * x * y;
        }
        let determinant = sw * sxx - sx * sx;
        if sw <= f64::MIN_POSITIVE {
            return (0.0, 0.0);
        }
        if determinant.abs() <= f64::MIN_POSITIVE {
            return (sy / sw, 0.0);
        }
        let slope = (sw * sxy - sx * sy) / determinant;
        ((sy - slope * sx) / sw, slope)
    }

    fn mean_gain(&self, lane: usize) -> f64 {
        let order = self.lanes;
        let (mut weighted, mut total) = (0.0, 0.0);
        for (bin, fit) in self.fits.iter().enumerate().filter(|(_, fit)| fit.used) {
            let weight = f64::from(fit.lambda);
            weighted += weight * f64::from(self.responses[bin * order + lane].norm());
            total += weight;
        }
        if total > 0.0 { weighted / total } else { 0.0 }
    }

    fn coherence(&self, lane: usize, phase: f64, delay: f64) -> f32 {
        let order = self.lanes;
        let mut cross = Complex::<f64>::default();
        let (mut reference, mut power) = (0.0, 0.0);
        for (bin, _) in self.fits.iter().enumerate().filter(|(_, fit)| fit.used) {
            let matrix = &self.cov[bin * order * order..(bin + 1) * order * order];
            let model = Complex::from_polar(1.0, phase - TAU * frequency(bin, self.len) * delay);
            cross += widen(matrix[lane * order]) * model.conj();
            reference += f64::from(matrix[0].re);
            power += f64::from(matrix[lane * order + lane].re);
        }
        if reference <= 0.0 || power <= 0.0 {
            return 0.0;
        }
        (cross.norm_sqr() / (reference * power)) as f32
    }

    fn equaliser(&mut self, lane: usize, phase: f64, delay: f64, gain: f64) -> Vec<Complex<f32>> {
        let order = self.lanes;
        let one = Complex::new(1.0, 0.0);
        for centred in 0..self.len {
            let bin = self.bin_of(centred);
            self.residual[centred] = if self.fits[bin].used && gain > 0.0 {
                let model =
                    Complex::from_polar(gain, phase - TAU * frequency(bin, self.len) * delay);
                narrow(widen(self.responses[bin * order + lane]) / model)
            } else {
                one
            };
        }
        let reach = EQ_SMOOTH / 2;
        for centred in 0..self.len {
            self.smoothed[centred] = if self.is_used(centred) {
                clip(self.used_mean(&self.residual, centred, reach))
            } else {
                one
            };
        }
        let spread = self.len / EQ_POINTS / 2;
        (0..EQ_POINTS)
            .map(|point| {
                let centre = ((point as f64 / EQ_POINTS as f64) * self.len as f64).round() as usize;
                let centre = centre.min(self.len - 1);
                if (centre.saturating_sub(spread)..=(centre + spread).min(self.len - 1))
                    .any(|centred| self.is_used(centred))
                {
                    clip(self.used_mean(&self.smoothed, centre, spread))
                } else {
                    one
                }
            })
            .collect()
    }

    fn used_mean(&self, values: &[Complex<f32>], centre: usize, reach: usize) -> Complex<f32> {
        let mut sum = Complex::<f32>::default();
        let mut count = 0usize;
        let first = centre.saturating_sub(reach);
        let last = (centre + reach).min(self.len - 1);
        for (centred, value) in values.iter().enumerate().take(last + 1).skip(first) {
            if self.is_used(centred) {
                sum += value;
                count += 1;
            }
        }
        if count == 0 {
            Complex::new(1.0, 0.0)
        } else {
            sum / count as f32
        }
    }

    fn purity(&self) -> f32 {
        let (lambda, trace) = self
            .fits
            .iter()
            .filter(|fit| fit.used)
            .fold((0.0f64, 0.0f64), |(lambda, trace), fit| {
                (lambda + f64::from(fit.lambda), trace + f64::from(fit.trace))
            });
        if trace > 0.0 {
            (lambda / trace) as f32
        } else {
            0.0
        }
    }
}

fn frequency(bin: usize, len: usize) -> f64 {
    if bin < len.div_ceil(2) {
        bin as f64 / len as f64
    } else {
        (bin as f64 - len as f64) / len as f64
    }
}

fn wrap(angle: f64) -> f64 {
    angle.sin().atan2(angle.cos())
}

fn widen(value: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(value.re), f64::from(value.im))
}

fn narrow(value: Complex<f64>) -> Complex<f32> {
    Complex::new(value.re as f32, value.im as f32)
}

fn clip(value: Complex<f32>) -> Complex<f32> {
    let low = 10f32.powf(-EQ_LIMIT_DB / 20.0);
    let high = 10f32.powf(EQ_LIMIT_DB / 20.0);
    let magnitude = value.norm();
    if !magnitude.is_finite() || magnitude <= f32::MIN_POSITIVE {
        return Complex::new(low, 0.0);
    }
    value * (magnitude.clamp(low, high) / magnitude)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::array_sync::signals::{Gaussian, lane_response, shaped, with_noise};

    const SOLVE_LEN: usize = 65_536;
    const BINS: usize = 1_024;

    fn lanes_from(
        source: &[Complex<f32>],
        responses: &[(f64, f64, f64)],
        noise: f32,
        seed: u64,
    ) -> Vec<Vec<Complex<f32>>> {
        let mut generator = Gaussian::new(seed);
        let mut lanes = vec![with_noise(source, noise, &mut generator)];
        for &(delay, phase_deg, gain_db) in responses {
            let response = lane_response(delay, phase_deg.to_radians(), 10f64.powf(gain_db / 20.0));
            lanes.push(with_noise(&shaped(source, response), noise, &mut generator));
        }
        lanes
    }

    fn solve(
        lanes: &[Vec<Complex<f32>>],
        purity_min: f32,
        coherence_min: f32,
    ) -> Result<BinSolution, BinError> {
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        BinSolver::new(lanes.len(), BINS).solve(&views, purity_min, coherence_min)
    }

    fn assert_close(response: &LaneResponse, (delay, phase_deg, gain_db): (f64, f64, f64)) {
        let delay_error = f64::from(response.delay_frac) - delay;
        let phase_error = wrap(f64::from(response.phase_rad) - phase_deg.to_radians()).to_degrees();
        let gain_error = 20.0 * f64::from(response.gain).log10() - gain_db;
        assert!(delay_error.abs() < 0.01, "delay {delay}: {delay_error}");
        assert!(phase_error.abs() < 0.5, "phase {phase_deg}: {phase_error}");
        assert!(gain_error.abs() < 0.05, "gain {gain_db}: {gain_error}");
    }

    #[test]
    fn bin_solver_recovers_delay_phase_and_gain() {
        let truth = [(0.37, 73.0, -2.5), (-0.21, -140.0, 1.5), (0.05, 12.0, 0.0)];
        let source = Gaussian::new(1).block(SOLVE_LEN);
        let lanes = lanes_from(&source, &truth, 0.03, 2);
        let solution = solve(&lanes, 0.8, 0.9).unwrap();
        assert_eq!(solution.lanes.len(), 4);
        assert!(solution.purity > 0.99, "{}", solution.purity);
        assert_eq!(solution.lanes[0], LaneResponse::reference());
        for (response, truth) in solution.lanes[1..].iter().zip(truth) {
            assert_close(response, truth);
            assert!(response.coherence > 0.99, "{}", response.coherence);
            assert_eq!(response.equaliser.len(), EQ_POINTS);
            for point in &response.equaliser {
                assert!(20.0 * point.norm().log10().abs() < 0.2, "{point}");
            }
        }
    }

    #[test]
    fn bin_solver_recovers_an_equaliser_ripple() {
        let ripple_db = |nu: f64| (TAU * 3.0 * nu).cos();
        let source = Gaussian::new(5).block(SOLVE_LEN);
        let mut generator = Gaussian::new(6);
        let lane = shaped(&source, |nu| {
            lane_response(0.2, 40f64.to_radians(), 0.9)(nu) * 10f64.powf(ripple_db(nu) / 20.0)
        });
        let lanes = [
            with_noise(&source, 0.03, &mut generator),
            with_noise(&lane, 0.03, &mut generator),
        ];
        let solution = solve(&lanes, 0.8, 0.9).unwrap();
        let response = &solution.lanes[1];
        assert!((f64::from(response.delay_frac) - 0.2).abs() < 0.01);
        assert!(
            wrap(f64::from(response.phase_rad) - 40f64.to_radians())
                .to_degrees()
                .abs()
                < 0.5
        );
        let compared: Vec<(f64, f64)> = response
            .equaliser
            .iter()
            .enumerate()
            .map(|(point, value)| (f64::from(equaliser_frequency(point, EQ_POINTS)), value))
            .filter(|(nu, _)| nu.abs() <= 0.375)
            .map(|(nu, value)| (nu, 20.0 * f64::from(value.norm()).log10() - ripple_db(nu)))
            .collect();
        let offset = compared.iter().map(|(_, error)| error).sum::<f64>() / compared.len() as f64;
        for (nu, error) in compared {
            assert!((error - offset).abs() < 0.2, "nu {nu}: {error} dB off");
        }
    }

    #[test]
    fn bin_solver_flags_an_incoherent_lane() {
        let source = Gaussian::new(8).block(SOLVE_LEN);
        let mut lanes = lanes_from(
            &source,
            &[(0.1, 30.0, 0.0), (0.0, 0.0, 0.0), (-0.2, -60.0, 0.0)],
            0.03,
            9,
        );
        lanes[2] = Gaussian::new(10).block(SOLVE_LEN);
        match solve(&lanes, 0.5, 0.6) {
            Err(BinError::LowCoherence { lane, coherence }) => {
                assert_eq!(lane, 2);
                assert!(coherence < 0.05, "{coherence}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_notch_on_the_reference_lane_stays_out_of_the_fit() {
        let truth = (0.3, -50.0, -1.0);
        let source = Gaussian::new(14).block(SOLVE_LEN);
        let mut lanes = lanes_from(&source, &[truth], 0.03, 15);
        let notched = shaped(&source, |nu| {
            if (0.1..=0.2).contains(&nu) {
                Complex::default()
            } else {
                Complex::new(1.0, 0.0)
            }
        });
        lanes[0] = with_noise(&notched, 0.03, &mut Gaussian::new(16));
        let solution = solve(&lanes, 0.8, 0.9).unwrap();
        assert_close(&solution.lanes[1], truth);
    }

    #[test]
    fn phase_slope_fit_unwraps_across_bins() {
        for truth in [
            (0.49, 179.0, 0.0),
            (-0.49, -179.0, -1.0),
            (1.49, 175.0, 0.0),
        ] {
            let source = Gaussian::new(12).block(SOLVE_LEN);
            let lanes = lanes_from(&source, &[truth], 0.03, 13);
            let solution = solve(&lanes, 0.8, 0.9).unwrap();
            assert_close(&solution.lanes[1], truth);
        }
    }

    #[test]
    fn a_short_capture_or_a_wrong_lane_count_is_refused() {
        let lanes = [
            vec![Complex::new(1.0, 0.0); 100],
            vec![Complex::new(1.0, 0.0); 100],
        ];
        assert_eq!(solve(&lanes, 0.8, 0.9), Err(BinError::Short));
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        assert_eq!(
            BinSolver::new(3, 64).solve(&views, 0.8, 0.9),
            Err(BinError::LaneCount {
                expected: 3,
                got: 2
            })
        );
        let silent = [
            vec![Complex::default(); 4096],
            vec![Complex::default(); 4096],
        ];
        assert_eq!(solve(&silent, 0.8, 0.9), Err(BinError::FewBins));
    }

    #[test]
    fn equaliser_points_run_from_minus_half_to_half() {
        let equaliser: Vec<Complex<f32>> = (0..4)
            .map(|point| Complex::new(point as f32, 0.0))
            .collect();
        assert!((equaliser_frequency(0, 4) + 0.5).abs() < f32::EPSILON);
        assert!(equaliser_frequency(2, 4).abs() < f32::EPSILON);
        assert!((equaliser_at(&equaliser, 0.0).re - 2.0).abs() < 1e-6);
        assert!((equaliser_at(&equaliser, 0.125).re - 2.5).abs() < 1e-6);
        assert!((equaliser_at(&equaliser, 0.375).re - 1.5).abs() < 1e-6);
        assert!((equaliser_at(&[], 0.1) - Complex::new(1.0, 0.0)).norm() < f32::EPSILON);
    }
}
