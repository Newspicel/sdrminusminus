//! Errors from RTL2832U operations.

use sdrmm_usb_stream::StreamError;

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong talking to a dongle.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No matching dongle was enumerated.
    #[error("RTL-SDR device not found")]
    DeviceNotFound,

    /// The USB device could not be opened.
    #[error("failed to open USB device: {0}")]
    OpenFailed(#[source] nusb::Error),

    /// Interface 0 could not be claimed — usually another process holds it.
    #[error("failed to claim USB interface: {0}")]
    ClaimFailed(#[source] nusb::Error),

    /// A register or I2C control transfer failed.
    #[error("control transfer failed: {0}")]
    ControlTransfer(#[source] nusb::transfer::TransferError),

    /// A control transfer returned fewer bytes than the register needs.
    #[error("{what} returned {got} bytes")]
    ShortResponse {
        /// The operation that came up short.
        what: &'static str,
        /// Bytes actually returned.
        got: usize,
    },

    /// Neither known tuner answered on the I2C bus.
    #[error("no supported tuner found (checked R820T at 0x34, R828D at 0x74)")]
    TunerNotFound,

    /// The tuner PLL did not lock at the requested frequency.
    #[error("PLL failed to lock at {freq_hz} Hz")]
    PllLockFailed {
        /// The requested frequency.
        freq_hz: u64,
    },

    /// The requested rate is outside the resampler's two valid windows.
    #[error("invalid sample rate {rate} Hz (valid: 225001-300000 or 900001-3200000)")]
    InvalidSampleRate {
        /// The requested rate.
        rate: u32,
    },

    /// A caller-supplied value the hardware cannot take.
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// The bulk-IN transport could not be started or ended on its own.
    #[error("streaming: {0}")]
    Stream(#[from] StreamError),
}
