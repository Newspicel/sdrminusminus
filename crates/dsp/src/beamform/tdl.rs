use num_complex::Complex;

use super::weights::check_references;
use super::{BeamError, ZERO, equal_len};
use crate::linalg::MAX_ORDER;

pub const TDL_MAX_TAPS: usize = 32;
pub const TDL_MAX_CMAC_PER_S: f64 = 2.0e8;

const POWER_EMA: f32 = 1e-3;
const REGULARISE: f32 = 1e-6;
const RLS_SEED: f32 = 1e-2;
const SYMMETRISE_EVERY: u32 = 4096;
const MAX_STEP: f32 = 2.0;
const DIVERGE_NORM: f32 = 1e6;
const SUPPRESSION_CAP_DB: f32 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Adaptation {
    Nlms { step: f32 },
    Rls { forget: f32 },
}

impl Adaptation {
    fn check(self) -> Result<(), BeamError> {
        match self {
            Self::Nlms { step } if step.is_finite() && step > 0.0 && step < MAX_STEP => Ok(()),
            Self::Rls { forget } if forget.is_finite() && forget > 0.0 && forget <= 1.0 => Ok(()),
            Self::Nlms { .. } => Err(BeamError::Setting("step")),
            Self::Rls { .. } => Err(BeamError::Setting("forget")),
        }
    }

    const fn is_rls(self) -> bool {
        matches!(self, Self::Rls { .. })
    }
}

#[must_use]
pub fn tdl_cmac_per_sample(references: usize, taps: usize, adaptation: Adaptation) -> f64 {
    let m = (references * taps) as f64;
    match adaptation {
        Adaptation::Nlms { .. } => 2.0 * m + 2.0,
        Adaptation::Rls { .. } => 3.0 * m * m + 4.0 * m,
    }
}

pub struct TdlCanceller {
    primary: usize,
    references: [usize; MAX_ORDER],
    refs: usize,
    taps: usize,
    delay: usize,
    history: Vec<Complex<f32>>,
    cursor: usize,
    primary_delay: Vec<Complex<f32>>,
    primary_cursor: usize,
    regressor: Vec<Complex<f32>>,
    h: Vec<Complex<f32>>,
    p: Vec<Complex<f32>>,
    gain: Vec<Complex<f32>>,
    adaptation: Adaptation,
    primed: bool,
    since_symmetrise: u32,
    power_in: f32,
    power_out: f32,
    power_ref: f32,
    resets: u32,
}

impl TdlCanceller {
    pub fn new(
        primary: usize,
        references: &[usize],
        taps: usize,
        adaptation: Adaptation,
    ) -> Result<Self, BeamError> {
        check_references(MAX_ORDER, primary, references)?;
        if !(1..=TDL_MAX_TAPS).contains(&taps) {
            return Err(BeamError::Setting("taps"));
        }
        adaptation.check()?;
        let refs = references.len();
        let m = refs * taps;
        let delay = taps / 2;
        let mut lanes = [0; MAX_ORDER];
        lanes[..refs].copy_from_slice(references);
        Ok(Self {
            primary,
            references: lanes,
            refs,
            taps,
            delay,
            history: vec![ZERO; refs * 2 * taps],
            cursor: 0,
            primary_delay: vec![ZERO; delay.max(1)],
            primary_cursor: 0,
            regressor: vec![ZERO; m],
            h: vec![ZERO; m],
            p: vec![ZERO; m * m],
            gain: vec![ZERO; m],
            adaptation,
            primed: false,
            since_symmetrise: 0,
            power_in: 0.0,
            power_out: 0.0,
            power_ref: 0.0,
            resets: 0,
        })
    }

    pub fn set_adaptation(&mut self, adaptation: Adaptation) -> Result<(), BeamError> {
        adaptation.check()?;
        if adaptation.is_rls() != self.adaptation.is_rls() {
            self.primed = false;
        }
        self.adaptation = adaptation;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.history.fill(ZERO);
        self.primary_delay.fill(ZERO);
        self.h.fill(ZERO);
        self.cursor = 0;
        self.primary_cursor = 0;
        self.primed = false;
        self.since_symmetrise = 0;
        self.power_in = 0.0;
        self.power_out = 0.0;
        self.power_ref = 0.0;
    }

