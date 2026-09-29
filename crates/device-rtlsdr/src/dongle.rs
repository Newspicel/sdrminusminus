mod board;
mod catalog;
mod chip;
mod demod;
mod eeprom;
mod error;
mod radio;
mod stream;
mod tuner;
mod usb;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod hardware;

pub(crate) use board::Board;
pub(crate) use catalog::{Catalog, Listing};
pub(crate) use demod::DIRECT_MAX_HZ;
#[cfg(test)]
pub(crate) use demod::PPM_LIMIT;
pub(crate) use error::Error;
pub(crate) use radio::{DirectSampling, Dongle};
pub(crate) use stream::{IN_FLIGHT_SAMPLES, Release, TRANSFER_BYTES};
#[cfg(test)]
pub(crate) use tuner::GAINS;
