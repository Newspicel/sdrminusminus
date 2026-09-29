use std::time::Duration;

use nusb::{
    MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError},
};

use super::error::{Error, Result};

const TIMEOUT: Duration = Duration::from_millis(300);
const STALL_ATTEMPTS: u32 = 4;
const STALL_PAUSE: Duration = Duration::from_millis(2);
const INTERFACE: u8 = 0;

pub(crate) trait Transport {
    fn read(&self, value: u16, index: u16, len: u16) -> Result<Vec<u8>, TransferError>;
    fn write(&self, value: u16, index: u16, data: &[u8]) -> Result<(), TransferError>;
}

#[derive(Clone)]
pub(crate) struct UsbControl {
    interface: nusb::Interface,
    _device: nusb::Device,
}

impl UsbControl {
    pub(crate) fn open(info: &nusb::DeviceInfo) -> Result<Self> {
        let node = format!("{}/{}", info.bus_id(), info.device_address());
        let device = info.open().wait().map_err(|source| Error::Open {
            node: node.clone(),
            source,
        })?;
        let interface = device
            .detach_and_claim_interface(INTERFACE)
            .wait()
            .map_err(|source| Error::Claim { node, source })?;
        Ok(Self {
            interface,
            _device: device,
        })
    }

    pub(crate) fn interface(&self) -> &nusb::Interface {
        &self.interface
    }
}

impl Transport for UsbControl {
    fn read(&self, value: u16, index: u16, len: u16) -> Result<Vec<u8>, TransferError> {
        retry_stalls(
            || {
                self.interface
                    .control_in(
                        ControlIn {
                            control_type: ControlType::Vendor,
                            recipient: Recipient::Device,
                            request: 0,
                            value,
                            index,
                            length: len,
                        },
                        TIMEOUT,
                    )
                    .wait()
            },
            || std::thread::sleep(STALL_PAUSE),
        )
    }

    fn write(&self, value: u16, index: u16, data: &[u8]) -> Result<(), TransferError> {
        retry_stalls(
            || {
                self.interface
                    .control_out(
                        ControlOut {
                            control_type: ControlType::Vendor,
                            recipient: Recipient::Device,
                            request: 0,
                            value,
                            index,
                            data,
                        },
                        TIMEOUT,
                    )
                    .wait()
            },
            || std::thread::sleep(STALL_PAUSE),
        )
    }
}

fn retry_stalls<T>(
    mut attempt: impl FnMut() -> Result<T, TransferError>,
    mut pause: impl FnMut(),
) -> Result<T, TransferError> {
    let mut tries = 1;
    loop {
        match attempt() {
            Err(TransferError::Stall) if tries < STALL_ATTEMPTS => {
                pause();
                tries += 1;
            }
            outcome => return outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn run(outcomes: &[Result<u8, TransferError>]) -> (Result<u8, TransferError>, usize, usize) {
        let calls = Cell::new(0);
        let pauses = Cell::new(0);
        let result = retry_stalls(
            || {
                let at = calls.get();
                calls.set(at + 1);
                outcomes[at]
            },
            || pauses.set(pauses.get() + 1),
        );
        (result, calls.get(), pauses.get())
    }

    #[test]
    fn a_stall_is_retried_until_it_goes_through() {
        let stall = Err(TransferError::Stall);
        assert_eq!(run(&[stall, stall, Ok(7)]), (Ok(7), 3, 2));
    }

    #[test]
    fn four_stalls_give_up_with_the_stall() {
        let stall = Err(TransferError::Stall);
        assert_eq!(run(&[stall; 5]), (stall, 4, 3));
    }

    #[test]
    fn other_errors_are_not_retried() {
        for error in [
            TransferError::Disconnected,
            TransferError::Cancelled,
            TransferError::Fault,
        ] {
            assert_eq!(run(&[Err(error), Ok(1)]), (Err(error), 1, 0));
        }
    }

    #[test]
    fn success_takes_one_attempt() {
        assert_eq!(run(&[Ok(3)]), (Ok(3), 1, 0));
    }
}
