//! Errors the transport can raise. Deliberately narrow: the drivers own device semantics, so
//! everything here is either a USB-level failure or the transfer-error threshold being reached.

use nusb::transfer::TransferError;

/// A stream that ended on its own — never on a caller's `stop`.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// The error policy gave up: `attempts` genuine transfer errors with no successful
    /// completion in between (see [`crate::TransferPolicy`]).
    #[error("usb transfer failed {attempts} times in a row: {source}")]
    Transfers {
        /// Consecutive errored completions that reached the threshold.
        attempts: u32,
        /// The completion status that tripped it.
        source: TransferError,
    },
    /// The endpoint could not be prepared or reset.
    #[error("usb endpoint: {0}")]
    Endpoint(#[from] nusb::Error),
    /// The transfer pump could not be started.
    #[error("spawn usb transfer pump: {0}")]
    Spawn(#[source] std::io::Error),
}

impl StreamError {
    /// Whether the device left the bus. Restarting the stream in place cannot help — the
    /// endpoint, the interface claim and the device handle are all gone with it.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        match self {
            Self::Transfers { source, .. } => *source == TransferError::Disconnected,
            Self::Endpoint(e) => e.kind() == nusb::ErrorKind::Disconnected,
            Self::Spawn(_) => false,
        }
    }
}

/// Result alias for transport operations.
pub type Result<T> = std::result::Result<T, StreamError>;
