//! `sdrmm-rtl-driver` — RTL2832U + R820T/R828D over pure-Rust USB (PLAN §3, §17).
//!
//! Vendored from **`rs-rtl` 0.4.2** (upstream `xoolive/desperado`, sha
//! `32c76e10c5b0c852cdc2550c5368b2fc0af8c611`), rebuilt on `sdrmm-usb-stream`. This is a fork we
//! own; there is no upstream contribution and the version above is the fork point, deliberately
//! pinned rather than tracked (`PLAN-NATIVE-DRIVERS.md` §3).
//!
//! What changed from upstream, and why:
//!
//! - **Streaming moved out.** The transfer queue, the error policy and the sample handoff are
//!   `sdrmm-usb-stream`, shared with the HackRF driver. Upstream's policy declared a transient
//!   stall a disconnect after five errored completions with fifteen transfers in flight, which
//!   cost ~9 s of dead air every time an antenna was touched.
//! - **`StreamControl` and the in-thread retune machinery are gone.** Their commands were
//!   fire-and-forget, so a rejected retune looked applied; the backend already bypassed them and
//!   drives every setter through the control endpoint instead.
//! - **No silent zeroes.** Upstream turned a short control response into a `0` register value;
//!   it is now an error.
//! - **Crystal (ppm) correction added.** Upstream had no public API for it and no way to reach
//!   an opened device's registers, so the backend had to reject the setting; it is now
//!   librtlsdr-shaped `set_freq_correction`, correcting the resampler and the tuner both.
//!
//! The register-level work — `device` (RTL2832U registers and the I2C bridge) and `tuner`
//! (R82xx programming) — is upstream's and is kept as the valuable part.

mod device;
mod error;
mod rtlsdr;
mod tuner;

pub use error::{Error, Result};
pub use rtlsdr::{
    BoardVariant, DEF_RTL_XTAL_FREQ, DeviceDescriptor, DeviceDescriptors, DeviceId, MAX_PPM,
    RTL_USB_PIDS, RTL_USB_VID, RtlSdr, TRANSFER_BUF_SIZE,
};
pub use tuner::{GAIN_VALUES, TunerType};
