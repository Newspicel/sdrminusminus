//! The seam between the transfer pump and the USB stack.
//!
//! Everything the pump needs from an endpoint is behind [`BulkIn`], so the error policy and the
//! queue discipline are exercised against a scripted mock instead of hardware (PLAN §14). The
//! trait is the shape `hackrf-nusb` 0.3.0 introduced, minus its async half.

use std::{ops::Deref, time::Duration};

use nusb::{
    Endpoint, MaybeFuture,
    transfer::{Buffer, Bulk, In, TransferError},
};

use crate::error::Result;

/// One finished transfer: the buffer it filled, how much of it is data, and how it ended.
#[derive(Debug)]
pub struct Completion<B> {
    /// The buffer that was submitted, ready to be resubmitted.
    pub buffer: B,
    /// Bytes received into `buffer`. Meaningless unless `status` is `Ok`.
    pub actual_len: usize,
    /// How the transfer ended.
    pub status: std::result::Result<(), TransferError>,
}

/// A queue of bulk-IN transfers.
///
/// Implementations must keep submitted transfers in submission order and must not block in
/// [`BulkIn::submit`] — the pump submits from the same thread it reads completions on.
pub trait BulkIn: Send + 'static {
    /// Transfer buffer. Reused across transfers, so it derefs to the *received* bytes.
    type Buffer: Deref<Target = [u8]> + Send;

    /// Clear a stall and reset the data toggle. Called once with nothing in flight, which is
    /// the only state `nusb` documents as safe for it.
    fn clear_halt(&mut self) -> Result<()>;
    /// Allocate a transfer buffer of `len` bytes.
    fn allocate(&self, len: usize) -> Self::Buffer;
    /// Queue `buffer` for another transfer.
    fn submit(&mut self, buffer: Self::Buffer);
    /// Transfers currently in flight.
    fn pending(&self) -> usize;
    /// Block until the oldest transfer finishes, or `timeout` expires. Must only be called
    /// with at least one transfer pending.
    fn wait_next_complete(&mut self, timeout: Duration) -> Option<Completion<Self::Buffer>>;
    /// Cancel every transfer in flight. Each still completes, with `Cancelled`.
    fn cancel_all(&mut self);
}

/// [`BulkIn`] over a real `nusb` endpoint.
#[derive(Debug)]
pub struct NusbBulkIn {
    endpoint: Endpoint<Bulk, In>,
}

impl NusbBulkIn {
    /// Claim `address` on `interface` as the stream's data endpoint.
    pub fn open(interface: &nusb::Interface, address: u8) -> Result<Self> {
        Ok(Self {
            endpoint: interface.endpoint::<Bulk, In>(address)?,
        })
    }
}

impl BulkIn for NusbBulkIn {
    type Buffer = Buffer;

    fn clear_halt(&mut self) -> Result<()> {
        self.endpoint.clear_halt().wait()?;
        Ok(())
    }

    fn allocate(&self, len: usize) -> Self::Buffer {
        self.endpoint.allocate(len)
    }

    fn submit(&mut self, buffer: Self::Buffer) {
        self.endpoint.submit(buffer);
    }

    fn pending(&self) -> usize {
        self.endpoint.pending()
    }

    fn wait_next_complete(&mut self, timeout: Duration) -> Option<Completion<Self::Buffer>> {
        self.endpoint
            .wait_next_complete(timeout)
            .map(|completion| Completion {
                buffer: completion.buffer,
                actual_len: completion.actual_len,
                status: completion.status,
            })
    }

    fn cancel_all(&mut self) {
        self.endpoint.cancel_all();
    }
}
