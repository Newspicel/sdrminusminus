pub(crate) mod capture;
mod channel;
mod command;
mod frontend;
mod patches;
mod retire;
mod spectrum;
mod worker;

pub use capture::CaptureRuntime;
#[cfg(test)]
pub(crate) use capture::ring_capacity;
pub(crate) use channel::{ChannelHost, ChannelSinks, DecodedSink, RawDecoded, RawImage, reaches};
pub(crate) use command::DspCommand;
pub use frontend::DspMeta;
pub(crate) use patches::DeviceRuntime;
pub use spectrum::SpectrumSnapshot;
pub(crate) use worker::Waker;

const FFT_SIZE: usize = 4096;
pub(crate) const DSP_BLOCK: usize = 2048;
