use std::fmt;

use crate::constellation::Constellation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinearError {
    PulseNotUnitEnergy,
    TooFewSamplesPerSymbol(usize),
    OffsetNeedsEvenSps(usize),
    RotationNotFinite,
}

impl fmt::Display for LinearError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PulseNotUnitEnergy => {
                write!(
                    f,
                    "amplitude pulse must be unit energy (pulse::Norm::Energy)"
                )
            }
            Self::TooFewSamplesPerSymbol(sps) => write!(f, "{sps} samples per symbol is under 2"),
            Self::OffsetNeedsEvenSps(sps) => {
                write!(f, "quadrature stagger needs an even sps, got {sps}")
            }
            Self::RotationNotFinite => write!(f, "per-symbol rotation must be finite"),
        }
    }
}

impl std::error::Error for LinearError {}

#[derive(Clone, Debug, PartialEq)]
pub struct LinearParams {
    constellation: Constellation,
    pulse: Vec<f32>,
    sps: usize,
    rotation_rad: f64,
    offset: bool,
}

impl LinearParams {
    pub fn new(
        constellation: Constellation,
        pulse: Vec<f32>,
        sps: usize,
    ) -> Result<Self, LinearError> {
        if sps < 2 {
            return Err(LinearError::TooFewSamplesPerSymbol(sps));
        }
        let energy: f64 = pulse.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
        if pulse.is_empty() || (energy - 1.0).abs() > 1e-3 {
            return Err(LinearError::PulseNotUnitEnergy);
        }
        Ok(Self {
            constellation,
            pulse,
            sps,
            rotation_rad: 0.0,
            offset: false,
        })
    }

    pub fn with_rotation(mut self, rotation_rad: f64) -> Result<Self, LinearError> {
        if !rotation_rad.is_finite() {
            return Err(LinearError::RotationNotFinite);
        }
        self.rotation_rad = rotation_rad;
        Ok(self)
    }

    pub fn with_offset(mut self, offset: bool) -> Result<Self, LinearError> {
        if offset && !self.sps.is_multiple_of(2) {
            return Err(LinearError::OffsetNeedsEvenSps(self.sps));
        }
        self.offset = offset;
        Ok(self)
    }

    #[must_use]
    pub fn constellation(&self) -> &Constellation {
        &self.constellation
    }

    #[must_use]
    pub fn pulse(&self) -> &[f32] {
        &self.pulse
    }

    #[must_use]
    pub fn sps(&self) -> usize {
        self.sps
    }

    #[must_use]
    pub fn rotation_rad(&self) -> f64 {
        self.rotation_rad
    }

    #[must_use]
    pub fn offset(&self) -> bool {
        self.offset
    }

    #[must_use]
    pub fn stagger_samples(&self) -> usize {
        if self.offset { self.sps / 2 } else { 0 }
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.constellation.bits_per_symbol()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constellation::tables,
        pulse::{self, Norm},
    };

    fn rrc(sps: usize) -> Vec<f32> {
        pulse::root_raised_cosine(sps as f64, 0.35, 8, Norm::Energy)
    }

    #[test]
    fn a_valid_entry_carries_its_four_axes() {
        let p = LinearParams::new(tables::qam_square(16).unwrap(), rrc(8), 8)
            .unwrap()
            .with_rotation(tables::PI_4_ROTATION)
            .unwrap()
            .with_offset(true)
            .unwrap();
        assert_eq!(p.sps(), 8);
        assert_eq!(p.stagger_samples(), 4);
        assert_eq!(p.bits_per_symbol(), 4);
        assert!((p.rotation_rad() - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    }

    #[test]
    fn a_pulse_off_the_energy_convention_is_refused() {
        let area = pulse::root_raised_cosine(8.0, 0.35, 8, Norm::Area);
        assert_eq!(
            LinearParams::new(tables::pam(2).unwrap(), area, 8).unwrap_err(),
            LinearError::PulseNotUnitEnergy
        );
        assert_eq!(
            LinearParams::new(tables::pam(2).unwrap(), Vec::new(), 8).unwrap_err(),
            LinearError::PulseNotUnitEnergy
        );
    }

    #[test]
    fn the_stagger_needs_a_half_symbol_that_exists() {
        assert_eq!(
            LinearParams::new(tables::qam_square(4).unwrap(), rrc(5), 5)
                .unwrap()
                .with_offset(true)
                .unwrap_err(),
            LinearError::OffsetNeedsEvenSps(5)
        );
        assert!(
            LinearParams::new(tables::qam_square(4).unwrap(), rrc(5), 5)
                .unwrap()
                .with_offset(false)
                .is_ok()
        );
    }

    #[test]
    fn degenerate_axes_are_refused() {
        assert_eq!(
            LinearParams::new(tables::pam(2).unwrap(), rrc(8), 1).unwrap_err(),
            LinearError::TooFewSamplesPerSymbol(1)
        );
        assert_eq!(
            LinearParams::new(tables::pam(2).unwrap(), rrc(8), 8)
                .unwrap()
                .with_rotation(f64::NAN)
                .unwrap_err(),
            LinearError::RotationNotFinite
        );
    }
}