    pub fn process(
        &mut self,
        lanes: &[&[Complex<f32>]],
        out: &mut Vec<Complex<f32>>,
    ) -> Result<(), BeamError> {
        let len = self.block_len(lanes)?;
        if !self.primed {
            self.prime(lanes, len);
        }
        let mut diverged = false;
        for t in 0..len {
            match self.sample(lanes, t) {
                Some(y) => out.push(y),
                None => {
                    out.push(ZERO);
                    self.reset();
                    if !diverged {
                        diverged = true;
                        self.resets += 1;
                    }
                }
            }
        }
        if diverged {
            Err(BeamError::Diverged)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn suppression_db(&self) -> f32 {
        if self.power_out > 0.0 {
            (10.0 * (self.power_in / self.power_out).log10()).min(SUPPRESSION_CAP_DB)
        } else if self.power_in > 0.0 {
            SUPPRESSION_CAP_DB
        } else {
            0.0
        }
    }

    #[must_use]
    pub const fn resets(&self) -> u32 {
        self.resets
    }

    #[must_use]
    pub const fn delay(&self) -> usize {
        self.delay
    }

    fn block_len(&self, lanes: &[&[Complex<f32>]]) -> Result<usize, BeamError> {
        let highest = self.references[..self.refs]
            .iter()
            .copied()
            .fold(self.primary, usize::max);
        if lanes.len() <= highest {
            return Err(BeamError::Lanes(lanes.len()));
        }
        equal_len(self.used_lanes().map(|lane| lanes[lane].len()))
    }

    fn used_lanes(&self) -> impl Iterator<Item = usize> + '_ {
        std::iter::once(self.primary).chain(self.references[..self.refs].iter().copied())
    }

    fn prime(&mut self, lanes: &[&[Complex<f32>]], len: usize) {
        if !self.adaptation.is_rls() || len == 0 {
            return;
        }
        let mut sum = 0.0f64;
        for &lane in &self.references[..self.refs] {
            sum += lanes[lane][..len]
                .iter()
                .filter(|value| value.is_finite())
                .map(|value| f64::from(value.norm_sqr()))
                .sum::<f64>();
        }
        let power = (sum / (len * self.refs) as f64) as f32;
        if !(power.is_finite() && power > 0.0) {
            return;
        }
        let m = self.h.len();
        let seed = (RLS_SEED * power).recip();
        self.p.fill(ZERO);
        for i in 0..m {
            self.p[i * m + i] = Complex::new(seed, 0.0);
        }
        self.since_symmetrise = 0;
        self.primed = true;
    }

    fn sample(&mut self, lanes: &[&[Complex<f32>]], t: usize) -> Option<Complex<f32>> {
        let taps = self.taps;
        for (k, &lane) in self.references[..self.refs].iter().enumerate() {
            let value = lanes[lane][t];
            let ring = &mut self.history[k * 2 * taps..(k + 1) * 2 * taps];
            ring[self.cursor] = value;
            ring[self.cursor + taps] = value;
        }
        self.cursor = (self.cursor + 1) % taps;
        for k in 0..self.refs {
            let start = k * 2 * taps + self.cursor;
            self.regressor[k * taps..(k + 1) * taps]
                .copy_from_slice(&self.history[start..start + taps]);
        }
        let d = self.delayed_primary(lanes[self.primary][t]);
        let estimate: Complex<f32> = self
            .h
            .iter()
            .zip(&self.regressor)
            .map(|(h, u)| h.conj() * u)
            .sum();
        let y = d - estimate;
        if !y.is_finite() {
            return None;
        }
        let adapted = match self.adaptation {
            Adaptation::Nlms { step } => self.nlms(step, y),
            Adaptation::Rls { forget } => self.rls(forget, y),
        };
        if !adapted {
            return None;
        }
        self.power_in += POWER_EMA * (d.norm_sqr() - self.power_in);
        self.power_out += POWER_EMA * (y.norm_sqr() - self.power_out);
        Some(y)
    }

    fn delayed_primary(&mut self, value: Complex<f32>) -> Complex<f32> {
        if self.delay == 0 {
            return value;
        }
        let out = self.primary_delay[self.primary_cursor];
        self.primary_delay[self.primary_cursor] = value;
        self.primary_cursor = (self.primary_cursor + 1) % self.delay;
        out
    }

    fn nlms(&mut self, step: f32, y: Complex<f32>) -> bool {
        let energy: f32 = self.regressor.iter().map(Complex::norm_sqr).sum();
        if !energy.is_finite() {
            return false;
        }
        self.power_ref += POWER_EMA * (energy - self.power_ref);
        let denominator = REGULARISE * self.power_ref + energy;
        if denominator <= 0.0 {
            return true;
        }
        let gain = y.conj() * (step / denominator);
        let mut norm = 0.0f32;
        for (h, u) in self.h.iter_mut().zip(&self.regressor) {
            *h += u * gain;
            norm += h.norm_sqr();
        }
        norm.is_finite() && norm <= DIVERGE_NORM * DIVERGE_NORM
    }

    fn rls(&mut self, forget: f32, y: Complex<f32>) -> bool {
        if !self.primed {
            return true;
        }
        let m = self.h.len();
        let mut quad = 0.0f32;
        for (row, pi) in self.gain.iter_mut().enumerate() {
            let line = &self.p[row * m..(row + 1) * m];
            *pi = line.iter().zip(&self.regressor).map(|(p, u)| p * u).sum();
        }
        for (u, pi) in self.regressor.iter().zip(&self.gain) {
            quad += (u.conj() * pi).re;
        }
        let denominator = forget + quad;
        if !(denominator.is_finite() && denominator > 0.0) {
            return false;
        }
        let inverse_forget = forget.recip();
        for row in 0..m {
            let k = self.gain[row] / denominator;
            let line = &mut self.p[row * m..(row + 1) * m];
            for (p, pi) in line.iter_mut().zip(&self.gain) {
                *p = (*p - k * pi.conj()) * inverse_forget;
            }
        }
        for (h, pi) in self.h.iter_mut().zip(&self.gain) {
            *h += pi * (y.conj() / denominator);
        }
        self.since_symmetrise += 1;
        if self.since_symmetrise >= SYMMETRISE_EVERY {
            self.since_symmetrise = 0;
            return self.symmetrise();
        }
        true
    }

    fn symmetrise(&mut self) -> bool {
        let m = self.h.len();
        for row in 0..m {
            let diagonal = self.p[row * m + row];
            if !(diagonal.re.is_finite() && diagonal.re > 0.0) {
                return false;
            }
            self.p[row * m + row] = Complex::new(diagonal.re, 0.0);
            for col in row + 1..m {
                let mean = (self.p[row * m + col] + self.p[col * m + row].conj()) * 0.5;
                if !mean.is_finite() {
                    return false;
                }
                self.p[row * m + col] = mean;
                self.p[col * m + row] = mean.conj();
            }
        }
        self.h.iter().all(|value| value.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::testing::{FREQ, covariance, db, power, views};
    use crate::beamform::{BlockSolver, WeightSet};
    use crate::manifold::{Direction, Geometry};
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};

    const LEN: usize = 60_000;

    fn delayed_interferer(len: usize) -> Vec<Vec<Complex<f32>>> {
        let geometry = Geometry::ula(0.3, 2, 90.0).unwrap();
        let mut scene = ArrayScene::new(geometry, FREQ, 1e6)
            .with_seed(4)
            .with_noise_db(-10.0)
            .with_source(SceneSource::new(
                Direction::horizon(20.0),
                30.0,
                SceneSignal::Noise {
                    offset_hz: 0.0,
                    bandwidth_hz: 5e5,
                },
            ));
        scene.lane_delay_samples = vec![0.0, 2.3];
        scene.render(len).unwrap()
    }

    fn run(canceller: &mut TdlCanceller, lanes: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
        let mut out = Vec::with_capacity(lanes[0].len());
        for start in (0..lanes[0].len()).step_by(4_096) {
            let end = (start + 4_096).min(lanes[0].len());
            let block: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[start..end]).collect();
            canceller.process(&block, &mut out).unwrap();
        }
        out
    }

    fn first_window_reaching(
        out: &[Complex<f32>],
        primary: &[Complex<f32>],
        delay: usize,
        target_db: f32,
    ) -> Option<usize> {
        let window = 64;
        (delay..out.len() - window)
            .step_by(window / 2)
            .find(|&start| {
                let before = power(&primary[start - delay..start - delay + window]);
                let after = power(&out[start..start + window]);
                db(before / after) >= target_db
            })
    }

    #[test]
    fn tdl_nlms_cancels_a_delayed_wideband_interferer() {
        let lanes = delayed_interferer(LEN);
        let mut canceller =
            TdlCanceller::new(0, &[1], 16, Adaptation::Nlms { step: 0.05 }).unwrap();
        run(&mut canceller, &lanes);
        let suppression = canceller.suppression_db();
        assert!(suppression > 25.0, "tdl {suppression} dB");
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(2).unwrap();
        let mut weights = WeightSet::zeros(2);
        solver.slc(&r, 0, &[1], &mut weights).unwrap();
        let mut textbook = [Complex::new(0.0f32, 0.0); 2];
        weights.textbook(&mut textbook);
        let one_tap = db(r.get(0, 0).re / r.quad(&textbook));
        assert!(one_tap < 10.0, "one tap {one_tap} dB");
        assert_eq!(canceller.resets(), 0);
    }

    #[test]
    fn rls_converges_faster_than_nlms() {
        let lanes = delayed_interferer(20_000);
        let mut nlms = TdlCanceller::new(0, &[1], 8, Adaptation::Nlms { step: 0.05 }).unwrap();
        let mut rls = TdlCanceller::new(0, &[1], 8, Adaptation::Rls { forget: 0.999 }).unwrap();
        let slow = run(&mut nlms, &lanes);
        let fast = run(&mut rls, &lanes);
        let delay = rls.delay();
        let slow_at = first_window_reaching(&slow, &lanes[0], delay, 20.0);
        let fast_at = first_window_reaching(&fast, &lanes[0], delay, 20.0);
        let fast_at = fast_at.unwrap();
        assert!(
            slow_at.is_none_or(|slow| fast_at < slow),
            "{fast_at} vs {slow_at:?}"
        );
        assert!(rls.suppression_db() > 20.0, "{}", rls.suppression_db());
        assert_eq!(rls.resets(), 0);
    }

    #[test]
    fn rls_reset_is_counted() {
        let mut lanes = delayed_interferer(12_000);
        let mut rls = TdlCanceller::new(0, &[1], 4, Adaptation::Rls { forget: 0.999 }).unwrap();
        let mut out = Vec::with_capacity(12_000);
        rls.process(&views(&lanes)[..], &mut out).unwrap();
        out.clear();
        lanes[1][5_000] = Complex::new(f32::NAN, f32::NAN);
        let blocks: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[4_000..8_000]).collect();
        assert_eq!(rls.process(&blocks, &mut out), Err(BeamError::Diverged));
        assert_eq!(rls.resets(), 1);
        assert_eq!(out.len(), 4_000);
        assert!(out.iter().all(|value| value.is_finite()));
        let blocks: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[8_000..]).collect();
        assert_eq!(rls.process(&blocks, &mut out), Ok(()));
        assert_eq!(rls.resets(), 1);
    }

    #[test]
    fn settings_and_cost_are_checked() {
        let nlms = Adaptation::Nlms { step: 0.05 };
        assert_eq!(tdl_cmac_per_sample(2, 16, nlms), 66.0);
        assert_eq!(
            tdl_cmac_per_sample(1, 32, Adaptation::Rls { forget: 0.999 }),
            3.0 * 1024.0 + 128.0
        );
        assert!(
            tdl_cmac_per_sample(1, 32, Adaptation::Rls { forget: 0.999 }) * 2.4e6
                > TDL_MAX_CMAC_PER_S
        );
        assert!(matches!(
            TdlCanceller::new(0, &[0], 4, nlms),
            Err(BeamError::Lanes(_))
        ));
        assert!(matches!(
            TdlCanceller::new(0, &[], 4, nlms),
            Err(BeamError::Lanes(_))
        ));
        assert!(matches!(
            TdlCanceller::new(0, &[1], 0, nlms),
            Err(BeamError::Setting("taps"))
        ));
        assert!(matches!(
            TdlCanceller::new(0, &[1], 33, nlms),
            Err(BeamError::Setting("taps"))
        ));
        assert!(matches!(
            TdlCanceller::new(0, &[1], 4, Adaptation::Nlms { step: 2.5 }),
            Err(BeamError::Setting("step"))
        ));
        let mut canceller = TdlCanceller::new(2, &[0, 1], 1, nlms).unwrap();
        assert_eq!(
            canceller.set_adaptation(Adaptation::Rls { forget: 1.5 }),
            Err(BeamError::Setting("forget"))
        );
        let lane = vec![Complex::new(1.0f32, 0.0); 8];
        let mut out = Vec::with_capacity(16);
        assert_eq!(
            canceller.process(&[&lane, &lane], &mut out),
            Err(BeamError::Lanes(2))
        );
        assert_eq!(canceller.process(&[&lane, &lane, &lane], &mut out), Ok(()));
        assert_eq!(out.len(), 8);
        assert_eq!(
            canceller.process(&[&lane, &lane, &lane[..4]], &mut out),
            Err(BeamError::LaneLength)
        );
        assert_eq!(out.len(), 8);
    }
}
