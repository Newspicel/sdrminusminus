use sdrmm_usb_stream::{StreamError, is_disconnect};

use super::commands::VendorRequest;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid configuration for {field}: {reason}")]
    InvalidConfig {
        field: &'static str,
        reason: &'static str,
    },

    #[error("no matching Airspy device found")]
    DeviceNotFound,

    #[error("{operation}: {source}")]
    Usb {
        operation: &'static str,
        #[source]
        source: nusb::Error,
    },

    #[error("{request:?} control transfer failed: {source}")]
    ControlTransfer {
        request: VendorRequest,
        #[source]
        source: nusb::transfer::TransferError,
    },

    #[error("{operation}: {reason}")]
    Protocol {
        operation: &'static str,
        reason: &'static str,
    },

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

    pub(crate) fn is_disconnected(&self) -> bool {
        match self {
            Self::Stream(error) => error.is_disconnected(),
            Self::ControlTransfer { source, .. } => is_disconnect(source),
            Self::Usb { source, .. } => source.kind() == nusb::ErrorKind::Disconnected,
            Self::InvalidConfig { .. } | Self::DeviceNotFound | Self::Protocol { .. } => false,
        }
    }

    pub(crate) fn is_permission_denied(&self) -> bool {
        match self {
            Self::Usb { source, .. } => source.kind() == nusb::ErrorKind::PermissionDenied,
            Self::Stream(_)
            | Self::ControlTransfer { .. }
            | Self::InvalidConfig { .. }
            | Self::DeviceNotFound
            | Self::Protocol { .. } => false,
        }
    }
}
