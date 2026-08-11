//! `sdrmm-device-net` — receivers reached over a network rather than a bus (PLAN §6): the
//! **rtl_tcp** and **SpyServer** client backends, pure Rust over `std::net`, no new dependency of
//! any kind.
//!
//! A radio on the other end of a socket is the same radio to everything above this crate: it
//! implements the same [`SdrDevice`](sdrmm_device::SdrDevice) trait, streams through the same
//! [`Capture`](sdrmm_device::Capture) supervisor, and renders from the same capability model. Two
//! things about it are genuinely different, and both are handled here rather than upstream.
//!
//! **It is named, not discovered.** Neither protocol has any discovery, so the only thing that can
//! produce `10.0.0.5:1234` is an operator typing it. That arrives through
//! [`DeviceDriver::resolve`](sdrmm_device::DeviceDriver::resolve), and a driver that adopts an
//! endpoint reports it from `probe` from then on — which it must, because everything above works
//! in probe results: a device set whose device leaves the probe list is faulted, and a faulted one
//! whose device comes back is re-opened. Those two are exactly the behaviour a remote wants, so a
//! server that reboots costs a reconnect and not a lost radio.
//!
//! **A dropped connection is a recoverable stream failure, not a lost device.** The capture
//! supervisor's tier-1 restart re-dials and replays every setting, which is why neither backend
//! reports a failure as fatal: unlike a device that left the USB bus, a remote can always be
//! called back.
//!
//! One security note, since this is the one backend whose "open a device" reaches outward: the
//! endpoint comes from the caller, so an authorized client can make the server dial an arbitrary
//! host and port. That is the same trust boundary the rest of the API sits behind (`crates/server`
//! is LAN-trusted by default, and every route is behind the same auth), and it is the whole point
//! of the feature — but it is worth knowing that this route exists.

mod adopted;
mod endpoint;
mod rtltcp;
mod socket;
mod spyserver;

pub use endpoint::Endpoint;
pub use rtltcp::{RtlTcpDevice, RtlTcpDriver};
pub use spyserver::{SpyServerDevice, SpyServerDriver};
