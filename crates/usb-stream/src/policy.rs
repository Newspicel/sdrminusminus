use nusb::transfer::TransferError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Deliver,
    Resubmit,
    Exit,
    GiveUp { attempts: u32, error: TransferError },
}

#[derive(Clone, Copy, Debug)]
pub struct TransferPolicy {
    threshold: u32,
    consecutive_errors: u32,
}

impl TransferPolicy {
    #[must_use]
    pub const fn new(threshold: u32) -> Self {
        Self {
            threshold,
            consecutive_errors: 0,
        }
    }

    #[must_use]
    pub const fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors
    }

    pub fn on_completion(
        &mut self,
        status: std::result::Result<(), TransferError>,
        stopping: bool,
    ) -> Action {
        let Err(error) = status else {
            self.consecutive_errors = 0;
            return Action::Deliver;
        };
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
