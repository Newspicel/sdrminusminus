use sdrmm_usb_stream::StreamError;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("RTL-SDR device not found")]
    DeviceNotFound,

    #[error("failed to open USB device: {0}")]
    OpenFailed(#[source] nusb::Error),

    #[error("failed to claim USB interface: {0}")]
    ClaimFailed(#[source] nusb::Error),

    #[error("control transfer failed: {0}")]
    ControlTransfer(#[source] nusb::transfer::TransferError),

    #[error("{what} returned {got} bytes")]
    ShortResponse { what: &'static str, got: usize },

    #[error("no supported tuner found (checked R820T at 0x34, R828D at 0x74)")]
    TunerNotFound,

    #[error("PLL failed to lock at {freq_hz} Hz")]
    PllLockFailed { freq_hz: u64 },

    #[error("invalid sample rate {rate} Hz (valid: 225001-300000 or 900001-3200000)")]
    InvalidSampleRate { rate: u32 },

    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    #[error("streaming: {0}")]
    Stream(#[from] StreamError),
}
