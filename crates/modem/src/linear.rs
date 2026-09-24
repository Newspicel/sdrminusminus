mod acquire;
mod anchor;
mod carrier;
mod demod;
mod differential;
mod envelope;
mod frontend;
mod modulator;
mod params;
mod timing;

pub use acquire::{Chirp, FrequencyAcquisition};
pub use anchor::{AnchorError, PhaseAnchor};
pub use carrier::{CarrierLoop, PhaseDetector};
pub use demod::{
    LinearBurstDemod, LinearDemod, LinearTiming, TIMING_BW_BURST, TIMING_BW_CONTINUOUS,
};
pub use differential::{DifferentialDetector, differential_detect};
pub use envelope::{EnvelopeDemod, EnvelopeTiming, slice_amplitude};
pub use frontend::{FrontCorrection, FrontEstimator};
pub use modulator::LinearMod;
pub use params::{LinearError, LinearParams};
pub use timing::{
    FeedforwardTiming, MIN_SPS, TimingMetric, TimingTrack, resample_at, rotated_power_offset,
    square_law_offset, square_law_track,
};
