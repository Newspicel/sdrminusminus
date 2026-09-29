use nusb::transfer::TransferError;
use sdrmm_device::DeviceError;
use sdrmm_usb_stream::{StreamError, is_busy, is_disconnect};

use super::chip::Access;

pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("list USB devices: {0}")]
    Scan(#[source] nusb::Error),

    #[error("no RTL-SDR at index {0}")]
    NotFound(usize),

    #[error("open USB device {node}: {source}")]
    Open {
        node: String,
        #[source]
        source: nusb::Error,
    },

    #[error("claim interface 0 of USB device {node}: {source}")]
    Claim {
        node: String,
        #[source]
        source: nusb::Error,
    },

    #[error("control transfer failed on {access}: {source}")]
    Control {
        access: Access,
        #[source]
        source: TransferError,
    },

    #[error("short response on {access}: {got} of {wanted} bytes")]
    Short {
        access: Access,
        wanted: usize,
        got: usize,
    },

    #[error("no supported tuner answered (checked R820T at 0x34, R828D at 0x74)")]
    NoTuner,

    #[error("{0} tuner found, only R820T and R828D work")]
    ForeignTuner(&'static str),

    #[error("PLL failed to lock at {lo_hz} Hz ({fault})")]
    Pll { lo_hz: u64, fault: PllFault },

    #[error("sample rate {0} Hz outside 225001-300000 and 900001-3200000")]
    SampleRate(u32),

    #[error("invalid parameter: {0}")]
    Invalid(Invalid),

    #[error("stream: {0}")]
    Stream(#[from] StreamError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PllFault {
    #[error("no valid divider")]
    NoDivider,
    #[error("nint too large")]
    NintTooLarge,
    #[error("no lock")]
    NoLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Invalid {
    #[error("ppm {0} outside ±488")]
    Ppm(i32),
    #[error("direct sampling center {0} Hz outside 0-14400000 Hz")]
    DirectCenter(u32),
    #[error("tuner is bypassed")]
    TunerBypassed,
    #[error("this board has no direct sampling")]
    NoDirectSampling,
    #[error("GPIO pin {0} outside 0-7")]
    Pin(u8),
    #[error("tuner register 0x{0:02x} is read-only")]
    TunerRegister(u8),
}

impl From<Invalid> for Error {
    fn from(invalid: Invalid) -> Self {
        Self::Invalid(invalid)
    }
}

#[cfg(target_os = "macos")]
const IOKIT_NOT_RESPONDING: u32 = 0xe000_02ed;
#[cfg(target_os = "macos")]
const IOKIT_PORT_SUSPENDED: u32 = 0xe000_4052;

#[cfg(target_os = "macos")]
fn os_says_gone(error: &nusb::Error) -> bool {
    matches!(
        error.os_error(),
        Some(IOKIT_NOT_RESPONDING | IOKIT_PORT_SUSPENDED)
    )
}

#[cfg(not(target_os = "macos"))]
fn os_says_gone(_error: &nusb::Error) -> bool {
    false
}

fn usb_gone(error: &nusb::Error) -> bool {
    error.kind() == nusb::ErrorKind::Disconnected || os_says_gone(error)
}

impl Error {
    pub(crate) fn is_disconnected(&self) -> bool {
        match self {
            Self::Control { source, .. } => is_disconnect(source),
            Self::Open { source, .. } | Self::Claim { source, .. } => usb_gone(source),
            Self::Stream(error) => error.is_disconnected(),
            _ => false,
        }
    }

    fn usb_error(&self) -> Option<&nusb::Error> {
        match self {
            Self::Open { source, .. } | Self::Claim { source, .. } | Self::Scan(source) => {
                Some(source)
            }
            _ => None,
        }
    }

    fn usb_kind_is(&self, kind: nusb::ErrorKind) -> bool {
        self.usb_error().is_some_and(|error| error.kind() == kind)
    }
}

impl From<Error> for DeviceError {
    fn from(error: Error) -> Self {
        if error.is_disconnected() {
            return Self::Disconnected("radio unplugged".to_owned());
        }
        let text = error.to_string();
        if error.usb_kind_is(nusb::ErrorKind::PermissionDenied) {
            return Self::PermissionDenied(text);
        }
        if error.usb_error().is_some_and(is_busy) {
            return Self::InUse(text);
        }
        if error.usb_kind_is(nusb::ErrorKind::NotFound) {
            return Self::NotFound(text);
        }
        if error.usb_kind_is(nusb::ErrorKind::Unsupported) {
            return Self::Unsupported(format!("{text}: install the WinUSB driver with Zadig"));
        }
        match error {
            Error::NotFound(_) => Self::NotFound(text),
            Error::SampleRate(_)
            | Error::Invalid(_)
            | Error::Pll { .. }
            | Error::ForeignTuner(_)
            | Error::NoTuner => Self::Unsupported(text),
            _ => Self::Io(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::chip::{Access, Dir};

    fn control(source: TransferError) -> Error {
        Error::Control {
            access: Access::Demod {
                page: 1,
                reg: 0x01,
                dir: Dir::Write,
            },
            source,
        }
    }

    #[test]
    fn a_control_error_names_what_it_was_for() {
        assert_eq!(
            control(TransferError::Stall).to_string(),
            "control transfer failed on demod write 1:0x01: endpoint stalled"
        );
    }

    #[test]
    fn an_unplugged_dongle_is_one_plain_disconnect() {
        let mapped = DeviceError::from(control(TransferError::Disconnected));
        assert!(
            matches!(&mapped, DeviceError::Disconnected(text) if text == "radio unplugged"),
            "{mapped}"
        );
    }

    #[test]
    fn a_stall_is_an_io_error_not_a_disconnect() {
        assert!(matches!(
            DeviceError::from(control(TransferError::Stall)),
            DeviceError::Io(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_dongle_that_stopped_answering_reads_as_gone() {
        assert!(matches!(
            DeviceError::from(control(TransferError::Unknown(IOKIT_NOT_RESPONDING))),
            DeviceError::Disconnected(_)
        ));
        assert!(matches!(
            DeviceError::from(control(TransferError::Unknown(IOKIT_PORT_SUSPENDED))),
            DeviceError::Disconnected(_)
        ));
    }

    #[test]
    fn settings_the_chip_cannot_take_are_unsupported() {
        for error in [
            Error::SampleRate(500_000),
            Error::Invalid(Invalid::Ppm(500)),
            Error::Pll {
                lo_hz: 27_570_000,
                fault: PllFault::NoDivider,
            },
            Error::ForeignTuner("Elonics E4000"),
        ] {
            assert!(matches!(
                DeviceError::from(error),
                DeviceError::Unsupported(_)
            ));
        }
    }

    #[test]
    fn a_pll_error_carries_the_lo() {
        let error = Error::Pll {
            lo_hz: 1_800_000_000,
            fault: PllFault::NoDivider,
        };
        assert_eq!(
            error.to_string(),
            "PLL failed to lock at 1800000000 Hz (no valid divider)"
        );
    }
}
