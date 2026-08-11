//! The RTL2832U driver itself: USB enumeration, register and I2C access, R82xx tuner
//! programming, and the settings the backend above drives.
//!
//! Everything here is device-level and knows nothing about the wire capability model — `caps`
//! is the only place that translates. Streaming is not here either: the transfer queue and the
//! USB error policy are `sdrmm-usb-stream`, shared with the HackRF backend, because getting that
//! policy wrong was the defect this driver exists to fix (PLAN §17, §18).
//!
//! `regs` and `tuner` are the tedious, valuable half — the RTL2832U's register/I2C encodings and
//! the R82xx's PLL, gain and filter programming, both mirroring librtlsdr, which is the only
//! specification these parts have.

mod error;
mod regs;
mod sdr;
mod tuner;

pub(crate) use error::Error;
#[cfg(test)]
pub(crate) use sdr::MAX_PPM;
pub(crate) use sdr::{
    BoardVariant, DIRECT_SAMPLING_MAX_HZ, DeviceDescriptor, DeviceDescriptors, DirectSampling,
    RtlSdr, TRANSFER_BUF_SIZE,
};
#[cfg(test)]
pub(crate) use tuner::GAIN_VALUES;
