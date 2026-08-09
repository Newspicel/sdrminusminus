//! `sdrmm-hackrf-driver` — HackRF One / Jawbreaker / rad1o over pure-Rust USB (PLAN §3, §17).
//!
//! Vendored from **`hackrf-nusb` 0.3.0** (upstream `bastibl/hackrf-nusb`), rebuilt on
//! `sdrmm-usb-stream`. This is a fork we own; there is no upstream contribution and the version
//! above is the fork point, deliberately pinned rather than tracked
//! (`PLAN-NATIVE-DRIVERS.md` §3).
//!
//! What changed from upstream, and why:
//!
//! - **Streaming moved out.** The transfer queue and the error policy are `sdrmm-usb-stream`,
//!   shared with the RTL driver. Upstream's `checked_completion` propagated *any* errored
//!   completion — cancellations included — straight into `close()`, so a single transfer fault
//!   killed the stream with no counting and no threshold.
//! - **Bytes, not samples.** Upstream converted cs8 to `Complex32` inside the transport; that
//!   now happens at the device edge, beside the RTL2832U's own (and different) conversion.
//! - **Blocking only.** The `MaybeFuture` layer, the async stream and the `wasm32` paths are
//!   gone — every consumer in this project drives the radio from a dedicated capture thread.
//! - **RX only.** The TX half and the half-duplex lifecycle state machine that existed to
//!   arbitrate between the two are not ported: PLAN §1 keeps TX declared and unimplemented
//!   through the RX phases, and unused transmit machinery is not worth carrying.

mod commands;
mod config;
mod control;
mod device;
mod discovery;
mod error;
mod types;

pub use config::Config;
pub use device::{Device, RX_TRANSFER_SIZE};
pub use discovery::DeviceDescriptor;
pub use error::{Error, Result};
pub use types::{BoardId, DeviceInfo};
