//! What a *burst* means to HackRF firmware, on top of the shared bulk-OUT transport.
//!
//! The queue itself — transfers in flight, buffer recycling, packet padding and the error policy
//! — is `sdrmm-usb-stream`'s [`TxQueue`], the mirror of the receive side and shared for the same
//! reason. What is left here is the one thing that is this radio's and not the transport's:
//! libhackrf ends a burst with a zero-filled transfer, and the firmware needs it to know the
//! samples stopped on purpose rather than because the host fell behind.
//!
//! A write that runs out of time mid-burst leaves that marker owed, and the next write — or the
//! drain — pays it before anything else goes on the wire.

use std::time::{Duration, Instant};

use sdrmm_usb_stream::{BulkOut, TxConfig, TxQueue, TxStats};

use super::error::Result;

/// Bytes per transfer, libhackrf's size.
pub(crate) const TX_TRANSFER_SIZE: usize = 262_144;

/// Whether the burst still owes the radio an end marker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BurstTail {
    /// Samples have gone out since the last marker, so one is due before the queue goes idle.
    flush_required: bool,
    /// A marker was due and could not be sent in time; it is owed before anything else.
    terminal_flush_pending: bool,
}

impl BurstTail {
    const fn submitted_samples(&mut self) {
        self.flush_required = true;
    }

    const fn submitted_marker(&mut self) {
        self.flush_required = false;
        self.terminal_flush_pending = false;
    }

    const fn require_marker(&mut self) {
        self.flush_required = true;
        self.terminal_flush_pending = true;
    }
}

/// A live transmit burst on the shared queue.
///
/// Generic over the endpoint for the same reason the transport is: the burst boundary is the one
/// piece of transmit logic this crate still owns, so it is exercised against a scripted endpoint
/// rather than a radio (PLAN §14).
#[derive(Debug)]
pub(crate) struct BurstQueue<B: BulkOut> {
    queue: TxQueue<B>,
    tail: BurstTail,
    /// Length of the end marker, as the firmware reported its own buffer size.
    marker_len: usize,
}

impl<B: BulkOut> BurstQueue<B> {
    /// Prepare a burst queue on `endpoint`. `marker_len` is what the firmware reported as its
    /// buffer size, or libhackrf's fallback.
    pub(crate) fn start(endpoint: B, marker_len: usize) -> Result<Self> {
        Ok(Self {
            queue: TxQueue::start(endpoint, TxConfig::new(TX_TRANSFER_SIZE))?,
            tail: BurstTail::default(),
            marker_len,
        })
    }

    pub(crate) const fn stats(&self) -> TxStats {
        self.queue.stats()
    }

    /// Queue `bytes` of interleaved cs8 IQ, returning how many of them were accepted.
    ///
    /// A short return means `timeout` expired with the queue full; the caller keeps the rest and
    /// tries again. `end_burst` asks for the end marker once the samples are queued.
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
            if !self.queue.make_room(deadline)? {
                return Ok(0);
            }
            self.submit_marker();
            if bytes.is_empty() {
                return Ok(0);
            }
        }

        let accepted = self.queue.write(bytes, deadline)?;
        if accepted != 0 {
            self.tail.submitted_samples();
        }
        // An empty write asking to end the burst is how a caller marks the end of samples it
        // already queued, so it still owes a marker even though nothing was accepted.
        if end_burst && (accepted != 0 || bytes.is_empty()) {
            self.tail.require_marker();
            if self.queue.make_room(deadline)? {
                self.submit_marker();
            }
        }
        Ok(accepted)
    }

    /// Mark the end of the burst if one is owed, then wait for every transfer to complete.
    pub(crate) fn flush_and_drain(&mut self) -> Result<()> {
        // The end marker is queued before draining even if that briefly exceeds the queue depth:
        // it belongs adjacent to the burst it terminates, not after a gap.
        if self.tail.flush_required {
            self.submit_marker();
        }
        self.queue.drain()?;
        Ok(())
    }

    /// Abandon whatever is in flight. Used when the burst has already failed, where waiting for
    /// a clean drain would only delay releasing the endpoint.
    pub(crate) fn abort(&mut self) {
        self.queue.abort();
    }

    fn submit_marker(&mut self) {
        self.queue.submit_zeros(self.marker_len);
        self.tail.submitted_marker();
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_usb_stream::testing::ScriptedBulkOut;

    use super::*;

    const MARKER: usize = 1024;
    /// One whole USB high-speed packet, which is what a short burst is padded up to.
    const PACKET: usize = 512;
    const QUEUE_DEPTH: usize = 16;

    fn queue() -> BurstQueue<ScriptedBulkOut> {
        BurstQueue::start(ScriptedBulkOut::default(), MARKER).expect("a fake endpoint cannot fail")
    }

    #[test]
    fn a_burst_is_terminated_by_the_marker_the_firmware_expects() {
        let mut queue = queue();
        assert_eq!(queue.write(&[1, 2, 3, 4], Duration::MAX, true).unwrap(), 4);
        assert_eq!(queue.queue.endpoint().submitted(), vec![PACKET, MARKER]);
        assert_eq!(queue.stats().zero_buffers, 1);
    }

    #[test]
    fn samples_without_an_end_go_out_unmarked() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        assert_eq!(queue.queue.endpoint().submitted(), vec![PACKET]);
        assert_eq!(queue.stats().zero_buffers, 0);
    }

    /// The burst boundary is what tells the firmware the samples stopped on purpose. A write
    /// whose samples got in but whose marker did not still owes one, and the next write must pay
    /// before anything else reaches the wire — otherwise the next burst runs into the last.
    #[test]
    fn a_timed_out_burst_end_is_paid_by_the_next_write() {
        let mut queue = queue();
        queue.queue.endpoint().state().starve = true;
        let bytes = vec![0u8; TX_TRANSFER_SIZE];
        // Fill the queue one short of the water mark with nothing completing, so the last write
        // gets its samples in and then runs out of room for the marker that ends them.
        for _ in 0..QUEUE_DEPTH - 1 {
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

        queue.queue.endpoint().state().starve = false;
        queue.queue.endpoint().state().submitted.clear();
        assert_eq!(queue.write(&[1, 2], Duration::MAX, false).unwrap(), 2);
        assert_eq!(
            queue.queue.endpoint().submitted(),
            vec![MARKER, PACKET],
            "the owed end marker must go first"
        );
        assert!(!queue.tail.terminal_flush_pending);
    }

    #[test]
    fn draining_marks_the_end_of_an_unterminated_burst() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        queue.flush_and_drain().unwrap();
        assert_eq!(queue.queue.endpoint().submitted(), vec![PACKET, MARKER]);
        assert_eq!(queue.stats().buffers_completed, 2);
    }

    /// A caller that queued samples earlier can close the burst with an empty write; the marker
    /// is still owed even though nothing new was accepted.
    #[test]
    fn an_empty_write_can_close_a_burst() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        assert_eq!(queue.write(&[], Duration::MAX, true).unwrap(), 0);
        assert_eq!(queue.queue.endpoint().submitted(), vec![PACKET, MARKER]);
        // …and the drain adds no second marker, because nothing is owed any more.
        queue.flush_and_drain().unwrap();
        assert_eq!(queue.stats().zero_buffers, 1);
    }
}
