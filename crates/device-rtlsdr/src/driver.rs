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
