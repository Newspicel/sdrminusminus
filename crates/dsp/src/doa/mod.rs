mod order;
mod peaks;
mod quality;
mod space;
mod spectrum;
mod subspace;
#[cfg(test)]
mod tests;

use num_complex::Complex;

use crate::covariance::{CovarianceError, PhaseMode};
use crate::linalg::{
    CMat, Cholesky, Eigen, HermitianEigen, LinalgError, MAX_ORDER, MAX_POLY_DEGREE, Roots,
};
use crate::manifold::{
    AliasReport, AzimuthSpan, Direction, ElevationSpan, GridSpec, MAX_ELEMENTS, Manifold,
    ManifoldError, Shape, SteeringGrid, alias_check, wavenumber,
};
use crate::special::{brent_max, norm_deg};

pub use order::{SourceCounter, Squelch, dominance_count, mdl_count};
pub use peaks::{
    Candidate, GridShape, MAX_CANDIDATES, apart_deg, local_maxima, mirror_deg, on_side, refine,
};
pub use quality::{
    MAX_SIGMA_DEG, confidence, crb_sigma_rad, joint_fit, model_sigma_rad, quantize_likelihood,
    rate_spread, temper_likelihood, total_sigma_deg,
};
pub use space::{
    ARRAY_CHANGED, ELEVATION_NEEDS_2D, ELEVATION_NEEDS_GRID, FB_NEEDS_SYMMETRY,
    LOADING_OUT_OF_RANGE, NEEDS_LINE_OR_CIRCLE, PEAK_RANGE_OUT_OF_RANGE, PEAKS_OUT_OF_RANGE,
    SIDE_NEEDS_LINE, SQUELCH_OUT_OF_RANGE, TOO_FEW_ELEMENTS, TOO_MANY_SOURCES,
};
pub use spectrum::{Pseudospectrum, bartlett, capon, music};
pub use subspace::{EspritSolvers, MAX_ESPRIT_SOURCES, esprit, line_azimuths, root_music};

use space::{Layout, Space, Workspace};

pub const MAX_PEAKS: usize = 4;
pub const LIKELIHOOD_POINTS: usize = 360;
pub const LIKELIHOOD_FLOOR: f32 = 0.02;
pub const CONFIDENCE_WINDOW_DEG: f64 = 5.0;
pub const SIGMA_FLOOR_DEG: f64 = 2.0;
pub const DEFAULT_CAL_SIGMA_DEG: f32 = 3.0;
pub const ANTENNA_PHASE_SIGMA_DEG: f64 = 5.0;
pub const MEASURED_PHASE_SIGMA_DEG: f64 = 2.0;
pub const NOISE_ABOVE_DB: f32 = 6.0;
pub const DOMINANT_WITHIN_DB: f32 = 12.0;
pub const STABLE_REPORTS: u8 = 3;

const RING_STEP_DEG: f64 = 1.0;
const MIRROR_APART_DEG: f64 = 1.0;
const ROOT_RADIUS_SLACK: f64 = 0.1;
const LIKELIHOOD_WINDOW_DEG: f64 = 10.0;
const LIKELIHOOD_COARSE_DEG: f64 = 0.5;
const CURVATURE_STEP_DEG: f64 = 0.25;
const EIGEN_FLOOR: f32 = 1e-12;
const DETECTION_SIGMAS: f64 = 3.0;
const MIN_SHARE: f64 = 1e-12;
const MIN_SNR_DB: f32 = -60.0;

