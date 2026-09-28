mod bins;
mod cfo;
mod coarse;
mod convolve;
mod design;
mod drift;
mod eigen;
#[cfg(test)]
mod signals;

pub use bins::{
    BinError, BinSolution, BinSolver, EQ_POINTS, FIT_BAND, LaneResponse, MIN_BINS, equaliser_at,
    equaliser_frequency,
};
pub use coarse::{
    Boxcar, COARSE_FRAME, COARSE_LAGS, COARSE_PEAK_DB, COARSE_SECOND_DB, CoarseError, CoarseLag,
    CoarseSearch, MAX_CFO_HZ, coarse_decimation,
};
pub use convolve::{FastConvolver, SpectrumLength};
pub use design::design_correction;
pub use drift::{
    DRIFT_MIN_POINTS, DRIFT_MIN_SPAN_S, DRIFT_SAMPLES_PER_S, DriftClass, DriftTrack, SLIP_SAMPLES,
};
pub use eigen::{POWER_ITERATIONS, dominant};

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use num_complex::Complex;

    use super::{
        signals::{Gaussian, lane_response, shaped},
        *,
    };

    const FFT: usize = 4_096;
    const TAPS: usize = 129;
    const BETA: f32 = 8.0;

    fn corrected(
        convolver: &mut FastConvolver,
        spectrum: &[Complex<f32>],
        lane: &[Complex<f32>],
    ) -> Vec<Complex<f32>> {
        convolver.set_response(spectrum).unwrap();
        let mut out = Vec::new();
        convolver.push(lane, &mut out);
        out
    }

    #[test]
    fn a_solved_correction_puts_lanes_on_top_of_each_other() {
        let ripple = |nu: f64| 10f64.powf((TAU * 3.0 * nu).cos() / 20.0);
        let band = |nu: f64| if nu.abs() <= 0.4 { 1.0 } else { 0.0 };
        let source = Gaussian::new(50).block(65_536);
        let reference = shaped(&source, |nu| Complex::new(band(nu), 0.0));
        let lane = shaped(&source, |nu| {
            lane_response(0.37, 73f64.to_radians(), 0.75)(nu) * ripple(nu) * band(nu)
        });
        let mut solver = BinSolver::new(2, 1_024);
        let solution = solver.solve(&[&reference, &lane], 0.8, 0.9).unwrap();
        let response = &solution.lanes[1];
        let weight = Complex::from_polar(1.0 / response.gain, -response.phase_rad);
        let mut identity = Vec::new();
        design_correction(
            FFT,
            TAPS,
            BETA,
            0.0,
            Complex::new(1.0, 0.0),
            None,
            &mut identity,
        );
        let mut correction = Vec::new();
        design_correction(
            FFT,
            TAPS,
            BETA,
            response.delay_frac,
            weight,
            Some(&response.equaliser),
            &mut correction,
        );
        let mut convolver = FastConvolver::new(FFT, TAPS);
        let first = corrected(&mut convolver, &identity, &reference);
        convolver.reset();
        let second = corrected(&mut convolver, &correction, &lane);
        let settled = TAPS..first.len() - TAPS;
        let error: f64 = first[settled.clone()]
            .iter()
            .zip(&second[settled.clone()])
            .map(|(a, b)| f64::from((a - b).norm_sqr()))
            .sum();
        let power: f64 = first[settled].iter().map(|a| f64::from(a.norm_sqr())).sum();
        let residual_db = 10.0 * (error / power).log10();
        assert!(residual_db < -40.0, "{residual_db} dB");
    }
}
