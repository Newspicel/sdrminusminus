//! The HackRF driver itself: USB enumeration, the vendor control protocol, the validated
//! configuration the radio holds, and the RX stream lifecycle.
//!
//! Everything here is device-level and knows nothing about the wire capability model — `caps` is
//! the only place that translates. Streaming is not here either: the transfer queue and the USB
//! error policy are `sdrmm-usb-stream`, shared with the RTL-SDR backend, because getting that
//! policy wrong was the defect this driver exists to fix (PLAN §17,
//! `PLAN-NATIVE-DRIVERS.md`).
//!
//! The radio is half duplex — one direction at a time — but nothing here arbitrates that: the
//! rule is `sdrmm-device`'s [`DuplexState`](sdrmm_device::DuplexState), shared with every other
//! radio that has the same constraint, and this layer only carries out what it is told. Transmit
//! reaches `SdrDevice::tx_start` and stops there: PLAN §12a gates every application-level TX
//! feature behind an explicit authorized-use switch that has not been built, so `Capabilities`
//! still reports `tx_capable: false` and no engine, server or UI path calls it.

mod commands;
mod config;
mod control;
mod discovery;
mod error;
mod radio;
mod tx;
mod types;

pub(crate) use config::Config;
pub(crate) use discovery::DeviceDescriptor;
pub(crate) use error::Error;
pub(crate) use radio::{HackRf, RX_TRANSFER_SIZE};
pub(crate) use tx::{BurstQueue, TX_TRANSFER_SIZE};
