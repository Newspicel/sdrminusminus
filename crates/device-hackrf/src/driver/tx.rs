//! The bulk-OUT half: queueing transmit buffers to the radio.
//!
//! Deliberately *not* in `sdrmm-usb-stream`. The RTL2832U cannot transmit, so a shared bulk-OUT
//! transport would have exactly one caller; what the two directions genuinely share is the
//! transfer-*error* policy, and that is what is imported from there rather than written a second
//! time. The seam below exists for the same reason the receive one does: so the queue discipline
//! and the burst boundary are exercised against a mock, with no radio (PLAN §14).
//!
//! Transmit has one rule receive does not: **a failed transfer is never re-sent.** On the
//! receive side an errored transfer carried nothing, so resubmitting it costs nothing and keeps
//! the queue full. Here it carried samples that were meant for a specific moment, and putting
//! them back on the wire behind everything queued after them would corrupt the burst more
//! thoroughly than the gap it is trying to repair. So [`TransferPolicy`]'s counting is used —
//! cancellations never count, genuine errors do, the threshold is the queue depth, any success
//! clears it — while its `Resubmit` verdict is read here as "tolerate and carry on".

use std::time::{Duration, Instant};

use nusb::{
    Endpoint, MaybeFuture,
    transfer::{Buffer, Bulk, Out, TransferError},
};
use sdrmm_usb_stream::{Action, TransferPolicy};

use super::error::{Error, Result};

/// Transfers kept in flight, matching the receive queue.
pub(crate) const TX_QUEUE_DEPTH: usize = 16;
/// Bytes per transfer, libhackrf's size.
pub(crate) const TX_TRANSFER_SIZE: usize = 262_144;
/// USB high-speed maximum packet size: every submitted buffer is a whole number of these, or the
/// radio's own framing slips.
const MAX_PACKET: usize = 512;

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
    /// Zero-filled terminal flush transfers, which mark the end of a burst.
    pub flush_buffers: u64,
}

/// One finished transmit transfer, with its buffer back for reuse.
#[derive(Debug)]
pub(crate) struct OutCompletion {
    pub(crate) bytes: Vec<u8>,
    pub(crate) status: std::result::Result<(), TransferError>,
}

/// A queue of bulk-OUT transfers.
pub(crate) trait BulkOut: Send + 'static {
    /// Clear a stall and reset the data toggle, with nothing in flight.
    fn clear_halt(&mut self) -> Result<()>;
    /// Queue `bytes` for transmission.
    fn submit(&mut self, bytes: Vec<u8>);
    /// Transfers currently in flight.
    fn pending(&self) -> usize;
    /// Block until the oldest transfer finishes, or `timeout` expires. Only called with at
    /// least one transfer pending.
    fn wait_next_complete(&mut self, timeout: Duration) -> Option<OutCompletion>;
    /// Cancel every transfer in flight. Each still completes, with `Cancelled`.
    fn cancel_all(&mut self);
}

/// [`BulkOut`] over a real `nusb` endpoint.
#[derive(Debug)]
pub(crate) struct NusbBulkOut {
    endpoint: Endpoint<Bulk, Out>,
}

impl NusbBulkOut {
    pub(crate) fn open(interface: &nusb::Interface, address: u8) -> Result<Self> {
        Ok(Self {
            endpoint: interface
                .endpoint::<Bulk, Out>(address)
                .map_err(|e| Error::usb("claiming the HackRF TX endpoint", e))?,
        })
    }
}

impl BulkOut for NusbBulkOut {
    fn clear_halt(&mut self) -> Result<()> {
        self.endpoint
            .clear_halt()
            .wait()
            .map_err(|e| Error::usb("clearing the HackRF TX endpoint stall", e))
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

/// Whether the burst still owes the radio an end marker.
///
/// libhackrf ends a burst with a zero-filled transfer, and the firmware needs it to know the
/// samples stopped on purpose rather than because the host fell behind. A write that runs out of
/// time mid-burst leaves the marker owed, and the next write — or `stop` — pays it before
/// anything else goes on the wire.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BurstTail {
    flush_required: bool,
    terminal_flush_pending: bool,
}

impl BurstTail {
    fn submitted_samples(&mut self) {
        self.flush_required = true;
    }

    fn submitted_flush(&mut self) {
        self.flush_required = false;
        self.terminal_flush_pending = false;
    }

