pub mod analog;
pub mod catalog;
pub mod e2e;
pub mod genie;
pub mod impair;
pub mod limits;
pub mod perf;
pub mod reference;
pub mod rng;
pub mod sweep;
pub mod theory;

use serde::{Deserialize, Serialize};

pub const MIN_ERRORS_PER_POINT: u64 = 100;

pub const FAILURE_BER: f64 = 1e-2;

/// See [`FAILURE_BER`].
pub const SENSITIVITY_MARGIN_DB: f64 = 3.0;

/// One measured point of an error-ratio curve: `errors` out of `trials` at `ebn0_db`.
/// What a trial is — a bit, a symbol, a frame — is the owning [`Curve`]'s statement to make;
/// the counts stay raw so confidence intervals can always be recomputed from the committed
/// artifact.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub ebn0_db: f64,
    pub errors: u64,
    pub trials: u64,
}

impl CurvePoint {
    /// The measured error ratio; 0 for an empty point rather than NaN, so a curve with an
    /// unreached point still compares.
    #[must_use]
    pub fn rate(&self) -> f64 {
        if self.trials == 0 {
            return 0.0;
        }
        self.errors as f64 / self.trials as f64
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub label: String,
    pub points: Vec<CurvePoint>,
}
