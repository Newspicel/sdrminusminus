//! `sdrmm-usb-stream` — the bulk USB transport the native SDR backends share, in both
//! directions (PLAN §3).
//!
//! Transport only: raw bytes in, raw bytes out. It knows nothing about registers, tuners or
//! sample formats — conversion belongs at the device edge, where the two radios genuinely
//! differ (signed cs8 on the HackRF, unsigned with a measured DC offset on the RTL2832U).
//!
//! It exists because the two crates this project vendored each hand-rolled their own USB
//! transfer-error policy and both were wrong: `rs-rtl` counted cancellations as errors and tore
//! the device down after five, and `hackrf-nusb` closed the whole stream on the first errored
//! completion of any kind. The correct policy is librtlsdr's, is fifteen years old, and now
//! lives once, in [`TransferPolicy`], behind unit tests.
//!
//! ```no_run
//! use sdrmm_usb_stream::{NusbBulkIn, StreamConfig, start};
//!
//! # fn example(interface: &nusb::Interface) -> Result<(), sdrmm_usb_stream::StreamError> {
//! let endpoint = NusbBulkIn::open(interface, 0x81)?;
//! let stream = start(endpoint, StreamConfig::new(16_384, "example-rx"))?;
//! while let Ok(block) = stream.recv_timeout(std::time::Duration::from_millis(100)) {
//!     println!("{} bytes", block.len());
//! }
//! # Ok(())
//! # }
//! ```

mod bulk;
mod error;
mod policy;
mod stream;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
mod tx;

pub use bulk::{BulkIn, Completion, NusbBulkIn};
pub use error::{Result, StreamError};
pub use policy::{Action, TransferPolicy};
pub use stream::{Block, RxStream, Stopper, StreamConfig, StreamingStats, start};
pub use tx::{BulkOut, NusbBulkOut, OutCompletion, TxConfig, TxQueue, TxStats};
