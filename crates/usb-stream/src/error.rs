use nusb::transfer::TransferError;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("usb transfer failed {attempts} times in a row: {source}")]
    Transfers {
        attempts: u32,
        source: TransferError,
    },
    #[error("usb endpoint: {0}")]
    Endpoint(#[from] nusb::Error),
    #[error("spawn usb transfer pump: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("stream config: {0}")]
    Config(&'static str),
}

impl StreamError {
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        match self {
            Self::Transfers { source, .. } => *source == TransferError::Disconnected,
            Self::Endpoint(e) => e.kind() == nusb::ErrorKind::Disconnected,
            Self::Spawn(_) | Self::Config(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, StreamError>;
