use num_complex::Complex;
use sdrmm_dsp::beamform::{BeamError, BeamMetrics, LaneNoise, WeightSet, beam_metrics, pattern};
use sdrmm_dsp::linalg::MAX_ORDER;
use sdrmm_dsp::manifold::Direction;
use sdrmm_dsp::special::{norm_deg, wrap_deg};
use sdrmm_wire::processor::beamformer::MAX_BEAM_NULLS;
use sdrmm_wire::{BeamMode, NoiseModel, SteerSource};

use super::{Beamformer, Path, steered};
use crate::array_processor::{ArrayBlock, Steer};

const ONE: Complex<f32> = Complex::new(1.0, 0.0);
const ZERO: Complex<f32> = Complex::new(0.0, 0.0);
const NANOS_PER_MILLI: u64 = 1_000_000;
const GSC_REFRESH_DEG: f64 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Resolved {
    pub(super) steer: Option<Direction>,
    pub(super) age_ms: Option<u32>,
    pub(super) nulls: [f64; MAX_BEAM_NULLS],
    pub(super) null_count: usize,
    pub(super) placed: [f64; MAX_BEAM_NULLS],
    pub(super) placed_count: usize,
}

impl Resolved {
    fn push_null(&mut self, relative_deg: f64, cap: usize) {
        if self.null_count < cap {
            self.nulls[self.null_count] = norm_deg(relative_deg);
            self.null_count += 1;
        }
    }
}

fn relative_of(
    steer: &Steer,
    true_deg: Option<f64>,
    relative_deg: f64,
    heading: Option<f64>,
) -> Option<f64> {
    if steer.same_array {
        Some(relative_deg)
    } else {
        true_deg
            .zip(heading)
            .map(|(bearing, heading)| bearing - heading)
    }
}

impl Beamformer {
    pub(super) fn update(&mut self, lanes: &[&[Complex<f32>]], block: &ArrayBlock<'_>) {
        self.heading = block.pose.heading_deg;
        if self.noise.push(lanes).is_err() {
            self.faults.solver_failures += 1;
        }
        self.flags.band_full = self.noise.band_full();
        self.filled = self.covariance.matrix(&mut self.matrix);
        self.resolve(block.unix_ns / NANOS_PER_MILLI);
        self.flags.singular = false;
        if self.filled && !self.flags.steer_stale && self.solve().is_err() {
            self.flags.singular = true;
            self.faults.solver_failures += 1;
        }
        self.readouts();
        self.covariance.decay(self.settings.carry_over);
    }

    fn resolve(&mut self, wall_ms: u64) {
        let needs_steer = steered(self.settings.mode);
        self.flags.steer_stale = false;
        let held = self.resolved.steer;
        self.resolved.age_ms = None;
        self.resolved.null_count = 0;
        let cap = MAX_BEAM_NULLS.min(self.lanes.saturating_sub(2));
        for k in 0..self.settings.null_count {
            self.resolved.push_null(self.settings.nulls[k], cap);
        }
        self.resolved.steer = match self.settings.steer {
            SteerSource::Fixed {
                azimuth_deg,
                elevation_deg,
            } => Some(Direction::new(norm_deg(azimuth_deg), elevation_deg)),
            SteerSource::Wired => self.wired(wall_ms, cap, held),
        };
        self.flags.no_steer = needs_steer && self.resolved.steer.is_none();
        self.flags.steer_stale &= needs_steer;
    }

    fn wired(&mut self, wall_ms: u64, cap: usize, held: Option<Direction>) -> Option<Direction> {
        let steer = self.steer?;
        let age = wall_ms.saturating_sub(steer.wall_ms);
        self.resolved.age_ms = Some(u32::try_from(age).unwrap_or(u32::MAX));
        if age > u64::from(self.settings.steer_timeout_ms) {
            self.flags.steer_stale = true;
            return held;
        }
        let relative = relative_of(&steer, steer.true_deg, steer.relative_deg, self.heading)?;
        if self.settings.auto_nulls {
            for k in 0..usize::from(steer.others).min(steer.others_relative_deg.len()) {
                let other = relative_of(
                    &steer,
                    steer.others_true_deg[k],
                    steer.others_relative_deg[k],
                    self.heading,
                );
                if let Some(other) = other {
                    self.resolved.push_null(other, cap);
                }
            }
        }
        Some(Direction::new(norm_deg(relative), steer.elevation_deg))
    }

    fn solve(&mut self) -> Result<(), BeamError> {
        let mode = self.settings.mode;
        let aimed = self.resolved.steer.filter(|_| steered(mode));
        if aimed.is_none() || !matches!(mode, BeamMode::Lcmv | BeamMode::Gsc) {
            self.constraints.clear();
            self.resolved.placed_count = 0;
        }
        let Some(direction) = aimed else {
            self.loading_used = 0.0;
            if steered(mode) {
                self.gsc_for = None;
                self.retarget(WeightSet::unit(self.lanes, self.settings.main));
                return Ok(());
            }
            return self.solve_unsteered();
        };
        let n = self.lanes;
        let mut steer = [ZERO; MAX_ORDER];
        self.steering(direction, &mut steer[..n])?;
        let mut next = self.target.clone();
        match mode {
            BeamMode::Das => {
                self.solver.das(&steer[..n], &mut next)?;
                self.loading_used = 0.0;
            }
            BeamMode::Mvdr => {
                self.loading_used = self.solver.mvdr(
                    &self.matrix,
                    &steer[..n],
                    self.settings.loading,
                    &mut next,
                )?;
            }
            BeamMode::Lcmv => {
                self.constrain(direction)?;
                self.loading_used = self.solver.lcmv(
                    Some(&self.matrix),
                    &self.constraints,
                    self.settings.loading,
                    &mut next,
                )?;
            }
            _ => {
                self.constrain(direction)?;
                return self.refresh_gsc(direction);
            }
        }
        self.retarget(next);
        Ok(())
    }

