use nusb::transfer::TransferError;

#[cfg(target_os = "macos")]
const IOKIT_NOT_RESPONDING: u32 = 0xe000_02ed;
#[cfg(target_os = "macos")]
const IOKIT_PORT_WAS_SUSPENDED: u32 = 0xe000_4052;

#[must_use]
pub fn is_disconnect(error: &TransferError) -> bool {
    match error {
        TransferError::Disconnected => true,
        #[cfg(target_os = "macos")]
        TransferError::Unknown(code) => {
            matches!(*code, IOKIT_NOT_RESPONDING | IOKIT_PORT_WAS_SUSPENDED)
        }
        _ => false,
    }
}

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
            Self::Transfers { source, .. } => is_disconnect(source),
            Self::Endpoint(e) => e.kind() == nusb::ErrorKind::Disconnected,
            Self::Spawn(_) | Self::Config(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, StreamError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unplug_is_a_disconnect() {
        assert!(is_disconnect(&TransferError::Disconnected));
    }

    #[test]
    fn a_stall_is_not_a_disconnect() {
        assert!(!is_disconnect(&TransferError::Stall));
        assert!(!is_disconnect(&TransferError::Cancelled));
        assert!(!is_disconnect(&TransferError::Fault));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_radio_that_stopped_answering_the_bus_is_a_disconnect() {
        assert!(is_disconnect(&TransferError::Unknown(IOKIT_NOT_RESPONDING)));
        assert!(is_disconnect(&TransferError::Unknown(
            IOKIT_PORT_WAS_SUSPENDED
        )));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn another_os_specific_code_is_not_a_disconnect() {
        assert!(!is_disconnect(&TransferError::Unknown(0xe000_404f)));
    }
}
