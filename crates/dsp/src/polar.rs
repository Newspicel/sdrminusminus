use num_complex::Complex;

use crate::beamform::WeightSet;
use crate::linalg::{CMat, Eigen, HermitianEigen, LinalgError};

pub const LINEAR_LIMIT_DEG: f32 = 5.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stokes {
    pub i: f32,
    pub q: f32,
    pub u: f32,
    pub v: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Right,
    Left,
    Linear,
}

impl Stokes {
    #[must_use]
    pub fn from_covariance(r_aa: f32, r_bb: f32, r_ab: Complex<f32>) -> Self {
        Self {
            i: r_aa + r_bb,
            q: r_aa - r_bb,
            u: 2.0 * r_ab.re,
            v: 2.0 * r_ab.im,
        }
    }

    #[must_use]
    pub fn polarised(&self) -> f32 {
        (self.q * self.q + self.u * self.u + self.v * self.v).sqrt()
    }

    #[must_use]
    pub fn degree(&self) -> f32 {
        if self.i > 0.0 {
            (self.polarised() / self.i).min(1.0)
        } else {
            0.0
        }
    }

    #[must_use]
    pub fn angle_deg(&self) -> f32 {
        0.5 * self.u.atan2(self.q).to_degrees()
    }

    #[must_use]
    pub fn ellipticity_deg(&self) -> f32 {
        let polarised = self.polarised();
        if polarised > 0.0 {
            0.5 * (self.v / polarised).clamp(-1.0, 1.0).asin().to_degrees()
        } else {
            0.0
        }
    }

    #[must_use]
    pub fn hand(&self, flip: bool) -> Hand {
        if self.ellipticity_deg().abs() < LINEAR_LIMIT_DEG {
            return Hand::Linear;
        }
        if (self.v > 0.0) == flip {
            Hand::Left
        } else {
            Hand::Right
        }
    }
}

pub fn matched_weights(
    r: &CMat,
    orthogonal: bool,
    eigen: &mut HermitianEigen,
    values: &mut Eigen,
    out: &mut WeightSet,
) -> Result<(), LinalgError> {
    if r.order() != 2 {
        return Err(LinalgError::Order(r.order()));
    }
    eigen.solve(r, values)?;
    let index = if orthogonal { 0 } else { 1 };
    let mut weights = WeightSet::from_textbook(values.vector(index));
    if !weights.is_finite() {
        return Err(LinalgError::NonFinite);
    }
    weights.align_phase_to(out);
    *out = weights;
    Ok(())
}

#[cfg(test)]
mod tests;
