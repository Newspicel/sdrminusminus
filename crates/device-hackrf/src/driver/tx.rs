use std::time::{Duration, Instant};

use sdrmm_usb_stream::{BulkOut, TxConfig, TxQueue, TxStats};

use super::error::Result;

pub(crate) const TX_TRANSFER_SIZE: usize = 262_144;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BurstTail {
    flush_required: bool,
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

#[derive(Debug)]
pub(crate) struct BurstQueue<B: BulkOut> {
    queue: TxQueue<B>,
    tail: BurstTail,
    marker_len: usize,
}

impl<B: BulkOut> BurstQueue<B> {
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

    pub(crate) fn write(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize> {
        let deadline = (timeout != Duration::MAX).then(|| Instant::now() + timeout);

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
        if end_burst && (accepted != 0 || bytes.is_empty()) {
            self.tail.require_marker();
            if self.queue.make_room(deadline)? {
                self.submit_marker();
            }
        }
        Ok(accepted)
    }

    pub(crate) fn flush_and_drain(&mut self) -> Result<()> {
        if self.tail.flush_required {
            self.submit_marker();
        }
        self.queue.drain()?;
        Ok(())
    }

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

    #[test]
    fn a_timed_out_burst_end_is_paid_by_the_next_write() {
        let mut queue = queue();
        queue.queue.endpoint().state().starve = true;
        let bytes = vec![0u8; TX_TRANSFER_SIZE];
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

    #[test]
    fn an_empty_write_can_close_a_burst() {
        let mut queue = queue();
        queue.write(&[1, 2], Duration::MAX, false).unwrap();
        assert_eq!(queue.write(&[], Duration::MAX, true).unwrap(), 0);
        assert_eq!(queue.queue.endpoint().submitted(), vec![PACKET, MARKER]);
        queue.flush_and_drain().unwrap();
        assert_eq!(queue.stats().zero_buffers, 1);
    }
}