    fn require_terminal_flush(&mut self) {
        self.flush_required = true;
        self.terminal_flush_pending = true;
    }
}

/// A live transmit queue. Bytes in, nothing out — sample conversion is the backend's, beside the
/// receive direction's, because that is where the two radios differ.
#[derive(Debug)]
pub(crate) struct TxQueue<B: BulkOut> {
    bulk_out: B,
    policy: TransferPolicy,
    stats: TxStats,
    tail: BurstTail,
    flush_size: usize,
    /// Completed buffers, kept so a steady burst allocates nothing.
    spare: Vec<Vec<u8>>,
    stopping: bool,
}

impl<B: BulkOut> TxQueue<B> {
    /// Prepare a queue on an un-stalled endpoint. `flush_size` is what the firmware reported as
    /// its buffer size, or libhackrf's fallback.
    pub(crate) fn start(mut bulk_out: B, flush_size: usize) -> Result<Self> {
        bulk_out.clear_halt()?;
        Ok(Self {
            bulk_out,
            policy: TransferPolicy::new(u32::try_from(TX_QUEUE_DEPTH).unwrap_or(u32::MAX)),
            stats: TxStats::default(),
            tail: BurstTail::default(),
            flush_size,
            spare: Vec::new(),
            stopping: false,
        })
    }

    pub(crate) const fn stats(&self) -> TxStats {
        self.stats
    }

    /// Queue `bytes` of interleaved cs8 IQ, returning how many of them were accepted.
    ///
    /// A short return means `timeout` expired with the queue full; the caller keeps the rest and
    /// tries again. `end_burst` asks for the terminal flush once the samples are queued.
    pub(crate) fn write(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize> {
        let deadline = (timeout != Duration::MAX).then(|| Instant::now() + timeout);

        // An earlier write ran out of time before it could mark its burst end. Pay that first,
        // so the radio never sees a later burst run into the previous one.
        if self.tail.terminal_flush_pending {
            if !self.make_room(deadline)? {
                return Ok(0);
            }
            self.submit_flush();
            if bytes.is_empty() {
                return Ok(0);
            }
        }

        let mut accepted = 0;
        for chunk in bytes.chunks(TX_TRANSFER_SIZE) {
            if !self.make_room(deadline)? {
                if end_burst && accepted != 0 {
                    self.tail.require_terminal_flush();
                }
                return Ok(accepted);
            }
            self.submit_samples(chunk);
            accepted += chunk.len();
        }

        if end_burst && (accepted != 0 || bytes.is_empty()) {
            self.tail.require_terminal_flush();
            if !self.make_room(deadline)? {
                return Ok(accepted);
            }
            self.submit_flush();
        }
        Ok(accepted)
    }

    /// Mark the end of the burst if one is owed, then wait for every transfer to complete.
    pub(crate) fn flush_and_drain(&mut self) -> Result<()> {
        // The end marker is queued before draining even if that briefly exceeds the queue
        // depth: it belongs adjacent to the burst it terminates, not after a gap.
        if self.tail.flush_required {
            self.submit_flush();
        }
        while self.bulk_out.pending() != 0 {
            self.complete_one(None)?;
        }
        Ok(())
    }

    /// Abandon whatever is in flight. Used when the burst has already failed, where waiting for
    /// a clean drain would only delay releasing the endpoint.
    pub(crate) fn abort(&mut self) {
        self.stopping = true;
        self.bulk_out.cancel_all();
        for _ in 0..TX_QUEUE_DEPTH {
            if self.bulk_out.pending() == 0 {
                break;
            }
            if self.bulk_out.wait_next_complete(FLUSH_POLL).is_none() {
                tracing::debug!("hackrf tx transfers still pending after cancel");
                break;
            }
        }
    }

    /// Wait until the queue has room for one more transfer. `false` means the deadline expired.
    fn make_room(&mut self, deadline: Option<Instant>) -> Result<bool> {
        while self.bulk_out.pending() >= TX_QUEUE_DEPTH {
            if !self.complete_one(deadline)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Consume one completion. `Ok(false)` means the deadline expired with none available.
    fn complete_one(&mut self, deadline: Option<Instant>) -> Result<bool> {
        let wait = deadline.map_or(FLUSH_POLL, |at| {
            at.saturating_duration_since(Instant::now())
        });
        let Some(completion) = self.bulk_out.wait_next_complete(wait) else {
            return Ok(false);
        };
        let mut bytes = completion.bytes;
        bytes.clear();
        if self.spare.len() < TX_QUEUE_DEPTH {
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
                    "hackrf tx transfer failed; samples dropped"
                );
                Ok(true)
            }
            Action::GiveUp { attempts, error } => {
                self.stats.buffers_failed += 1;
                Err(Error::TxFailed { attempts, error })
            }
        }
    }

    fn submit_samples(&mut self, chunk: &[u8]) {
        let mut bytes = self.spare.pop().unwrap_or_default();
        bytes.clear();
        bytes.extend_from_slice(chunk);
        // The endpoint takes whole packets only; the tail of a burst is padded with silence.
        bytes.resize(chunk.len().next_multiple_of(MAX_PACKET), 0);
        self.bulk_out.submit(bytes);
        self.tail.submitted_samples();
        self.stats.bytes_accepted += chunk.len() as u64;
        self.stats.buffers_submitted += 1;
    }

    fn submit_flush(&mut self) {
        let mut bytes = self.spare.pop().unwrap_or_default();
        bytes.clear();
        bytes.resize(self.flush_size, 0);
        self.bulk_out.submit(bytes);
        self.tail.submitted_flush();
        self.stats.buffers_submitted += 1;
        self.stats.flush_buffers += 1;
    }
}

/// How long a drain waits on one completion before checking whether it is making progress.
const FLUSH_POLL: Duration = Duration::from_millis(500);

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use super::*;

