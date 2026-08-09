//! Errors from HackRF operations.

use sdrmm_usb_stream::StreamError;

/// Convenience result alias.
pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong talking to a HackRF.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A configuration value the radio will not take.
    #[error("invalid configuration for {field}: {reason}")]
    InvalidConfig {
        /// The field that failed validation.
        field: &'static str,
        /// Why the value is invalid.
        reason: &'static str,
    },

    /// No matching HackRF was enumerated.
    #[error("no matching HackRF device found")]
    DeviceNotFound,

    /// The USB device could not be opened or its interface claimed.
    #[error("{operation}: {source}")]
    Usb {
        /// What was being attempted.
        operation: &'static str,
        /// The underlying USB error.
        #[source]
        source: nusb::Error,
    },

    /// A vendor control transfer failed.
    #[error("control transfer failed: {0}")]
    ControlTransfer(#[source] nusb::transfer::TransferError),

    /// The device answered a vendor request with something the protocol does not allow.
    #[error("{operation}: {reason}")]
    Protocol {
        /// The request that got the bad answer.
        operation: &'static str,
        /// What was wrong with it.
        reason: &'static str,
    },

    /// The shared USB transport could not be started, or gave up on its own — in either
    /// direction: both halves count transfer errors under the same policy.
    #[error("streaming: {0}")]
    Stream(#[from] StreamError),
}

impl Error {
    pub(crate) const fn invalid_config(field: &'static str, reason: &'static str) -> Self {
        Self::InvalidConfig { field, reason }
    }

    pub(crate) const fn protocol(operation: &'static str, reason: &'static str) -> Self {
        Self::Protocol { operation, reason }
    }

    pub(crate) const fn usb(operation: &'static str, source: nusb::Error) -> Self {
        Self::Usb { operation, source }
    }
}