    fn solve_unsteered(&mut self) -> Result<(), BeamError> {
        let mut next = self.target.clone();
        match self.settings.mode {
            BeamMode::Canceller if self.settings.wideband() => return Ok(()),
            BeamMode::Canceller => {
                self.loading_used = self.solver.slc(
                    &self.matrix,
                    self.settings.main,
                    self.settings.references(),
                    &mut next,
                )?;
            }
            BeamMode::Cma if self.cma_seeded => return Ok(()),
            BeamMode::Cma => {
                self.solver.mrc(
                    &self.matrix,
                    mrc_noise(&self.noise, self.settings.noise),
                    &mut next,
                )?;
                self.cma.seed(&next)?;
                self.cma_seeded = true;
            }
            _ => self.solver.mrc(
                &self.matrix,
                mrc_noise(&self.noise, self.settings.noise),
                &mut next,
            )?,
        }
        self.retarget(next);
        Ok(())
    }

    fn retarget(&mut self, mut next: WeightSet) {
        next.align_phase_to(&self.target);
        self.target = next;
        let ramp = self.crossfade_samples();
        self.ramp.set_target(&self.target, ramp);
    }

    fn steering(&self, direction: Direction, out: &mut [Complex<f32>]) -> Result<(), BeamError> {
        let manifold = self.manifold.as_ref().ok_or(BeamError::Lanes(0))?;
        manifold.steer(self.freq_hz, direction, out);
        Ok(())
    }

    fn constrain(&mut self, direction: Direction) -> Result<(), BeamError> {
        let n = self.lanes;
        let mut vector = [ZERO; MAX_ORDER];
        self.constraints.clear();
        self.resolved.placed_count = 0;
        self.steering(direction, &mut vector[..n])?;
        self.constraints.push(&vector[..n], ONE)?;
        for k in 0..self.resolved.null_count {
            let null = self.resolved.nulls[k];
            self.steering(Direction::horizon(null), &mut vector[..n])?;
            match self.constraints.push(&vector[..n], ZERO) {
                Ok(()) => {
                    self.resolved.placed[self.resolved.placed_count] = null;
                    self.resolved.placed_count += 1;
                }
                Err(BeamError::NullInMainLobe(_)) => {}
                Err(BeamError::TooManyConstraints(_)) => break,
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    fn refresh_gsc(&mut self, direction: Direction) -> Result<(), BeamError> {
        let mut angles = [0.0; MAX_BEAM_NULLS + 1];
        angles[0] = direction.azimuth_deg;
        let count = self.resolved.placed_count;
        angles[1..=count].copy_from_slice(&self.resolved.placed[..count]);
        let moved = self.gsc_for.is_none_or(|(previous, placed)| {
            placed != count
                || previous[..=count]
                    .iter()
                    .zip(&angles[..=count])
                    .any(|(old, new)| wrap_deg(old - new).abs() > GSC_REFRESH_DEG)
        });
        if moved {
            self.gsc.set_constraints(&self.constraints, &mut self.qr)?;
            self.gsc_for = Some((angles, count));
        }
        Ok(())
    }

    fn readouts(&mut self) {
        let path = self.path();
        match path {
            Path::Gsc => self.gsc.weights(&mut self.effective),
            Path::Cma => self.cma.weights(&mut self.effective),
            Path::Tdl => self.effective = WeightSet::zeros(self.lanes),
            Path::Ramp => self.effective.clone_from(&self.target),
        }
        self.beam_power = mean_power(&self.beam);
        self.metrics = if path == Path::Tdl || !self.filled {
            BeamMetrics::default()
        } else {
            let primary =
                matches!(self.settings.mode, BeamMode::Canceller).then_some(self.settings.main);
            beam_metrics(
                &self.matrix,
                &self.effective,
                self.noise.noise(),
                primary,
                &self.constraints,
            )
            .unwrap_or_else(|_| {
                self.faults.solver_failures += 1;
                BeamMetrics::default()
            })
        };
        self.has_pattern = path != Path::Tdl
            && match (self.manifold.as_ref(), self.ring.as_ref()) {
                (Some(manifold), Some(ring)) => pattern(
                    manifold,
                    self.freq_hz,
                    &self.effective,
                    ring,
                    &mut self.pattern,
                )
                .is_ok(),
                _ => false,
            };
    }
}

fn mrc_noise(noise: &LaneNoise, model: NoiseModel) -> Option<&[f32]> {
    match model {
        NoiseModel::Measured => noise.noise(),
        NoiseModel::White => None,
    }
}

fn mean_power(samples: &[Complex<f32>]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(Complex::norm_sqr).sum::<f32>() / samples.len() as f32
}