    const FLUSH_SIZE: usize = 1024;

    #[derive(Debug, Default)]
    struct FakeState {
        queued: VecDeque<Vec<u8>>,
        submitted: Vec<usize>,
        statuses: VecDeque<std::result::Result<(), TransferError>>,
        cancel_calls: usize,
        clear_halt_calls: usize,
        starve: bool,
    }

    /// Scripted [`BulkOut`]: completions succeed unless a status was queued, and `starve` makes
    /// the endpoint stop completing so a caller's timeout can be exercised.
    #[derive(Debug, Default)]
    struct FakeBulkOut {
        state: Mutex<FakeState>,
    }

    impl FakeBulkOut {
        fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl BulkOut for FakeBulkOut {
        fn clear_halt(&mut self) -> Result<()> {
            self.state().clear_halt_calls += 1;
            Ok(())
        }

        fn submit(&mut self, bytes: Vec<u8>) {
            let mut state = self.state();
            state.submitted.push(bytes.len());
            state.queued.push_back(bytes);
        }

        fn pending(&self) -> usize {
            self.state().queued.len()
        }

        fn wait_next_complete(&mut self, _timeout: Duration) -> Option<OutCompletion> {
            let mut state = self.state();
            if state.starve {
                return None;
            }
            let bytes = state.queued.pop_front()?;
            let status = state.statuses.pop_front().unwrap_or(Ok(()));
            Some(OutCompletion { bytes, status })
        }

        fn cancel_all(&mut self) {
            let mut state = self.state();
            state.cancel_calls += 1;
            let cancelled = state.queued.len();
            state.statuses.clear();
            for _ in 0..cancelled {
                state.statuses.push_back(Err(TransferError::Cancelled));
            }
        }
    }

    fn queue() -> TxQueue<FakeBulkOut> {
        TxQueue::start(FakeBulkOut::default(), FLUSH_SIZE).expect("fake endpoint cannot fail")
    }

    #[test]
    fn starting_clears_the_endpoint_stall() {
        let queue = queue();
        assert_eq!(queue.bulk_out.state().clear_halt_calls, 1);
    }

    #[test]
    fn a_burst_is_padded_to_whole_packets_and_terminated() {
        let mut queue = queue();
        assert_eq!(queue.write(&[1, 2, 3, 4], Duration::MAX, true).unwrap(), 4);
        let submitted = queue.bulk_out.state().submitted.clone();
        // Four bytes padded up to one 512-byte packet, then the terminal flush.
        assert_eq!(submitted, vec![MAX_PACKET, FLUSH_SIZE]);
        assert_eq!(queue.stats().bytes_accepted, 4);
        assert_eq!(queue.stats().flush_buffers, 1);
    }

