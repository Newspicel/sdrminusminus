pub(crate) mod capture;
mod channel;
mod command;
mod downconvert;
mod frontend;
mod patches;
mod retire;
mod spectrum;
mod subbands;
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
pub(crate) const MAX_DSP_BLOCK: usize = 8192;

pub(crate) fn dsp_block_len(sample_rate: f64) -> usize {
    if sample_rate >= MAX_DSP_BLOCK as f64 * 1000.0 {
        MAX_DSP_BLOCK
    } else if sample_rate >= (2 * DSP_BLOCK) as f64 * 1000.0 {
        2 * DSP_BLOCK
    } else {
        DSP_BLOCK
    }
}
