//! The bulk-OUT half: queueing transmit buffers to the radio.
//!
//! The mirror of [`crate::stream`], and here for the same reason: the transfer queue, the buffer
//! recycling and above all the *error policy* are properties of the USB path, not of the radio
//! on the end of it. The first radio to need this was the HackRF; the second one to need it
//! should not have to write it again.
//!
//! Transmit has one rule receive does not: **a failed transfer is never re-sent.** On the receive
//! side an errored transfer carried nothing, so resubmitting it costs nothing and keeps the queue
//! full. Here it carried samples that were meant for a specific moment, and putting them back on
//! the wire behind everything queued after them would corrupt the burst more thoroughly than the
//! gap it is trying to repair. So [`TransferPolicy`]'s counting is used — cancellations never
//! count, genuine errors do, the threshold is the queue depth, any success clears it — while its
//! `Resubmit` verdict is read here as "tolerate and carry on".
//!
//! What is *not* here is what a burst means. A radio that marks the end of one with a zero-filled
//! transfer (the HackRF does; its firmware uses it to tell a burst that ended on purpose from a
//! host that fell behind) drives that from its own backend through [`TxQueue::submit_zeros`] —
//! this layer knows only that some transfers carry samples and some do not.

use std::time::{Duration, Instant};

use nusb::{
    Endpoint, MaybeFuture,
    transfer::{Buffer, Bulk, Out, TransferError},
};

use crate::{
    error::{Result, StreamError},
    policy::{Action, TransferPolicy},
};

/// One finished transmit transfer, with its buffer back for reuse.
#[derive(Debug)]
pub struct OutCompletion {
    /// The buffer that was submitted.
    pub bytes: Vec<u8>,
    /// How the transfer ended.
    pub status: std::result::Result<(), TransferError>,
}

/// A queue of bulk-OUT transfers.
///
/// The seam exists for the same reason [`crate::BulkIn`]'s does: so the queue discipline and the
/// error policy are exercised against a scripted mock instead of hardware (PLAN §14).
pub trait BulkOut: Send + 'static {
    /// Clear a stall and reset the data toggle, with nothing in flight.
    ///
    /// # Errors
    /// Whatever the USB stack reports.
    fn clear_halt(&mut self) -> Result<()>;
    /// Queue `bytes` for transmission.
    fn submit(&mut self, bytes: Vec<u8>);
    /// Transfers currently in flight.
    fn pending(&self) -> usize;
    /// Block until the oldest transfer finishes, or `timeout` expires. Only called with at least
    /// one transfer pending.
    fn wait_next_complete(&mut self, timeout: Duration) -> Option<OutCompletion>;
    /// Cancel every transfer in flight. Each still completes, with `Cancelled`.
    fn cancel_all(&mut self);
}

/// [`BulkOut`] over a real `nusb` endpoint.
#[derive(Debug)]
pub struct NusbBulkOut {
    endpoint: Endpoint<Bulk, Out>,
}

impl NusbBulkOut {
    /// Claim `address` on `interface` as the transmit endpoint.
    ///
    /// # Errors
    /// [`StreamError::Endpoint`] if the endpoint cannot be claimed.
    pub fn open(interface: &nusb::Interface, address: u8) -> Result<Self> {
        Ok(Self {
            endpoint: interface.endpoint::<Bulk, Out>(address)?,
        })
    }
}

impl BulkOut for NusbBulkOut {
    fn clear_halt(&mut self) -> Result<()> {
        self.endpoint.clear_halt().wait()?;
        Ok(())
    }

    fn submit(&mut self, bytes: Vec<u8>) {
        self.endpoint.submit(Buffer::from(bytes));
    }

    fn pending(&self) -> usize {
        self.endpoint.pending()
    }

    fn wait_next_complete(&mut self, timeout: Duration) -> Option<OutCompletion> {
        self.endpoint
            .wait_next_complete(timeout)
            .map(|completion| OutCompletion {
                bytes: completion.buffer.into_vec(),
                status: completion.status,
            })
    }

    fn cancel_all(&mut self) {
        self.endpoint.cancel_all();
    }
}

