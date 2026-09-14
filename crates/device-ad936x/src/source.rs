use std::{fmt, sync::Arc};

use sdrmm_device::{DeviceError, net::Endpoint};

use crate::iio::{MIN_COUPLES, NetTransport, Transport, UsbBus};

/// How many conversations a socket-served radio is given at once: one for control, one for each
/// direction's buffer, and room to spare. The radio itself is the limit that matters.
const NET_LINKS: usize = 8;

/// Where a radio's IIOD lives, and how to start another conversation with it.
///
/// Attributes and buffers travel on separate conversations, because a refill holds its own until
/// it is finished and a retune must not wait behind it.
#[derive(Clone)]
pub(crate) enum Source {
    Net(Endpoint),
    Usb(Arc<UsbBus>),
}

impl Source {
    pub(crate) fn open(&self) -> Result<Arc<dyn Transport>, DeviceError> {
        Ok(match self {
            Self::Net(endpoint) => Arc::new(NetTransport::connect(endpoint)?),
            Self::Usb(bus) => Arc::new(bus.claim()?),
        })
    }

    /// Conversations this radio can hold at once, which is what decides whether it can transmit
    /// while it receives.
    pub(crate) fn links(&self) -> usize {
        match self {
            Self::Net(_) => NET_LINKS,
            Self::Usb(bus) => bus.couples(),
        }
    }

    /// Whether a buffer in each direction can run alongside the control conversation.
    pub(crate) fn full_duplex(&self) -> bool {
        self.links() > MIN_COUPLES
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Net(endpoint) => write!(f, "ip:{endpoint}"),
            Self::Usb(bus) => write!(f, "usb:{bus:?}"),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Net(endpoint) => write!(f, "{endpoint}"),
            Self::Usb(_) => f.write_str("usb"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_served_radio_has_room_for_both_directions_at_once() {
        let source = Source::Net(Endpoint::parse("radio.local", 30_431).expect("endpoint"));
        assert!(source.links() > MIN_COUPLES);
        assert!(source.full_duplex());
        assert_eq!(source.to_string(), "radio.local:30431");
    }
}
