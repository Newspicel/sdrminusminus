//! The measurement harness (MODEM-PLAN §4): the universal consumer every catalog entry answers
//! to. Four measurement classes — correctness (§4.1), performance (§4.2), resistance (§4.3),
//! end-to-end (§4.4) — built before the first engine and applied to every entry after it.
//!
//! The trust chain, in order: [`theory`] is trusted because its closed forms reproduce
//! published values; [`impair`] is trusted because every impairment's applied value is
//! measured back in its own unit test; [`sweep`] is trusted only once its measured BPSK curve
//! sits within 0.2 dB of the exact erfc form across 0–10 dB — the calibration gate every
//! later measurement inherits. Nothing above the harness is believed ahead of it.
//!
//! Every run is seeded ([`rng`]) and reproducible; a curve or table that cannot be regenerated
//! bit-for-bit from its stated seed is a bug in the harness, not a tolerance to widen.

pub mod e2e;
pub mod impair;
pub mod limits;
pub mod perf;
pub mod reference;
pub mod rng;
pub mod sweep;
pub mod theory;

use serde::{Deserialize, Serialize};

/// Errors a sweep point accumulates before its ratio is believed (§4.1: minimum error counts
/// per point). At 100 the two-sided 95% interval is within ±20% of the estimate — but that
/// interval is vertical, and a dB gate reads horizontally: the horizontal CI is the vertical
/// one divided by the curve's local log-slope. Where BER falls steeply (≳0.5 decade/dB, the
/// high-SNR region) 100 errors keeps a 0.2 dB gate honest; on the shallow low-SNR shoulder
/// (~0.15–0.2 decade/dB at 0–2 dB for BPSK) the same 100 errors is ±0.3–0.4 dB of pure
/// counting noise — measured as a +0.246 dB excursion that failed a calibration run. So this
/// is a floor, asserted on every point; fixed-tolerance gates budget errors per point against
/// the local slope (the gate tests in `reference`/`sweep` document the arithmetic).
pub const MIN_ERRORS_PER_POINT: u64 = 100;

/// Default failure criterion for the limits runner (§4.3): the axis value at which
/// post-detection BER exceeds this while the entry operates [`SENSITIVITY_MARGIN_DB`] above
/// its measured 1e-3 sensitivity — or at which sync/lock is lost, whichever comes first.
/// Entries for which BER is meaningless (analog, acquisition metrics) document their own.
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

/// A measured error-ratio curve, the committed artifact behind §4.1. `label` states exactly
/// what was counted (e.g. `"dmr uncoded BER, steady-state, seed 0x5eed"`); `points` ascend in
/// Eb/N0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub label: String,
    pub points: Vec<CurvePoint>,
}
