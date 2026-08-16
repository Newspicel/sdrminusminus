use std::time::{Duration, Instant};

use nusb::{
    Endpoint, MaybeFuture,
    transfer::{Buffer, Bulk, Out, TransferError},
};

use crate::{
    error::{Result, StreamError},
    policy::{Action, TransferPolicy},
};

#[derive(Debug)]
pub struct OutCompletion {
    pub bytes: Vec<u8>,
    pub status: std::result::Result<(), TransferError>,
}

pub trait BulkOut: Send + 'static {
    fn clear_halt(&mut self) -> Result<()>;
    fn submit(&mut self, bytes: Vec<u8>);
    fn pending(&self) -> usize;
    fn wait_next_complete(&mut self, timeout: Duration) -> Option<OutCompletion>;
    fn cancel_all(&mut self);
}

#[derive(Debug)]
pub struct NusbBulkOut {
    endpoint: Endpoint<Bulk, Out>,
}

impl NusbBulkOut {
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

#[derive(Clone, Copy, Debug)]
pub struct TxConfig {
    pub transfer_size: usize,
    pub queue_depth: usize,
    pub max_packet: usize,
}

impl TxConfig {
    #[must_use]
    pub const fn new(transfer_size: usize) -> Self {
        Self {
            transfer_size,
            queue_depth: 16,
            max_packet: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxStats {
    pub bytes_accepted: u64,
    pub buffers_submitted: u64,
    pub buffers_completed: u64,
    pub buffers_failed: u64,
    pub zero_buffers: u64,
}

#[derive(Debug)]
pub struct TxQueue<B: BulkOut> {
    bulk_out: B,
    policy: TransferPolicy,
    stats: TxStats,
    config: TxConfig,
    spare: Vec<Vec<u8>>,
    stopping: bool,
}

impl<B: BulkOut> TxQueue<B> {
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

    #[must_use]
    pub const fn stats(&self) -> TxStats {
        self.stats
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.bulk_out.pending()
    }

    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub const fn endpoint(&self) -> &B {
        &self.bulk_out
    }

    pub fn write(&mut self, bytes: &[u8], deadline: Option<Instant>) -> Result<usize> {
        let mut accepted = 0;
        for chunk in bytes.chunks(self.config.transfer_size) {
            if !self.make_room(deadline)? {
                return Ok(accepted);
            }
            let mut buffer = self.take_buffer();
            buffer.extend_from_slice(chunk);
            buffer.resize(chunk.len().next_multiple_of(self.config.max_packet), 0);
            self.bulk_out.submit(buffer);
            self.stats.bytes_accepted += chunk.len() as u64;
            self.stats.buffers_submitted += 1;
            accepted += chunk.len();
        }
        Ok(accepted)
    }

    pub fn submit_zeros(&mut self, len: usize) {
        let mut buffer = self.take_buffer();
        buffer.resize(len, 0);
        self.bulk_out.submit(buffer);
        self.stats.buffers_submitted += 1;
        self.stats.zero_buffers += 1;
    }

    pub fn make_room(&mut self, deadline: Option<Instant>) -> Result<bool> {
        while self.bulk_out.pending() >= self.config.queue_depth {
            if !self.complete_one(deadline)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn drain(&mut self) -> Result<()> {
        // Bounded, because a timeout out of `complete_one` leaves the transfer pending: an
        // endpoint that stalls or leaves the bus mid-drain would otherwise spin here forever,
        // and this runs on the caller's thread inside `TxStream::stop`.
        for _ in 0..self.config.queue_depth {
            if self.bulk_out.pending() == 0 {
                return Ok(());
            }
            if !self.complete_one(None)? {
                break;
            }
        }
        if self.bulk_out.pending() != 0 {
            tracing::warn!("usb tx endpoint stopped completing transfers; cancelling the queue");
            self.abort();
        }
        Ok(())
    }

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

    #[test]
    fn draining_a_stalled_endpoint_gives_up_and_cancels_instead_of_hanging() {
        let mut queue = queue();
        queue.write(&[1, 2], None).unwrap();
        queue.bulk_out.state().starve = true;
        queue.drain().expect("drain returns on a stalled endpoint");
        assert_eq!(queue.bulk_out.state().cancel_calls, 1);
    }

    #[test]
    fn a_failed_transfer_is_dropped_rather_than_re_sent() {
        let mut queue = queue();
        queue.bulk_out.fail_next(1, TransferError::Fault);
        let bytes = vec![0u8; TRANSFER_SIZE];
        for _ in 0..=DEPTH {
            queue.write(&bytes, None).unwrap();
        }
        assert_eq!(queue.stats().buffers_failed, 1);
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
