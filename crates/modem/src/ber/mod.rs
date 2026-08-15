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

pub const SENSITIVITY_MARGIN_DB: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub ebn0_db: f64,
    pub errors: u64,
    pub trials: u64,
}

impl CurvePoint {
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
