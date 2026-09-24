use sdrmm_usb_stream::{StreamError, is_disconnect};

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("RTL-SDR device not found")]
    DeviceNotFound,

    #[error("failed to open USB device: {0}")]
    OpenFailed(#[source] nusb::Error),

    #[error("failed to claim USB interface: {0}")]
    ClaimFailed(#[source] nusb::Error),

    #[error("control transfer failed on {op}: {source}")]
    ControlTransfer {
        op: String,
        #[source]
        source: nusb::transfer::TransferError,
    },

    #[error("{what} returned {got} bytes")]
    ShortResponse { what: &'static str, got: usize },

    #[error("no supported tuner found (checked R820T at 0x34, R828D at 0x74)")]
    TunerNotFound,

    #[error("the {0} tuner is not supported, only R820T and R828D")]
    UnsupportedTuner(&'static str),

    #[error("PLL failed to lock at {freq_hz} Hz")]
    PllLockFailed { freq_hz: u64 },

    #[error("invalid sample rate {rate} Hz (valid: 225001-300000 or 900001-3200000)")]
    InvalidSampleRate { rate: u32 },

    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    #[error("streaming: {0}")]
    Stream(#[from] StreamError),
}

impl Error {
    /// Whether the radio answered nothing because it is no longer on the bus.
    pub(crate) fn is_disconnected(&self) -> bool {
        match self {
            Self::Stream(error) => error.is_disconnected(),
            Self::ControlTransfer { source, .. } => is_disconnect(source),
            Self::OpenFailed(error) | Self::ClaimFailed(error) => {
                error.kind() == nusb::ErrorKind::Disconnected
            }
            _ => false,
        }
    }

    /// Whether the operating system refused this user the device node.
    pub(crate) fn is_permission_denied(&self) -> bool {
        self.usb_kind() == Some(nusb::ErrorKind::PermissionDenied)
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.usb_kind() == Some(nusb::ErrorKind::Busy)
    }

    pub(crate) fn is_wrong_driver(&self) -> bool {
        self.usb_kind() == Some(nusb::ErrorKind::Unsupported)
    }

    pub(crate) fn is_missing(&self) -> bool {
        self.usb_kind() == Some(nusb::ErrorKind::NotFound)
    }

    fn usb_kind(&self) -> Option<nusb::ErrorKind> {
        match self {
            Self::OpenFailed(error) | Self::ClaimFailed(error) => Some(error.kind()),
            _ => None,
        }
    }
}
