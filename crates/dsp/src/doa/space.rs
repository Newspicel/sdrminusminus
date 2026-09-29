use num_complex::Complex;

use super::subspace::MAX_ESPRIT_SOURCES;
use super::{DoaConfig, DoaError, Estimator, MAX_PEAKS, SourceCount, UlaSide};
use crate::covariance::{CovarianceError, PhaseMode, forward_backward, smooth, smooth_diagonal};
use crate::linalg::{CMat, LinalgError};
use crate::manifold::{AzimuthSpan, Geometry, GridSpec, MAX_ELEMENTS, Permutation, Shape};
use crate::special::norm_deg;

pub const NEEDS_LINE_OR_CIRCLE: &str = "Needs a line or circle";
pub const ARRAY_CHANGED: &str = "Array changed";
pub const FB_NEEDS_SYMMETRY: &str = "FB needs a symmetric array";
pub const ELEVATION_NEEDS_2D: &str = "Elevation needs a 2D array";
pub const ELEVATION_NEEDS_GRID: &str = "Elevation needs a grid method";
pub const SIDE_NEEDS_LINE: &str = "Side needs a line";
pub const TOO_MANY_SOURCES: &str = "Too many sources";
pub const TOO_FEW_ELEMENTS: &str = "Too few elements";
pub const PEAKS_OUT_OF_RANGE: &str = "Peaks out of range";
pub const LOADING_OUT_OF_RANGE: &str = "Loading out of range";
pub const SQUELCH_OUT_OF_RANGE: &str = "Squelch out of range";
pub const PEAK_RANGE_OUT_OF_RANGE: &str = "Peak range out of range";
pub const TABLE_NEEDS_GRID: &str = "Table needs a grid method";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Line {
    pub order: Permutation,
    pub spacing_m: f64,
    pub sort_axis_deg: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layout {
    pub elements: usize,
    pub antipodes: Option<Permutation>,
    pub collinear: bool,
    pub axis_deg: Option<f64>,
    pub line: Option<Line>,
}

impl Layout {
    pub fn of(geometry: &Geometry) -> Self {
        let axis_deg = match geometry.shape() {
            Shape::Ula { axis_deg, .. } => Some(axis_deg),
            _ => geometry.line_axis_deg(),
        };
        let line = match (
            geometry.uniform_line_spacing_m(),
            geometry.axis_order(),
            geometry.line_axis_deg(),
        ) {
            (Some(spacing_m), Some(order), Some(sort_axis_deg)) => Some(Line {
                order,
                spacing_m,
                sort_axis_deg,
            }),
            _ => None,
        };
        Self {
            elements: geometry.len(),
            antipodes: geometry.antipodes(),
            collinear: geometry.is_collinear(),
            axis_deg,
            line,
        }
    }

    pub fn grid_spec(&self, config: &DoaConfig) -> GridSpec {
        let span = match (config.ula_side, self.axis_deg) {
            (UlaSide::Front, Some(axis)) => AzimuthSpan::Half {
                centre_deg: norm_deg(axis - 90.0),
            },
            (UlaSide::Back, Some(axis)) => AzimuthSpan::Half {
                centre_deg: norm_deg(axis + 90.0),
            },
            _ => AzimuthSpan::Full,
        };
        GridSpec {
            azimuth_step_deg: config.azimuth_step_deg,
            span,
            elevation: config.elevation,
        }
    }

    pub fn side_centre_deg(&self, side: UlaSide) -> Option<f64> {
        let axis = self.axis_deg?;
        Some(match side {
            UlaSide::Both | UlaSide::Front => axis - 90.0,
            UlaSide::Back => axis + 90.0,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Space {
    Element,
    Line { subarrays: usize },
    Modes,
    Vandermonde { subarrays: usize },
}

impl Space {
    pub fn order(self, elements: usize, phase: Option<&PhaseMode>) -> usize {
        match self {
            Self::Element => elements,
            Self::Line { subarrays } => (elements + 1).saturating_sub(subarrays),
            Self::Modes => phase.map_or(0, PhaseMode::virtual_len),
            Self::Vandermonde { subarrays } => phase.map_or(0, |phase| {
                (phase.vandermonde_len() + 1).saturating_sub(subarrays)
            }),
        }
    }
}

const fn unsupported(text: &'static str) -> DoaError {
    DoaError::Unsupported(text)
}

pub(super) fn plan(
    config: &DoaConfig,
    layout: &Layout,
    phase: Option<&PhaseMode>,
) -> Result<Space, DoaError> {
    check_scalars(config)?;
    let structured = structured(config);
    if config.elevation.is_some() {
        if layout.collinear {
            return Err(unsupported(ELEVATION_NEEDS_2D));
        }
        if structured {
            return Err(unsupported(ELEVATION_NEEDS_GRID));
        }
    }
    if config.ula_side != UlaSide::Both && layout.axis_deg.is_none() {
        return Err(unsupported(SIDE_NEEDS_LINE));
    }
    let space = choose(config, layout, phase, structured)?;
    let order = space.order(layout.elements, phase);
    let fixed = match config.sources {
        SourceCount::Fixed(d) => Some(usize::from(d)),
        SourceCount::Auto => None,
    };
    if order < 2 || (config.smoothing > 0 && fixed.is_some_and(|d| d >= order)) {
        return Err(unsupported(TOO_FEW_ELEMENTS));
    }
    let esprit_limit = config.estimator == Estimator::Esprit;
    if fixed.is_some_and(|d| d >= order || (esprit_limit && d > MAX_ESPRIT_SOURCES)) {
        return Err(unsupported(TOO_MANY_SOURCES));
    }
    Ok(space)
}

pub(super) fn check_table(config: &DoaConfig, measured: bool) -> Result<(), DoaError> {
    if !measured {
        return Ok(());
    }
    if structured(config) {
        return Err(unsupported(TABLE_NEEDS_GRID));
    }
    if config.forward_backward {
        return Err(unsupported(FB_NEEDS_SYMMETRY));
    }
    Ok(())
}

const fn structured(config: &DoaConfig) -> bool {
    matches!(config.estimator, Estimator::RootMusic | Estimator::Esprit) || config.smoothing > 0
}

fn check_scalars(config: &DoaConfig) -> Result<(), DoaError> {
    if !(1..=MAX_PEAKS).contains(&usize::from(config.max_peaks)) {
        return Err(unsupported(PEAKS_OUT_OF_RANGE));
    }
    if !(config.loading.is_finite() && config.loading >= 0.0) {
        return Err(unsupported(LOADING_OUT_OF_RANGE));
    }
    if !(config.peak_range_db.is_finite() && config.peak_range_db >= 0.0) {
        return Err(unsupported(PEAK_RANGE_OUT_OF_RANGE));
    }
    let squelch_ok = config.squelch.is_none_or(|squelch| {
        squelch.open_db.is_finite()
            && squelch.hysteresis_db.is_finite()
            && squelch.hysteresis_db >= 0.0
    });
    if !squelch_ok {
        return Err(unsupported(SQUELCH_OUT_OF_RANGE));
    }
    Ok(())
}

fn choose(
    config: &DoaConfig,
    layout: &Layout,
    phase: Option<&PhaseMode>,
    structured: bool,
) -> Result<Space, DoaError> {
    let subarrays = usize::from(config.smoothing) + 1;
    if !structured {
        if config.forward_backward && layout.antipodes.is_none() {
            return Err(unsupported(FB_NEEDS_SYMMETRY));
        }
        return Ok(Space::Element);
    }
    if layout.line.is_some() {
        return Ok(Space::Line { subarrays });
    }
    let Some(phase) = phase else {
        return Err(unsupported(NEEDS_LINE_OR_CIRCLE));
    };
    if config.estimator == Estimator::RootMusic && config.smoothing == 0 {
        return Ok(Space::Modes);
    }
    if phase.vandermonde_modes().is_none() {
        return Err(CovarianceError::BesselNull.into());
    }
    Ok(Space::Vandermonde { subarrays })
}

pub(super) struct Workspace {
    pub work: CMat,
    staging: CMat,
    whiten: [f32; MAX_ELEMENTS],
    diagonal: [f32; MAX_ELEMENTS],
}

impl Workspace {
    pub fn new(elements: usize) -> Result<Self, LinalgError> {
        Ok(Self {
            work: CMat::identity(elements)?,
            staging: CMat::identity(elements)?,
            whiten: [1.0; MAX_ELEMENTS],
            diagonal: [1.0; MAX_ELEMENTS],
        })
    }

    pub fn prepare(
        &mut self,
        space: Space,
        fb: bool,
        layout: &Layout,
        phase: Option<&PhaseMode>,
        r: &CMat,
    ) -> Result<usize, DoaError> {
        match space {
            Space::Element => {
                self.work.clone_from(r);
                if fb {
                    let pairs = layout
                        .antipodes
                        .as_ref()
                        .ok_or(unsupported(FB_NEEDS_SYMMETRY))?;
                    forward_backward(&mut self.work, pairs)?;
                }
            }
            Space::Line { subarrays } => {
                let line = layout.line.ok_or(unsupported(NEEDS_LINE_OR_CIRCLE))?;
                smooth(r, &line.order, subarrays, fb, &mut self.work)?;
            }
            Space::Modes => {
                let phase = phase.ok_or(unsupported(NEEDS_LINE_OR_CIRCLE))?;
                phase.transform(r, &mut self.work)?;
                if fb {
                    let reversal = Permutation::reversal(self.work.order());
                    forward_backward(&mut self.work, &reversal)?;
                }
            }
            Space::Vandermonde { subarrays } => {
                let phase = phase.ok_or(unsupported(NEEDS_LINE_OR_CIRCLE))?;
                self.prepare_vandermonde(phase, subarrays, fb, r)?;
            }
        }
        self.whiten_unless(space);
        Ok(self.work.order())
    }

    fn prepare_vandermonde(
        &mut self,
        phase: &PhaseMode,
        subarrays: usize,
        fb: bool,
        r: &CMat,
    ) -> Result<(), DoaError> {
        phase.transform_vandermonde(r, &mut self.staging)?;
        let full = self.staging.order();
        smooth(
            &self.staging,
            &Permutation::identity(full),
            subarrays,
            fb,
            &mut self.work,
        )?;
        let len = smooth_diagonal(phase.noise_diagonal(), subarrays, fb, &mut self.diagonal);
        if len != self.work.order() {
            return Err(CovarianceError::TooFewForSmoothing.into());
        }
        for (weight, &noise) in self.whiten.iter_mut().zip(&self.diagonal[..len]) {
            *weight = if noise > 0.0 {
                noise.sqrt().recip()
            } else {
                0.0
            };
        }
        for row in 0..len {
            for col in 0..len {
                let scale = self.whiten[row] * self.whiten[col];
                self.work.set(row, col, self.work.get(row, col) * scale);
            }
        }
        Ok(())
    }

    fn whiten_unless(&mut self, space: Space) {
        if !matches!(space, Space::Vandermonde { .. }) {
            self.whiten.fill(1.0);
        }
    }

    pub fn order(&self) -> usize {
        self.work.order()
    }

    pub fn weights<'a>(&'a self, space: Space, phase: Option<&'a PhaseMode>) -> Option<&'a [f32]> {
        let m = self.order();
        match space {
            Space::Element | Space::Line { .. } => None,
            Space::Modes => phase.map(PhaseMode::mode_magnitudes),
            Space::Vandermonde { .. } => Some(&self.whiten[..m]),
        }
    }

    pub fn whitening(&self) -> &[f32] {
        &self.whiten[..self.order()]
    }

    pub fn steer(
        &self,
        space: Space,
        layout: &Layout,
        phase: Option<&PhaseMode>,
        element: &[Complex<f32>],
        azimuth_deg: f64,
        out: &mut [Complex<f32>],
    ) -> usize {
        let m = self.order();
        let out = &mut out[..m];
        match (space, layout.line) {
            (Space::Element, _) => out.copy_from_slice(&element[..m]),
            (Space::Line { .. }, Some(line)) => {
                for (i, slot) in out.iter_mut().enumerate() {
                    *slot = element[line.order.get(i)];
                }
            }
            (Space::Line { .. }, None) => out.fill(Complex::new(0.0, 0.0)),
            (Space::Modes | Space::Vandermonde { .. }, _) => {
                let weights = self.weights(space, phase).unwrap_or(&[]);
                vandermonde(azimuth_deg, weights, out);
            }
        }
        m
    }
}

fn vandermonde(azimuth_deg: f64, weights: &[f32], out: &mut [Complex<f32>]) {
    let turn = Complex::from_polar(1.0f64, azimuth_deg.to_radians());
    let mut power = Complex::new(1.0f64, 0.0);
    for (slot, index) in out.iter_mut().zip(0..) {
        let weight = f64::from(weights.get(index).copied().unwrap_or(0.0));
        *slot = Complex::new((power.re * weight) as f32, (power.im * weight) as f32);
        power *= turn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{Vec3, Winding, wavenumber};

    const FREQ: f64 = 433.92e6;

    fn config(estimator: Estimator) -> DoaConfig {
        DoaConfig {
            estimator,
            ..DoaConfig::default()
        }
    }

    fn uca(count: usize, beta: f64) -> (Layout, PhaseMode) {
        let geometry =
            Geometry::uca(beta / wavenumber(FREQ), count, 0.0, Winding::Clockwise).unwrap();
        let phase = PhaseMode::new(&geometry, FREQ).unwrap();
        (Layout::of(&geometry), phase)
    }

    #[test]
    fn plans_follow_the_geometry() {
        let (layout, phase) = uca(8, 2.0);
        let phase = Some(&phase);
        assert_eq!(
            plan(&config(Estimator::Music), &layout, phase),
            Ok(Space::Element)
        );
        assert_eq!(
            plan(&config(Estimator::RootMusic), &layout, phase),
            Ok(Space::Modes)
        );
        assert_eq!(
            plan(&config(Estimator::Esprit), &layout, phase),
            Ok(Space::Vandermonde { subarrays: 1 })
        );
        let smoothed = DoaConfig {
            smoothing: 2,
            ..config(Estimator::Music)
        };
        let space = plan(&smoothed, &layout, phase).unwrap();
        assert_eq!(space, Space::Vandermonde { subarrays: 3 });
        assert_eq!(space.order(8, phase), 5);
        let line = Layout::of(&Geometry::ula(0.3, 6, 180.0).unwrap());
        assert_eq!(line.axis_deg, Some(180.0));
        assert_eq!(
            plan(&smoothed, &line, None),
            Ok(Space::Line { subarrays: 3 })
        );
        assert_eq!(line.grid_spec(&smoothed).span, AzimuthSpan::Full);
        let front = DoaConfig {
            ula_side: UlaSide::Front,
            ..smoothed
        };
        assert_eq!(
            line.grid_spec(&front).span,
            AzimuthSpan::Half { centre_deg: 90.0 }
        );
    }

    #[test]
    fn refusals_carry_the_node_texts() {
        let triangle = Layout::of(
            &Geometry::explicit(&[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.3, 0.0, 0.0),
                Vec3::new(0.0, 0.4, 0.0),
            ])
            .unwrap(),
        );
        assert_eq!(
            plan(&config(Estimator::RootMusic), &triangle, None),
            Err(DoaError::Unsupported(NEEDS_LINE_OR_CIRCLE))
        );
        let fb = DoaConfig {
            forward_backward: true,
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&fb, &triangle, None),
            Err(DoaError::Unsupported(FB_NEEDS_SYMMETRY))
        );
        let side = DoaConfig {
            ula_side: UlaSide::Back,
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&side, &triangle, None),
            Err(DoaError::Unsupported(SIDE_NEEDS_LINE))
        );
        let line = Layout::of(&Geometry::ula(0.3, 4, 90.0).unwrap());
        let many = DoaConfig {
            sources: SourceCount::Fixed(4),
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&many, &line, None),
            Err(DoaError::Unsupported(TOO_MANY_SOURCES))
        );
        let deep = DoaConfig {
            smoothing: 3,
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&deep, &line, None),
            Err(DoaError::Unsupported(TOO_FEW_ELEMENTS))
        );
        let crowded = DoaConfig {
            smoothing: 1,
            sources: SourceCount::Fixed(3),
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&crowded, &line, None),
            Err(DoaError::Unsupported(TOO_FEW_ELEMENTS))
        );
        let (circle, phase) = uca(5, 2.0);
        let raised = DoaConfig {
            elevation: Some(crate::manifold::ElevationSpan::default()),
            ..config(Estimator::Esprit)
        };
        assert_eq!(
            plan(&raised, &circle, Some(&phase)),
            Err(DoaError::Unsupported(ELEVATION_NEEDS_GRID))
        );
        assert_eq!(
            plan(&raised, &line, None),
            Err(DoaError::Unsupported(ELEVATION_NEEDS_2D))
        );
        let peaks = DoaConfig {
            max_peaks: 0,
            ..config(Estimator::Music)
        };
        assert_eq!(
            plan(&peaks, &line, None),
            Err(DoaError::Unsupported(PEAKS_OUT_OF_RANGE))
        );
        let (null, phase) = uca(5, 2.405);
        assert_eq!(
            plan(&config(Estimator::Esprit), &null, Some(&phase)),
            Err(DoaError::Covariance(CovarianceError::BesselNull))
        );
        assert_eq!(
            plan(&config(Estimator::RootMusic), &null, Some(&phase)),
            Ok(Space::Modes)
        );
    }

    #[test]
    fn working_steering_is_vandermonde_on_a_circle() {
        let (layout, phase) = uca(8, 2.0);
        let mut workspace = Workspace::new(8).unwrap();
        let r = CMat::identity(8).unwrap();
        let space = Space::Vandermonde { subarrays: 1 };
        let m = workspace
            .prepare(space, false, &layout, Some(&phase), &r)
            .unwrap();
        assert_eq!(m, 7);
        for i in 0..m {
            assert!((workspace.work.get(i, i).re - 1.0).abs() < 1e-4);
        }
        let mut out = [Complex::new(0.0f32, 0.0); 16];
        let written = workspace.steer(space, &layout, Some(&phase), &[], 30.0, &mut out);
        assert_eq!(written, 7);
        let ratio = out[1] / out[0] * workspace.whitening()[0] / workspace.whitening()[1];
        assert!((ratio.arg() - 30f32.to_radians()).abs() < 1e-5);
    }
}
