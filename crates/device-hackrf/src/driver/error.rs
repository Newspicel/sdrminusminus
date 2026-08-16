use sdrmm_usb_stream::StreamError;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid configuration for {field}: {reason}")]
    InvalidConfig {
        field: &'static str,
        reason: &'static str,
    },

    #[error("no matching HackRF device found")]
    DeviceNotFound,

    #[error("{operation}: {source}")]
    Usb {
        operation: &'static str,
        #[source]
        source: nusb::Error,
    },

    #[error("control transfer failed: {0}")]
    ControlTransfer(#[source] nusb::transfer::TransferError),

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

    /// Whether the radio answered nothing because it is no longer on the bus.
    pub(crate) fn is_disconnected(&self) -> bool {
        match self {
            Self::Stream(error) => error.is_disconnected(),
            Self::ControlTransfer(error) => *error == nusb::transfer::TransferError::Disconnected,
            Self::Usb { source, .. } => source.kind() == nusb::ErrorKind::Disconnected,
            Self::InvalidConfig { .. } | Self::DeviceNotFound | Self::Protocol { .. } => false,
        }
    }
}