type C32 = Complex<f32>;
type C64 = Complex<f64>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Estimator {
    Bartlett,
    Capon,
    #[default]
    Music,
    RootMusic,
    Esprit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceCount {
    #[default]
    Auto,
    Fixed(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OrderRule {
    #[default]
    Dominance,
    Mdl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UlaSide {
    #[default]
    Both,
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SquelchConfig {
    pub open_db: f32,
    pub hysteresis_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoaConfig {
    pub estimator: Estimator,
    pub sources: SourceCount,
    pub rule: OrderRule,
    pub max_peaks: u8,
    pub forward_backward: bool,
    pub smoothing: u8,
    pub loading: f32,
    pub azimuth_step_deg: f64,
    pub elevation: Option<ElevationSpan>,
    pub ula_side: UlaSide,
    pub squelch: Option<SquelchConfig>,
    pub peak_range_db: f32,
}

impl Default for DoaConfig {
    fn default() -> Self {
        Self {
            estimator: Estimator::Music,
            sources: SourceCount::Auto,
            rule: OrderRule::Dominance,
            max_peaks: 2,
            forward_backward: false,
            smoothing: 0,
            loading: 1e-3,
            azimuth_step_deg: 1.0,
            elevation: None,
            ula_side: UlaSide::Both,
            squelch: Some(SquelchConfig {
                open_db: 6.0,
                hysteresis_db: 1.0,
            }),
            peak_range_db: 20.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Peak {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub power: f32,
    pub sigma_deg: f32,
    pub sigma_el_deg: f32,
    pub confidence: f32,
    pub fit: f32,
    pub ambiguity: f32,
    pub mirror_deg: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoaReport {
    pub peaks: [Peak; MAX_PEAKS],
    pub peak_count: usize,
    pub sources: usize,
    pub sources_raw: usize,
    pub squelch_open: bool,
    pub eigenvalues: [f32; MAX_ORDER],
    pub order: usize,
    pub eig_ratio_db: f32,
    pub lambda12_db: f32,
    pub snr_db: f32,
    pub noise_power: f32,
    pub snapshots: f64,
    pub fit: f32,
    pub loading_used: f32,
}

impl DoaReport {
    #[must_use]
    pub fn peaks(&self) -> &[Peak] {
        &self.peaks[..self.peak_count.min(MAX_PEAKS)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconfigure {
    InPlace,
    NeedsNew,
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum DoaError {
    #[error(transparent)]
    Linalg(#[from] LinalgError),
    #[error(transparent)]
    Manifold(#[from] ManifoldError),
    #[error(transparent)]
    Covariance(#[from] CovarianceError),
    #[error("{0}")]
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Found {
    azimuth_deg: f64,
    elevation_deg: f64,
    fit_scale: f32,
    power: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Stats {
    order: usize,
    sources: usize,
    noise: f32,
    snapshots: f64,
}

struct Solvers {
    eigen: HermitianEigen,
    decomposition: Eigen,
    loaded: CMat,
    projector: CMat,
    working_chol: Cholesky,
    element_chol: Cholesky,
    fit_chol: Cholesky,
    roots: Roots,
    esprit: EspritSolvers,
    coeffs: [C64; MAX_POLY_DEGREE + 1],
    found: [C64; MAX_POLY_DEGREE],
}

impl Solvers {
    fn new(elements: usize) -> Result<Self, LinalgError> {
        Ok(Self {
            eigen: HermitianEigen::new(elements)?,
            decomposition: Eigen::new(),
            loaded: CMat::identity(elements)?,
            projector: CMat::identity(elements)?,
            working_chol: Cholesky::new(MAX_ORDER)?,
            element_chol: Cholesky::new(MAX_ORDER)?,
            fit_chol: Cholesky::new(MAX_ORDER)?,
            roots: Roots::new(MAX_POLY_DEGREE)?,
            esprit: EspritSolvers::new()?,
            coeffs: [C64::new(0.0, 0.0); MAX_POLY_DEGREE + 1],
            found: [C64::new(0.0, 0.0); MAX_POLY_DEGREE],
        })
    }
}

pub struct Doa {
    config: DoaConfig,
    freq_hz: f64,
    manifold: Manifold,
    layout: Layout,
    phase: Option<PhaseMode>,
    plan: Result<Space, DoaError>,
    grid: SteeringGrid,
    ring: SteeringGrid,
    alias: AliasReport,
    workspace: Workspace,
    solvers: Solvers,
    counter: SourceCounter,
    squelch: Squelch,
    spectrum: Vec<f32>,
}

impl Doa {
    pub fn new(manifold: &Manifold, config: &DoaConfig, freq_hz: f64) -> Result<Self, DoaError> {
        if !(freq_hz.is_finite() && freq_hz > 0.0) {
            return Err(ManifoldError::Frequency.into());
        }
        let geometry = manifold.geometry();
        let layout = Layout::of(geometry);
        let phase = match geometry.shape() {
            Shape::Uca { .. } => match PhaseMode::new(geometry, freq_hz) {
                Ok(phase) => Some(phase),
                Err(CovarianceError::NotStructured) => None,
                Err(other) => return Err(other.into()),
            },
            Shape::Ula { .. } | Shape::Explicit => None,
        };
        let plan = space::plan(config, &layout, phase.as_ref())?;
        let grid = SteeringGrid::new(manifold, layout.grid_spec(config), freq_hz)?;
        let ring = SteeringGrid::new(manifold, GridSpec::ring(RING_STEP_DEG), freq_hz)?;
        let alias = alias_check(&ring, geometry, freq_hz);
        let elements = geometry.len();
        Ok(Self {
            config: *config,
            freq_hz,
            spectrum: vec![0.0; grid.points()],
            manifold: manifold.clone(),
            layout,
            phase,
            plan: Ok(plan),
            grid,
            ring,
            alias,
            workspace: Workspace::new(elements)?,
            solvers: Solvers::new(elements)?,
            counter: SourceCounter::new(),
            squelch: Squelch::new(),
        })
    }

    pub fn configure(
        &mut self,
        manifold: &Manifold,
        config: &DoaConfig,
    ) -> Result<Reconfigure, DoaError> {
        if !self.same_array(manifold) {
            return Ok(Reconfigure::NeedsNew);
        }
        let plan = space::plan(config, &self.layout, self.phase.as_ref())?;
        let spec = self.layout.grid_spec(config);
        let current = self.grid.spec();
        let same_storage = spec.azimuth_step_deg == current.azimuth_step_deg
            && spec.elevation == current.elevation
            && matches!(
                (spec.span, current.span),
                (AzimuthSpan::Full, AzimuthSpan::Full)
                    | (AzimuthSpan::Half { .. }, AzimuthSpan::Half { .. })
            );
        if !same_storage {
            return Ok(Reconfigure::NeedsNew);
        }
        if spec.span != current.span {
            self.grid.respan(manifold, spec.span)?;
        }
        self.config = *config;
        self.plan = Ok(plan);
        self.reset();
        Ok(Reconfigure::InPlace)
    }

    pub fn retune(&mut self, manifold: &Manifold, freq_hz: f64) -> Result<AliasReport, DoaError> {
        if !(freq_hz.is_finite() && freq_hz > 0.0) {
            return Err(ManifoldError::Frequency.into());
        }
        if !self.same_array(manifold) {
            return Err(DoaError::Unsupported(ARRAY_CHANGED));
        }
        if let Some(phase) = self.phase.as_mut() {
            phase.retune(freq_hz)?;
        }
        self.grid.rebuild(manifold, freq_hz)?;
        self.ring.rebuild(manifold, freq_hz)?;
        self.freq_hz = freq_hz;
        self.alias = alias_check(&self.ring, manifold.geometry(), freq_hz);
        self.reset();
        self.plan = space::plan(&self.config, &self.layout, self.phase.as_ref());
        self.plan.map(|_| self.alias)
    }

    pub const fn reset(&mut self) {
        self.counter.reset();
        self.squelch.reset();
    }

    #[must_use]
    pub fn spectrum(&self) -> &[f32] {
        &self.spectrum
    }

    #[must_use]
    pub const fn grid(&self) -> &SteeringGrid {
        &self.grid
    }

    #[must_use]
    pub const fn alias(&self) -> &AliasReport {
        &self.alias
    }

    #[must_use]
    pub const fn config(&self) -> &DoaConfig {
        &self.config
    }

    #[must_use]
    pub const fn freq_hz(&self) -> f64 {
        self.freq_hz
    }

    #[must_use]
    pub fn mode_bias_deg(&self) -> Option<f32> {
        let uses_modes = matches!(self.plan, Ok(Space::Modes | Space::Vandermonde { .. }));
        self.phase
            .as_ref()
            .filter(|_| uses_modes)
            .map(PhaseMode::mode_bias_deg)
    }

    pub fn estimate(
        &mut self,
        manifold: &Manifold,
        r: &CMat,
        snapshots: f64,
        cal_sigma_deg: f32,
        out: &mut DoaReport,
    ) -> Result<(), DoaError> {
        let space = self.plan?;
        self.check_input(manifold, r)?;
        self.workspace.prepare(
            space,
            self.config.forward_backward,
            &self.layout,
            self.phase.as_ref(),
            r,
        )?;
        let delta = self.decompose()?;
        *out = DoaReport {
            snapshots,
            ..DoaReport::default()
        };
        let stats = self.statistics(delta, snapshots, out);
        out.loading_used = self.fill_spectrum(manifold, space, stats)?;
        let mut found = [Found::default(); MAX_PEAKS];
        let count = if out.squelch_open {
            self.solvers
                .element_chol
                .factor_loaded(r, self.config.loading)?;
            self.find(manifold, space, stats, &mut found)?
        } else {
            0
        };
        self.assess(
            manifold,
            r,
            stats,
            f64::from(cal_sigma_deg),
            &found[..count],
            out,
        )
    }

    pub fn likelihood(
        &self,
        manifold: &Manifold,
        r: &CMat,
        report: &DoaReport,
        out: &mut [f32; LIKELIHOOD_POINTS],
    ) -> Result<bool, DoaError> {
        let Some(primary) = report.peaks().first().filter(|_| report.squelch_open) else {
            return Ok(false);
        };
        self.check_input(manifold, r)?;
        let beam = Beam {
            manifold,
            r,
            freq_hz: self.freq_hz,
            elevation_deg: primary.elevation_deg,
            ring: &self.ring,
        };
        let peak = beam.peak(primary.azimuth_deg);
        let top = beam.power(peak);
        let scale =
            report.snapshots.max(1.0) / f64::from(report.noise_power).max(f64::MIN_POSITIVE);
        let mut raw = [0.0f32; LIKELIHOOD_POINTS];
        for (degree, value) in raw.iter_mut().enumerate() {
            *value = (scale * (beam.at_degree(degree) - top)) as f32;
        }
        let h = CURVATURE_STEP_DEG.to_radians();
        let bend = beam.power(peak + CURVATURE_STEP_DEG) - 2.0 * top
            + beam.power(peak - CURVATURE_STEP_DEG);
        let curvature = -scale * bend / (h * h);
        temper_likelihood(
            &mut raw,
            curvature,
            f64::from(primary.sigma_deg).to_radians(),
        );
        *out = raw;
        Ok(true)
    }

    fn same_array(&self, manifold: &Manifold) -> bool {
        let same_table = match (manifold.table(), self.manifold.table()) {
            (None, None) => true,
            (Some(given), Some(kept)) => std::ptr::eq(given, kept),
            _ => false,
        };
        same_table && manifold.geometry() == self.manifold.geometry()
    }

    fn check_input(&self, manifold: &Manifold, r: &CMat) -> Result<(), DoaError> {
        if !self.same_array(manifold) {
            return Err(DoaError::Unsupported(ARRAY_CHANGED));
        }
        let n = self.manifold.len();
        if r.order() != n {
            return Err(LinalgError::Order(r.order()).into());
        }
        if !r.is_finite() {
            return Err(LinalgError::NonFinite.into());
        }
        let trace = r.trace_re();
        if trace.is_nan() || trace <= 0.0 {
            return Err(LinalgError::NotPositiveDefinite(0).into());
        }
        Ok(())
    }

    fn decompose(&mut self) -> Result<f32, DoaError> {
        let solvers = &mut self.solvers;
        let work = &self.workspace.work;
        let m = work.order();
        solvers.loaded.clone_from(work);
        let delta = crate::covariance::load_diagonal(&mut solvers.loaded, self.config.loading);
        if solvers.eigen.order() != m {
            solvers.eigen = HermitianEigen::new(m)?;
        }
        solvers
            .eigen
            .solve(&solvers.loaded, &mut solvers.decomposition)?;
        Ok(delta)
    }

    fn statistics(&mut self, delta: f32, snapshots: f64, out: &mut DoaReport) -> Stats {
        let m = self.workspace.order();
        let floor = EIGEN_FLOOR * self.workspace.work.trace_re();
        let values = self.solvers.decomposition.values();
        let mut desc = [0.0f32; MAX_ORDER];
        for (k, slot) in desc.iter_mut().enumerate().take(m) {
            *slot = (values[m - 1 - k] - delta).max(floor);
        }
        let desc = &desc[..m];
        let (adopted, raw) = match self.config.sources {
            SourceCount::Fixed(d) => (usize::from(d), usize::from(d)),
            SourceCount::Auto => self.counter.update(self.config.rule, desc, snapshots),
        };
        let sources = adopted.min(m - 1);
        let noise = desc[sources..].iter().sum::<f32>() / (m - sources) as f32;
        let rest = desc[1..].iter().sum::<f32>() / (m - 1) as f32;
        out.eigenvalues[..m].copy_from_slice(desc);
        out.order = m;
        out.sources = sources;
        out.sources_raw = raw.min(m - 1);
        out.noise_power = noise;
        out.eig_ratio_db = decibels(desc[0] / rest);
        out.lambda12_db = decibels(desc[0] / desc[1]);
        out.squelch_open = self.squelch.update(self.config.squelch, out.eig_ratio_db);
        Stats {
            order: m,
            sources,
            noise,
            snapshots,
        }
    }

    fn fill_spectrum(
        &mut self,
        manifold: &Manifold,
        space: Space,
        stats: Stats,
    ) -> Result<f32, DoaError> {
        let mut loading_used = self.config.loading;
        if self.config.estimator == Estimator::Capon {
            let mean = self.workspace.work.trace_re() / stats.order as f32;
            let absolute = self
                .solvers
                .working_chol
                .factor_loaded(&self.workspace.work, self.config.loading)?;
            loading_used = if mean > 0.0 { absolute / mean } else { 0.0 };
        }
        let mut spectrum = std::mem::take(&mut self.spectrum);
        let probe = self.probe(manifold, space, stats);
        for (point, value) in spectrum.iter_mut().enumerate().take(self.grid.points()) {
            *value = probe.at_point(&self.grid, point);
        }
        self.spectrum = spectrum;
        Ok(loading_used)
    }

    fn probe<'a>(&'a self, manifold: &'a Manifold, space: Space, stats: Stats) -> Probe<'a> {
        let pseudo = match self.config.estimator {
            Estimator::Bartlett => Pseudospectrum::Bartlett(&self.workspace.work),
            Estimator::Capon => Pseudospectrum::Capon(&self.solvers.working_chol),
            Estimator::Music | Estimator::RootMusic | Estimator::Esprit => Pseudospectrum::Music {
                eigen: &self.solvers.decomposition,
                signals: stats.sources.max(1),
            },
        };
        Probe {
            pseudo,
            space,
            workspace: &self.workspace,
            layout: &self.layout,
            phase: self.phase.as_ref(),
            manifold,
            freq_hz: self.freq_hz,
        }
    }

    fn wanted(&self, stats: Stats) -> usize {
        stats
            .sources
            .max(1)
            .min(usize::from(self.config.max_peaks))
            .min(MAX_PEAKS)
    }

    fn find(
        &mut self,
        manifold: &Manifold,
        space: Space,
        stats: Stats,
        out: &mut [Found; MAX_PEAKS],
    ) -> Result<usize, DoaError> {
        match self.config.estimator {
            Estimator::Bartlett | Estimator::Capon | Estimator::Music => {
                Ok(self.grid_peaks(manifold, space, stats, out))
            }
            Estimator::RootMusic | Estimator::Esprit => {
                self.subspace_peaks(manifold, space, stats, out)
            }
        }
    }

    fn grid_peaks(
        &self,
        manifold: &Manifold,
        space: Space,
        stats: Stats,
        out: &mut [Found; MAX_PEAKS],
    ) -> usize {
        let shape = GridShape {
            azimuths: self.grid.azimuths(),
            elevations: self.grid.elevations(),
            wraps: self.grid.wraps(),
        };
        let mut candidates = [Candidate::default(); MAX_CANDIDATES];
        let count = local_maxima(
            &self.spectrum,
            shape,
            self.config.peak_range_db,
            &mut candidates,
        );
        let wanted = self.wanted(stats);
        let apart = 1.5 * self.grid.azimuth_step_deg();
        let mut kept = 0;
        for candidate in &candidates[..count] {
            if kept == wanted {
                break;
            }
            let direction = self.grid.direction(candidate.point);
            let azimuth_deg = self.to_side(direction.azimuth_deg);
            let taken = out[..kept].iter().any(|found| {
                found.elevation_deg == direction.elevation_deg
                    && apart_deg(found.azimuth_deg, azimuth_deg) < apart
            });
            if !taken {
                out[kept] = Found {
                    azimuth_deg,
                    elevation_deg: direction.elevation_deg,
                    fit_scale: 1.0,
                    power: candidate.power,
                };
                kept += 1;
            }
        }
        let probe = self.probe(manifold, space, stats);
        for found in &mut out[..kept] {
            self.refine_found(&probe, found);
            found.azimuth_deg = self.to_side(found.azimuth_deg);
        }
        kept
    }

    fn refine_found(&self, probe: &Probe<'_>, found: &mut Found) {
        let step = self.grid.azimuth_step_deg();
        let along_azimuth = |found: &mut Found| {
            let elevation = found.elevation_deg;
            let (azimuth, power) = best_of(
                |azimuth| probe.power(Direction::new(azimuth, elevation)),
                (found.azimuth_deg, found.power),
                found.azimuth_deg - step,
                found.azimuth_deg + step,
            );
            found.azimuth_deg = norm_deg(azimuth);
            found.power = power;
        };
        along_azimuth(found);
        let Some(span) = self.grid.spec().elevation else {
            return;
        };
        let rows = self.grid.elevations().saturating_sub(1).max(1) as f64;
        let elevation_step = (span.max_deg - span.min_deg) / rows;
        let azimuth = found.azimuth_deg;
        let (elevation, power) = best_of(
            |elevation| probe.power(Direction::new(azimuth, elevation)),
            (found.elevation_deg, found.power),
            (found.elevation_deg - elevation_step).max(span.min_deg),
            (found.elevation_deg + elevation_step).min(span.max_deg),
        );
        found.elevation_deg = elevation;
        found.power = power;
        along_azimuth(found);
    }

    fn to_side(&self, azimuth_deg: f64) -> f64 {
        match (
            self.layout.axis_deg,
            self.layout.side_centre_deg(self.config.ula_side),
        ) {
            (Some(axis), Some(centre)) if !on_side(azimuth_deg, centre) => {
                mirror_deg(azimuth_deg, axis)
            }
            _ => norm_deg(azimuth_deg),
        }
    }

    fn mirror_of(&self, azimuth_deg: f64) -> Option<f64> {
        let axis = self.layout.axis_deg?;
        if self.config.ula_side != UlaSide::Both {
            return None;
        }
        let mirror = mirror_deg(azimuth_deg, axis);
        (apart_deg(mirror, azimuth_deg) > MIRROR_APART_DEG).then_some(mirror)
    }

    fn subspace_peaks(
        &mut self,
        manifold: &Manifold,
        space: Space,
        stats: Stats,
        out: &mut [Found; MAX_PEAKS],
    ) -> Result<usize, DoaError> {
        let mut values = [C64::new(0.0, 0.0); MAX_ORDER];
        let (count, radial) = match self.config.estimator {
            Estimator::Esprit => (self.esprit_values(space, stats, &mut values)?, false),
            _ => (self.root_values(space, stats, &mut values)?, true),
        };
        let wanted = self.wanted(stats);
        let limit = stats.sources.max(1);
        let mut chosen = [Found::default(); MAX_ORDER];
        let mut kept = 0;
        for value in &values[..count] {
            if kept == limit {
                break;
            }
            let Some(azimuth_deg) = self.value_azimuth(space, *value) else {
                continue;
            };
            let radius = value.norm();
            let fit_scale = if radial && 1.0 - radius > ROOT_RADIUS_SLACK {
                radius.clamp(0.0, 1.0) as f32
            } else {
                1.0
            };
            chosen[kept] = Found {
                azimuth_deg,
                elevation_deg: 0.0,
                fit_scale,
                power: self.capon_at(manifold, Direction::horizon(azimuth_deg)),
            };
            kept += 1;
        }
        let chosen = &mut chosen[..kept];
        chosen.sort_unstable_by(|a, b| b.power.total_cmp(&a.power));
        let taken = kept.min(wanted);
        out[..taken].copy_from_slice(&chosen[..taken]);
        Ok(taken)
    }

    fn root_values(
        &mut self,
        space: Space,
        stats: Stats,
        out: &mut [C64; MAX_ORDER],
    ) -> Result<usize, DoaError> {
        let m = stats.order;
        let signals = stats.sources.max(1).min(m - 1);
        let solvers = &mut self.solvers;
        solvers.projector.resize(m)?;
        for k in 0..m - signals {
            let e = solvers.decomposition.vector(k);
            for i in 0..m {
                for j in 0..m {
                    solvers.projector.add(i, j, e[i] * e[j].conj());
                }
            }
        }
        if let Some(weights) = self.workspace.weights(space, self.phase.as_ref()) {
            for i in 0..m {
                for j in 0..m {
                    let value = solvers.projector.get(i, j) * (weights[i] * weights[j]);
                    solvers.projector.set(i, j, value);
                }
            }
        }
        root_music(
            &solvers.projector,
            &mut solvers.roots,
            &mut solvers.coeffs,
            &mut solvers.found,
            out,
        )
    }

    fn esprit_values(
        &mut self,
        space: Space,
        stats: Stats,
        out: &mut [C64; MAX_ORDER],
    ) -> Result<usize, DoaError> {
        let m = stats.order;
        let d = stats.sources.max(1).min(m - 1).min(MAX_ESPRIT_SOURCES);
        let mut signal = [C32::new(0.0, 0.0); MAX_ORDER * MAX_ESPRIT_SOURCES];
        let mut rows = [0.0f32; MAX_ORDER];
        let whitening = self.workspace.whitening();
        let weighted = matches!(space, Space::Vandermonde { .. });
        for slot in 0..d {
            let vector = self.solvers.decomposition.vector(m - 1 - slot);
            let column = &mut signal[slot * m..(slot + 1) * m];
            for ((value, &e), &weight) in column.iter_mut().zip(vector).zip(whitening) {
                *value = if weighted && weight > 0.0 {
                    e / weight
                } else {
                    e
                };
            }
        }
        if weighted {
            for (row, value) in rows.iter_mut().enumerate().take(m - 1) {
                *value = pair_weight(whitening[row], whitening[row + 1]);
            }
        }
        let row_weights = if weighted { &rows[..m - 1] } else { &[][..] };
        esprit(
            &signal[..m * d],
            m,
            d,
            row_weights,
            &mut self.solvers.esprit,
            out,
        )
    }

    fn value_azimuth(&self, space: Space, value: C64) -> Option<f64> {
        match (space, self.layout.line) {
            (Space::Line { .. }, Some(line)) => {
                let kd = wavenumber(self.freq_hz) * line.spacing_m;
                let (first, _) = line_azimuths(value.arg(), kd, line.sort_axis_deg)?;
                Some(self.to_side(first))
            }
            (Space::Line { .. }, None) => None,
            _ => Some(norm_deg(value.arg().to_degrees())),
        }
    }

    fn capon_at(&self, manifold: &Manifold, direction: Direction) -> f32 {
        let n = self.manifold.len();
        let mut a = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        let mut scratch = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        manifold.steer(self.freq_hz, direction, &mut a[..n]);
        capon(&self.solvers.element_chol, &a[..n], &mut scratch)
    }

    fn assess(
        &mut self,
        manifold: &Manifold,
        r: &CMat,
        stats: Stats,
        cal_sigma_deg: f64,
        found: &[Found],
        out: &mut DoaReport,
    ) -> Result<(), DoaError> {
        let n = self.manifold.len();
        let mut ranked = [Found::default(); MAX_PEAKS];
        let count = found.len().min(MAX_PEAKS);
        for (slot, item) in ranked.iter_mut().zip(found) {
            *slot = Found {
                power: self.capon_at(
                    manifold,
                    Direction::new(item.azimuth_deg, item.elevation_deg),
                ),
                ..*item
            };
        }
        ranked[..count].sort_unstable_by(|a, b| b.power.total_cmp(&a.power));
        let mut steering = [C32::new(0.0, 0.0); MAX_PEAKS * MAX_ELEMENTS];
        let mut signal = 0.0f64;
        for (k, item) in ranked[..count].iter().enumerate() {
            let a = &mut steering[k * n..(k + 1) * n];
            let direction = Direction::new(item.azimuth_deg, item.elevation_deg);
            manifold.steer(self.freq_hz, direction, a);
            let (peak, share) = self.peak_quality(manifold, r, stats, cal_sigma_deg, item, a);
            out.peaks[k] = peak;
            signal += share;
        }
        out.peak_count = count;
        let mut scratch = [C32::new(0.0, 0.0); MAX_PEAKS * MAX_ELEMENTS];
        out.fit = joint_fit(
            &steering[..count * n],
            count,
            n,
            r,
            &mut self.solvers.fit_chol,
            &mut scratch,
        )?;
        let noise = f64::from(stats.noise);
        out.snr_db = if count > 0 {
            decibels((signal / noise) as f32)
        } else {
            let top = f64::from(out.eigenvalues[0]);
            decibels(((top / noise - 1.0) / stats.order as f64) as f32)
        }
        .max(MIN_SNR_DB);
        Ok(())
    }

    fn peak_quality(
        &self,
        manifold: &Manifold,
        r: &CMat,
        stats: Stats,
        cal_sigma_deg: f64,
        item: &Found,
        a: &[C32],
    ) -> (Peak, f64) {
        let n = a.len();
        let noise = f64::from(stats.noise);
        let margin = 1.0 + DETECTION_SIGMAS / stats.snapshots.max(1.0).sqrt();
        let share = (f64::from(item.power) - margin * noise / n as f64).max(MIN_SHARE * noise);
        let snr = share / noise;
        let direction = Direction::new(item.azimuth_deg, item.elevation_deg);
        let mut rates = [0.0f64; MAX_ELEMENTS];
        manifold.phase_rates(self.freq_hz, direction, &mut rates[..n]);
        let sigma = sigma_deg(&rates[..n], snr, stats.snapshots, cal_sigma_deg);
        let sigma_el_deg = if self.grid.spec().elevation.is_some() {
            manifold.elevation_rates(self.freq_hz, direction, &mut rates[..n]);
            sigma_deg(&rates[..n], snr, stats.snapshots, cal_sigma_deg)
        } else {
            0.0
        };
        let mirror_deg = self.mirror_of(item.azimuth_deg);
        let aliased = self.alias.aliased && stats.sources == 1;
        let ambiguity = if mirror_deg.is_some() || aliased {
            0.5
        } else {
            0.0
        };
        let norm = spectrum::norm_sqr(a);
        let trace = r.trace_re();
        let fit = if norm > 0.0 && trace > 0.0 {
            (r.quad(a) / (norm * trace)).clamp(0.0, 1.0) * item.fit_scale
        } else {
            0.0
        };
        let peak = Peak {
            azimuth_deg: item.azimuth_deg,
            elevation_deg: item.elevation_deg,
            power: item.power,
            sigma_deg: sigma as f32,
            sigma_el_deg: sigma_el_deg as f32,
            confidence: confidence(sigma, ambiguity),
            fit,
            ambiguity,
            mirror_deg,
        };
        (peak, share)
    }
}

fn pair_weight(first: f32, second: f32) -> f32 {
    let spread = first * first + second * second;
    if spread > 0.0 {
        first * second / spread.sqrt()
    } else {
        0.0
    }
}

fn decibels(ratio: f32) -> f32 {
    10.0 * ratio.log10()
}

fn sigma_deg(rates: &[f64], snr: f64, snapshots: f64, cal_sigma_deg: f64) -> f64 {
    total_sigma_deg(
        crb_sigma_rad(rates, snr, rates.len(), snapshots),
        model_sigma_rad(rates, cal_sigma_deg),
    )
}

fn best_of(mut f: impl FnMut(f64) -> f32, current: (f64, f32), lo: f64, hi: f64) -> (f64, f32) {
    let (x, fx) = brent_max(
        |x| f64::from(f(x)),
        lo,
        hi,
        peaks::REFINE_TOL_DEG,
        peaks::REFINE_ITERATIONS,
    );
    let power = fx as f32;
    if power >= current.1 {
        (x, power)
    } else {
        current
    }
}

struct Probe<'a> {
    pseudo: Pseudospectrum<'a>,
    space: Space,
    workspace: &'a Workspace,
    layout: &'a Layout,
    phase: Option<&'a PhaseMode>,
    manifold: &'a Manifold,
    freq_hz: f64,
}

impl Probe<'_> {
    fn power(&self, direction: Direction) -> f32 {
        let n = self.manifold.len();
        let mut element = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        if matches!(self.space, Space::Element | Space::Line { .. }) {
            self.manifold
                .steer(self.freq_hz, direction, &mut element[..n]);
        }
        self.evaluate(&element[..n], direction.azimuth_deg)
    }

    fn at_point(&self, grid: &SteeringGrid, point: usize) -> f32 {
        let mut scratch = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        match self.space {
            Space::Element => self.pseudo.power(grid.vector(point), &mut scratch),
            _ => self.evaluate(grid.vector(point), grid.direction(point).azimuth_deg),
        }
    }

    fn evaluate(&self, element: &[C32], azimuth_deg: f64) -> f32 {
        let mut working = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        let mut scratch = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        let m = self.workspace.steer(
            self.space,
            self.layout,
            self.phase,
            element,
            azimuth_deg,
            &mut working,
        );
        self.pseudo.power(&working[..m], &mut scratch)
    }
}

struct Beam<'a> {
    manifold: &'a Manifold,
    r: &'a CMat,
    freq_hz: f64,
    elevation_deg: f64,
    ring: &'a SteeringGrid,
}

impl Beam<'_> {
    fn power(&self, azimuth_deg: f64) -> f64 {
        let n = self.manifold.len();
        let mut a = [C32::new(0.0, 0.0); MAX_ELEMENTS];
        self.manifold.steer(
            self.freq_hz,
            Direction::new(azimuth_deg, self.elevation_deg),
            &mut a[..n],
        );
        self.of(&a[..n])
    }

    fn at_degree(&self, degree: usize) -> f64 {
        if self.elevation_deg == 0.0 && self.ring.points() == LIKELIHOOD_POINTS {
            self.of(self.ring.vector(degree))
        } else {
            self.power(degree as f64)
        }
    }

    fn of(&self, a: &[C32]) -> f64 {
        let norm = f64::from(spectrum::norm_sqr(a));
        if norm > 0.0 {
            f64::from(self.r.quad(a)) / norm
        } else {
            0.0
        }
    }

    fn peak(&self, around_deg: f64) -> f64 {
        let steps = (2.0 * LIKELIHOOD_WINDOW_DEG / LIKELIHOOD_COARSE_DEG).round() as i32;
        let mut best = (around_deg, f64::NEG_INFINITY);
        for step in 0..=steps {
            let azimuth =
                around_deg - LIKELIHOOD_WINDOW_DEG + f64::from(step) * LIKELIHOOD_COARSE_DEG;
            let power = self.power(azimuth);
            if power > best.1 {
                best = (azimuth, power);
            }
        }
        let (refined, power) = refine(|x| self.power(x), best.0, LIKELIHOOD_COARSE_DEG);
        if power >= best.1 { refined } else { best.0 }
    }
}
