mod doppler;
mod observer;
mod pass;
mod satellite;
mod sgp4;
mod time;
mod tle;

pub use doppler::{SPEED_OF_LIGHT_KM_S, downlink_hz, uplink_hz};
pub use observer::{Look, Observer};
pub use pass::Pass;
pub use satellite::Satellite;
pub use sgp4::{Propagator, State};
pub use time::{julian_date, unix_seconds};
pub use tle::Tle;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OrbitError {
    #[error("bad element set: {0}")]
    Tle(&'static str),
    #[error("unusable elements: {0}")]
    Elements(&'static str),
    #[error("the orbit has decayed: {0}")]
    Decayed(&'static str),
}
