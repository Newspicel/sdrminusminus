//! What distinguishes one linear entry from another, as data ( §3.3): the point
//! table, the amplitude pulse, the oversampling, an optional per-symbol rotation and an optional
//! quadrature stagger. Nothing else. A `match` on a standard inside [`super`] is a defect, and
//! the four axes below are what make that achievable — BPSK, 1024-QAM, π/2-BPSK, OQPSK and
//! π/4-DQPSK differ only in these values.

use std::fmt;

use crate::constellation::Constellation;

/// Why a parameter set was rejected. Construction is setup-time, so this is a `Result` —
/// a bad configuration must surface as an error, never take a running engine down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinearError {
    /// The pulse has no taps, or its energy is not 1 (the crate-root amplitude-pulse
    /// convention: `pulse::Norm::Energy`). Every Eb/N0 in `ber` depends on it.
    PulseNotUnitEnergy,
    /// Samples per symbol below 2 — a linear receiver's timing detector needs a mid-symbol
    /// sample, and the Gardner detector in `SymbolSync` literally reads one.
    TooFewSamplesPerSymbol(usize),
    /// [`LinearParams::with_offset`] on an odd `sps`. The stagger is exactly half a symbol, and
    /// the receiver undoes it with an integer-sample delay; an odd `sps` has no such delay.
    OffsetNeedsEvenSps(usize),
    /// A rotation that is not a finite number of radians.
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

/// A linear entry's complete parameterisation. Immutable once built, so every invariant checked
/// at construction holds for the whole life of the modulator and demodulator built from it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearParams {
    constellation: Constellation,
    pulse: Vec<f32>,
    sps: usize,
    rotation_rad: f64,
    offset: bool,
}

impl LinearParams {
    /// `pulse` is the transmit amplitude pulse at unit energy — root-raised cosine for the
    /// shaped entries, rect for the keyed ones — and `sps` its whole number of samples per
    /// symbol. Integer `sps` (unlike the CPM engine's fractional one) is a deliberate
    /// restriction of this engine: the stagger axis needs an exact half-symbol delay, and every
    /// linear entry in the catalog is specified at an integer oversampling anyway.
    ///
    /// # Errors
    /// [`LinearError::PulseNotUnitEnergy`], [`LinearError::TooFewSamplesPerSymbol`].
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

    /// Adds a per-symbol phase rotation: symbol k is transmitted at `exp(j·k·rotation)` times
    /// its table point. π/2 gives π/2-BPSK, π/4 gives the π/4-DQPSK grid
    /// ([`constellation::tables::offset_rotation`](crate::constellation::tables::offset_rotation)).
    /// The receiver de-rotates by the same schedule, so nothing downstream sees it.
    ///
    /// # Errors
    /// [`LinearError::RotationNotFinite`].
    pub fn with_rotation(mut self, rotation_rad: f64) -> Result<Self, LinearError> {
        if !rotation_rad.is_finite() {
            return Err(LinearError::RotationNotFinite);
        }
        self.rotation_rad = rotation_rad;
        Ok(self)
    }

    /// Staggers the quadrature rail by half a symbol — the OQPSK axis. What it buys is envelope:
    /// with both rails switching at once a QPSK trajectory passes through the origin on a
    /// diagonal transition, and a saturated amplifier regrows the spectrum it clipped there;
    /// staggered, no transition crosses more than 90°.
    ///
    /// The receiver undoes it with an integer-sample delay on the *in-phase* rail rather than
    /// advancing Q, which lands both rails on the odd half-symbol instants and hands the rest
    /// of the chain ordinary QPSK. That is why `sps` must be even.
    ///
    /// # Errors
    /// [`LinearError::OffsetNeedsEvenSps`].
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

    /// The stagger in samples — half a symbol, or zero when the entry is not staggered.
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

    /// The pulse convention is load-bearing for every curve, so a pulse that is not unit energy
    /// is refused at construction rather than shifting a waterfall by 10·log10(Σh²) later.
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
        // Without the stagger an odd sps is perfectly fine.
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