/// How a transmit queue is built. `queue_depth` doubles as the error threshold (see
/// [`TransferPolicy::new`]).
#[derive(Clone, Copy, Debug)]
pub struct TxConfig {
    /// Bytes per USB transfer.
    pub transfer_size: usize,
    /// Transfers kept in flight.
    pub queue_depth: usize,
    /// The endpoint takes whole packets only, so a short tail is padded up to one. USB
    /// high-speed bulk is 512; full-speed is 64.
    pub max_packet: usize,
}

impl TxConfig {
    /// Defaults for a high-speed bulk endpoint, with the receive side's queue depth.
    #[must_use]
    pub const fn new(transfer_size: usize) -> Self {
        Self {
            transfer_size,
            queue_depth: 16,
            max_packet: 512,
        }
    }
}

/// Counters for one transmit session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxStats {
    /// Sample bytes accepted from the caller, before padding.
    pub bytes_accepted: u64,
    /// Transfers handed to the USB stack.
    pub buffers_submitted: u64,
    /// Transfers that completed successfully.
    pub buffers_completed: u64,
    /// Transfers that completed with an error, and whose samples never reached the air.
    pub buffers_failed: u64,
    /// Zero-filled transfers, which carry no samples — a burst marker, in the radios that use
    /// one.
    pub zero_buffers: u64,
}

/// A live transmit queue. Bytes in, nothing out — sample conversion is the backend's, beside the
/// receive direction's, because that is where radios differ.
#[derive(Debug)]
pub struct TxQueue<B: BulkOut> {
    bulk_out: B,
    policy: TransferPolicy,
    stats: TxStats,
    config: TxConfig,
    /// Completed buffers, kept so a steady burst allocates nothing.
    spare: Vec<Vec<u8>>,
    stopping: bool,
}

impl<B: BulkOut> TxQueue<B> {
    /// Prepare a queue on an un-stalled endpoint.
    ///
    /// # Errors
    /// [`StreamError::Config`] for a queue that cannot run, or whatever un-stalling the endpoint
    /// reports.
    pub fn start(mut bulk_out: B, config: TxConfig) -> Result<Self> {
        if config.queue_depth == 0 {
            return Err(StreamError::Config("queue_depth must be at least 1"));
        }
        if config.transfer_size == 0 {
            return Err(StreamError::Config("transfer_size must be at least 1"));
        }
        if config.max_packet == 0 {
            return Err(StreamError::Config("max_packet must be at least 1"));
        }
        bulk_out.clear_halt()?;
        Ok(Self {
            bulk_out,
            policy: TransferPolicy::new(u32::try_from(config.queue_depth).unwrap_or(u32::MAX)),
            stats: TxStats::default(),
            config,
            spare: Vec::new(),
            stopping: false,
        })
    }

    /// Counters for this session.
    #[must_use]
    pub const fn stats(&self) -> TxStats {
        self.stats
    }