    #[test]
    fn a_burst_longer_than_one_transfer_is_split() {
        let mut queue = queue();
        let bytes = vec![7u8; TX_TRANSFER_SIZE + MAX_PACKET];
        assert_eq!(
            queue.write(&bytes, Duration::MAX, false).unwrap(),
            bytes.len()
        );
        assert_eq!(
            queue.bulk_out.state().submitted,
            vec![TX_TRANSFER_SIZE, MAX_PACKET]
        );
        // No end marker was asked for, so none was sent.
        assert_eq!(queue.stats().flush_buffers, 0);
    }

    /// The burst boundary is what tells the firmware the samples stopped on purpose. A write
    /// whose samples got in but whose marker did not still owes it, and the next write must pay
    /// before anything else goes on the wire — otherwise the next burst runs into the last.
    #[test]
    fn a_timed_out_burst_end_is_paid_by_the_next_write() {
        let mut queue = queue();
        queue.bulk_out.state().starve = true;
        let bytes = vec![0u8; TX_TRANSFER_SIZE];
        // Fill the queue one short of the water mark with nothing completing, so the last write
        // gets its samples in and then runs out of room for the marker that ends them.
        for _ in 0..TX_QUEUE_DEPTH - 1 {
            assert_eq!(
                queue.write(&bytes, Duration::ZERO, false).unwrap(),
                bytes.len()
            );
        }
        assert_eq!(
            queue.write(&bytes, Duration::ZERO, true).unwrap(),
            bytes.len()
        );
        assert!(queue.tail.terminal_flush_pending);

        queue.bulk_out.state().starve = false;
        queue.bulk_out.state().submitted.clear();
        assert_eq!(queue.write(&[1, 2], Duration::MAX, false).unwrap(), 2);
        assert_eq!(
            queue.bulk_out.state().submitted,
            vec![FLUSH_SIZE, MAX_PACKET],
            "the owed end marker must go first"
        );
        assert!(!queue.tail.terminal_flush_pending);
    }

    #[test]
    fn draining_marks_the_end_of_an_unterminated_burst() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        queue.flush_and_drain().unwrap();
        assert_eq!(
            queue.bulk_out.state().submitted,
            vec![MAX_PACKET, FLUSH_SIZE]
        );
        assert_eq!(queue.bulk_out.pending(), 0);
        assert_eq!(queue.stats().buffers_completed, 2);
    }

    /// The receive side resubmits an errored transfer; this side must not — the samples were
    /// meant for a moment that has passed, and re-sending them behind the queue would corrupt
    /// the burst worse than the gap.
    #[test]
    fn a_failed_transfer_is_dropped_rather_than_re_sent() {
        let mut queue = queue();
        queue
            .bulk_out
            .state()
            .statuses
            .push_back(Err(TransferError::Fault));
        let bytes = vec![0u8; TX_TRANSFER_SIZE];
        for _ in 0..=TX_QUEUE_DEPTH {
            queue.write(&bytes, Duration::MAX, false).unwrap();
        }
        assert_eq!(queue.stats().buffers_failed, 1);
        // One submission per write and nothing re-sent.
        assert_eq!(queue.bulk_out.state().submitted.len(), TX_QUEUE_DEPTH + 1);
    }

    #[test]
    fn a_full_queue_of_failures_ends_the_burst() {
        let mut queue = queue();
        for _ in 0..TX_QUEUE_DEPTH {
            queue
                .bulk_out
                .state()
                .statuses
                .push_back(Err(TransferError::Fault));
        }
        let bytes = vec![0u8; TX_TRANSFER_SIZE];
        let error = loop {
            match queue.write(&bytes, Duration::MAX, false) {
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(matches!(
            error,
            Error::TxFailed { attempts, error: TransferError::Fault }
                if attempts as usize == TX_QUEUE_DEPTH
        ));
    }

    #[test]
    fn aborting_cancels_and_collects_every_transfer() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        queue.abort();
        assert_eq!(queue.bulk_out.state().cancel_calls, 1);
        assert_eq!(queue.bulk_out.pending(), 0);
    }

    #[test]
    fn completed_buffers_are_reused_instead_of_reallocated() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        queue.flush_and_drain().unwrap();
        assert!(!queue.spare.is_empty(), "buffers must come back for reuse");
        let before = queue.spare.len();
        queue.write(&[3, 4], Duration::MAX, false).unwrap();
        assert_eq!(queue.spare.len(), before - 1);
    }
}
