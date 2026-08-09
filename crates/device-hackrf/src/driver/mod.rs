//! The HackRF driver itself: USB enumeration, the vendor control protocol, the validated
//! configuration the radio holds, and the RX stream lifecycle.
//!
//! Everything here is device-level and knows nothing about the wire capability model — `caps` is
//! the only place that translates. Streaming is not here either: the transfer queue and the USB
//! error policy are `sdrmm-usb-stream`, shared with the RTL-SDR backend, because getting that
//! policy wrong was the defect this driver exists to fix (PLAN §17,
//! `PLAN-NATIVE-DRIVERS.md`).
//!
//! RX only. PLAN §1 keeps the TX half declared and unimplemented through the RX phases, and the
//! half-duplex arbitration a TX path needs is not worth carrying until there is one.

mod commands;
mod config;
mod control;
mod discovery;
mod error;
mod radio;
mod types;

pub(crate) use config::Config;
pub(crate) use discovery::DeviceDescriptor;
pub(crate) use error::Error;
pub(crate) use radio::{HackRf, RX_TRANSFER_SIZE};