    /// Transfers currently in flight.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.bulk_out.pending()
    }

    /// The endpoint underneath, for a backend that layers burst semantics on this queue and has
    /// to script it in its own tests.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub const fn endpoint(&self) -> &B {
        &self.bulk_out
    }

    /// Queue `bytes` for transmission, returning how many were accepted.
    ///
    /// A short return means `deadline` passed with the queue full; the caller keeps the rest and
    /// tries again. `None` waits as long as it takes.
    ///
    /// # Errors
    /// [`StreamError::Transfers`] once a whole queue's worth of transfers has failed with no
    /// success in between.
    pub fn write(&mut self, bytes: &[u8], deadline: Option<Instant>) -> Result<usize> {
        let mut accepted = 0;
        for chunk in bytes.chunks(self.config.transfer_size) {
            if !self.make_room(deadline)? {
                return Ok(accepted);
            }
            let mut buffer = self.take_buffer();
            buffer.extend_from_slice(chunk);
            // The endpoint takes whole packets only; a short tail is padded with silence.
            buffer.resize(chunk.len().next_multiple_of(self.config.max_packet), 0);
            self.bulk_out.submit(buffer);
            self.stats.bytes_accepted += chunk.len() as u64;
            self.stats.buffers_submitted += 1;
            accepted += chunk.len();
        }
        Ok(accepted)
    }

    /// Submit one zero-filled transfer of `len` bytes, carrying no samples.
    ///
    /// The caller is responsible for having made room; [`TxQueue::make_room`] is public for that.
    /// Overshooting the queue depth deliberately is sometimes right — a burst marker belongs
    /// adjacent to the burst it terminates, not after a gap.
    pub fn submit_zeros(&mut self, len: usize) {
        let mut buffer = self.take_buffer();
        buffer.resize(len, 0);
        self.bulk_out.submit(buffer);
        self.stats.buffers_submitted += 1;
        self.stats.zero_buffers += 1;
    }

    /// Wait until the queue has room for one more transfer. `false` means the deadline passed.
    ///
    /// # Errors
    /// [`StreamError::Transfers`] if the error threshold is reached while waiting.
    pub fn make_room(&mut self, deadline: Option<Instant>) -> Result<bool> {
        while self.bulk_out.pending() >= self.config.queue_depth {
            if !self.complete_one(deadline)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Wait for every transfer in flight to complete.
    ///
    /// # Errors
    /// [`StreamError::Transfers`] if the error threshold is reached while draining.
    pub fn drain(&mut self) -> Result<()> {
        while self.bulk_out.pending() != 0 {
            self.complete_one(None)?;
        }
        Ok(())
    }

    /// Abandon whatever is in flight. Used when the burst has already failed, where waiting for
    /// a clean drain would only delay releasing the endpoint.
    pub fn abort(&mut self) {
        self.stopping = true;
        self.bulk_out.cancel_all();
        for _ in 0..self.config.queue_depth {
            if self.bulk_out.pending() == 0 {
                break;
            }
            if self.bulk_out.wait_next_complete(DRAIN_POLL).is_none() {
                tracing::debug!("usb tx transfers still pending after cancel");
                break;
            }
        }
    }

    /// Consume one completion. `Ok(false)` means the deadline passed with none available.
    fn complete_one(&mut self, deadline: Option<Instant>) -> Result<bool> {
        let wait = deadline.map_or(DRAIN_POLL, |at| {
            at.saturating_duration_since(Instant::now())
        });
        let Some(completion) = self.bulk_out.wait_next_complete(wait) else {
            return Ok(false);
        };
        let mut bytes = completion.bytes;
        bytes.clear();
        if self.spare.len() < self.config.queue_depth {
            self.spare.push(bytes);
        }
        match self.policy.on_completion(completion.status, self.stopping) {
            Action::Deliver => {
                self.stats.buffers_completed += 1;
                Ok(true)
            }
            Action::Exit => Ok(false),
            Action::Resubmit => {
                // Tolerated, not repeated: see the module comment. The samples are gone, and the
                // gap is the honest outcome.
                self.stats.buffers_failed += 1;
                tracing::warn!(
                    consecutive_errors = self.policy.consecutive_errors(),
                    "usb tx transfer failed; samples dropped"
                );
                Ok(true)
            }
            Action::GiveUp { attempts, error } => {
                self.stats.buffers_failed += 1;
                Err(StreamError::Transfers {
                    attempts,
                    source: error,
                })
            }
        }
    }

    fn take_buffer(&mut self) -> Vec<u8> {
        let mut bytes = self.spare.pop().unwrap_or_default();
        bytes.clear();
        bytes
    }
}

/// How long a drain waits on one completion before checking whether it is making progress.
const DRAIN_POLL: Duration = Duration::from_millis(500);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedBulkOut;

    const TRANSFER_SIZE: usize = 4096;
    const MAX_PACKET: usize = 512;
    const DEPTH: usize = 16;

    fn config() -> TxConfig {
        TxConfig {
            transfer_size: TRANSFER_SIZE,
            queue_depth: DEPTH,
            max_packet: MAX_PACKET,
        }
    }

    fn queue() -> TxQueue<ScriptedBulkOut> {
        TxQueue::start(ScriptedBulkOut::default(), config()).expect("fake endpoint cannot fail")
    }

    #[test]
    fn starting_clears_the_endpoint_stall() {
        assert_eq!(queue().bulk_out.state().clear_halt_calls, 1);
    }

    #[test]
    fn an_unrunnable_config_is_refused() {
        for (config, what) in [
            (
                TxConfig {
                    queue_depth: 0,
                    ..config()
                },
                "queue_depth",
            ),
            (
                TxConfig {
                    transfer_size: 0,
                    ..config()
                },
                "transfer_size",
            ),
            (
                TxConfig {
                    max_packet: 0,
                    ..config()
                },
                "max_packet",
            ),
        ] {
            let error = TxQueue::start(ScriptedBulkOut::default(), config).expect_err(what);
            assert!(matches!(error, StreamError::Config(_)), "{what}: {error}");
        }
    }

    #[test]
    fn a_short_burst_is_padded_up_to_one_whole_packet() {
        let mut queue = queue();
        assert_eq!(queue.write(&[1, 2, 3, 4], None).unwrap(), 4);
        assert_eq!(queue.bulk_out.submitted(), vec![MAX_PACKET]);
        assert_eq!(queue.stats().bytes_accepted, 4);
        assert_eq!(queue.stats().zero_buffers, 0);
    }

    #[test]
    fn a_burst_longer_than_one_transfer_is_split() {
        let mut queue = queue();
        let bytes = vec![7u8; TRANSFER_SIZE + MAX_PACKET];
        assert_eq!(queue.write(&bytes, None).unwrap(), bytes.len());
        assert_eq!(queue.bulk_out.submitted(), vec![TRANSFER_SIZE, MAX_PACKET]);
    }

    #[test]
    fn a_zero_filled_transfer_carries_no_samples() {
        let mut queue = queue();
        queue.submit_zeros(1024);
        assert_eq!(queue.bulk_out.submitted(), vec![1024]);
        assert_eq!(queue.stats().bytes_accepted, 0);
        assert_eq!(queue.stats().zero_buffers, 1);
        assert_eq!(queue.stats().buffers_submitted, 1);
    }

    #[test]
    fn a_full_queue_short_returns_once_the_deadline_passes() {
        let mut queue = queue();
        queue.bulk_out.state().starve = true;
        let bytes = vec![0u8; TRANSFER_SIZE];
        for _ in 0..DEPTH {
            assert_eq!(
                queue.write(&bytes, Some(Instant::now())).unwrap(),
                bytes.len()
            );
        }
        assert_eq!(
            queue.write(&bytes, Some(Instant::now())).unwrap(),
            0,
            "a full queue accepts nothing rather than blocking"
        );
    }

    #[test]
    fn draining_waits_for_every_transfer() {
        let mut queue = queue();
        queue.write(&[1, 2], None).unwrap();
        queue.submit_zeros(64);
        queue.drain().unwrap();
        assert_eq!(queue.pending(), 0);
        assert_eq!(queue.stats().buffers_completed, 2);
    }

    /// The receive side resubmits an errored transfer; this side must not — the samples were
    /// meant for a moment that has passed, and re-sending them behind the queue would corrupt
    /// the burst worse than the gap.
    #[test]
    fn a_failed_transfer_is_dropped_rather_than_re_sent() {
        let mut queue = queue();
        queue.bulk_out.fail_next(1, TransferError::Fault);
        let bytes = vec![0u8; TRANSFER_SIZE];
        for _ in 0..=DEPTH {
            queue.write(&bytes, None).unwrap();
        }
        assert_eq!(queue.stats().buffers_failed, 1);
        // One submission per write and nothing re-sent.
        assert_eq!(queue.bulk_out.submitted().len(), DEPTH + 1);
    }

    #[test]
    fn a_full_queue_of_failures_ends_the_burst() {
        let mut queue = queue();
        queue.bulk_out.fail_next(DEPTH, TransferError::Fault);
        let bytes = vec![0u8; TRANSFER_SIZE];
        let error = loop {
            match queue.write(&bytes, None) {
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(matches!(
            error,
            StreamError::Transfers { attempts, source: TransferError::Fault }
                if attempts as usize == DEPTH
        ));
    }

    #[test]
    fn aborting_cancels_and_collects_every_transfer() {
        let mut queue = queue();
        queue.write(&[1, 2], None).unwrap();
        queue.abort();
        assert_eq!(queue.bulk_out.state().cancel_calls, 1);
        assert_eq!(queue.pending(), 0);
    }

    #[test]
    fn completed_buffers_are_reused_instead_of_reallocated() {
        let mut queue = queue();
        queue.write(&[1, 2], None).unwrap();
        queue.drain().unwrap();
        assert!(!queue.spare.is_empty(), "buffers must come back for reuse");
        let before = queue.spare.len();
        queue.write(&[3, 4], None).unwrap();
        assert_eq!(queue.spare.len(), before - 1);
    }
}
