//! What to do about a transfer that did not succeed.
//!
//! This is the one piece both native drivers used to own privately and both got wrong (PLAN
//! §18). It is librtlsdr's policy (`src/librtlsdr.c:3814`): a cancellation is never an error,
//! only genuine failures count, the threshold is the queue depth, and any success clears the
//! count. Where it differs — deliberately — is that an errored
//! transfer is resubmitted and the stream continues instead of being retired, which is what
//! makes a single fault survivable.
//!
//! Pure and stateless apart from the counter, so every rule below is a unit test.

use nusb::transfer::TransferError;

/// What the pump does with a completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Hand the bytes to the consumer, then resubmit.
    Deliver,
    /// Discard the bytes and resubmit; the stream is still healthy.
    Resubmit,
    /// A cancellation we asked for. Stop, with no error.
    Exit,
    /// Too many genuine errors in a row. Stop and report.
    GiveUp {
        /// Consecutive errored completions, including this one.
        attempts: u32,
        /// The status that reached the threshold.
        error: TransferError,
    },
}

/// Consecutive-error accounting for one streaming session.
#[derive(Clone, Copy, Debug)]
pub struct TransferPolicy {
    threshold: u32,
    consecutive_errors: u32,
}

impl TransferPolicy {
    /// `threshold` is the queue depth: one fault aborts everything behind it, so anything
    /// smaller declares a stall a disconnect. That off-by-a-queue is exactly the bug this
    /// replaces — `rs-rtl` tripped at 5 with 15 transfers in flight.
    #[must_use]
    pub const fn new(threshold: u32) -> Self {
        Self {
            threshold,
            consecutive_errors: 0,
        }
    }

    /// Consecutive genuine errors since the last successful completion.
    #[must_use]
    pub const fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors
    }

    /// Classify one completion. `stopping` is the caller's own stop flag, and is the only thing
    /// that separates "we cancelled these" from "the pipe faulted and took the queue with it" —
    /// the two are indistinguishable in the completion itself.
    pub fn on_completion(
        &mut self,
        status: std::result::Result<(), TransferError>,
        stopping: bool,
    ) -> Action {
        let Err(error) = status else {
            self.consecutive_errors = 0;
            return Action::Deliver;
        };
        // `nusb` reports a timeout as `Cancelled` too, but the pump never puts a timeout on a
        // transfer (`wait_next_complete` leaves it pending), so every cancellation here is
        // either ours or the fallout of a fault on another transfer in the queue.
        if error == TransferError::Cancelled {
            return if stopping {
                Action::Exit
            } else {
                Action::Resubmit
            };
        }
        self.consecutive_errors += 1;
        if self.consecutive_errors >= self.threshold {
            Action::GiveUp {
                attempts: self.consecutive_errors,
                error,
            }
        } else {
            Action::Resubmit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPTH: u32 = 16;

    fn policy() -> TransferPolicy {
        TransferPolicy::new(DEPTH)
    }

    #[test]
    fn success_delivers_and_clears_the_count() {
        let mut policy = policy();
        assert_eq!(
            policy.on_completion(Err(TransferError::Fault), false),
            Action::Resubmit
        );
        assert_eq!(policy.consecutive_errors(), 1);
        assert_eq!(policy.on_completion(Ok(()), false), Action::Deliver);
        assert_eq!(policy.consecutive_errors(), 0);
    }

    /// The regression that motivates the whole change: one stalled pipe delivers one real error
    /// plus a cancellation for every transfer queued behind it. Under `rs-rtl`'s policy that was
    /// 5 in a row and cost a full re-open; here it costs nothing.
    #[test]
    fn a_burst_of_cancellations_while_running_never_trips_the_threshold() {
        let mut policy = policy();
        assert_eq!(
            policy.on_completion(Err(TransferError::Fault), false),
            Action::Resubmit
        );
        for _ in 0..DEPTH * 4 {
            assert_eq!(
                policy.on_completion(Err(TransferError::Cancelled), false),
                Action::Resubmit
            );
        }
        assert_eq!(policy.consecutive_errors(), 1);
    }

    #[test]
    fn cancellations_while_stopping_are_a_clean_exit() {
        let mut policy = policy();
        assert_eq!(
            policy.on_completion(Err(TransferError::Cancelled), true),
            Action::Exit
        );
        assert_eq!(policy.consecutive_errors(), 0);
    }

    #[test]
    fn genuine_errors_give_up_at_the_queue_depth() {
        let mut policy = policy();
        for attempt in 1..DEPTH {
            assert_eq!(
                policy.on_completion(Err(TransferError::Stall), false),
                Action::Resubmit,
                "attempt {attempt} should still retry"
            );
        }
        assert_eq!(
            policy.on_completion(Err(TransferError::Stall), false),
            Action::GiveUp {
                attempts: DEPTH,
                error: TransferError::Stall,
            }
        );
    }

    #[test]
    fn one_success_inside_a_run_of_errors_resets_the_countdown() {
        let mut policy = policy();
        for _ in 0..DEPTH - 1 {
            assert_eq!(
                policy.on_completion(Err(TransferError::Fault), false),
                Action::Resubmit
            );
        }
        assert_eq!(policy.on_completion(Ok(()), false), Action::Deliver);
        for _ in 0..DEPTH - 1 {
            assert_eq!(
                policy.on_completion(Err(TransferError::Fault), false),
                Action::Resubmit
            );
        }
    }

    /// An unplug fails every queued transfer with the same status, so the threshold is also what
    /// bounds how long a gone device is retried before the stream reports it.
    #[test]
    fn a_disconnect_gives_up_within_one_queue() {
        let mut policy = policy();
        let mut completions = 0;
        loop {
            completions += 1;
            if let Action::GiveUp { error, .. } =
                policy.on_completion(Err(TransferError::Disconnected), false)
            {
                assert_eq!(error, TransferError::Disconnected);
                break;
            }
            assert!(completions <= DEPTH, "gave up after more than one queue");
        }
        assert_eq!(completions, DEPTH);
    }
}
