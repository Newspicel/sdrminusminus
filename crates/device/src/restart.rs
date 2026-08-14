use std::time::Duration;

/// How long a stream may deliver nothing at all before the capture loop treats it as failed.
///
/// A streaming radio free-runs and cannot go quiet while healthy, and an unplug fails its queued
/// transfers rather than going silent — so this fires only for a board that has wedged with no
/// error to report, which would otherwise park the capture thread forever and leave the device
/// set advertising Running behind a dead waterfall.
pub const SILENT_STREAM_TIMEOUT: Duration = Duration::from_secs(5);

/// What a capture loop should do about a stream that ended on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recovery {
    /// Restart the stream in place, after waiting out `delay`.
    RetryAfter {
        /// 1-based attempt number, for the log line.
        attempt: u32,
        /// How long to let the pipe settle first.
        delay: Duration,
    },
    /// Out of attempts. Report the failure and let the engine take the device down.
    GiveUp {
        /// Restarts tried since the stream was last healthy.
        attempts: u32,
    },
}

/// Attempt counting and backoff for one capture thread.
#[derive(Clone, Copy, Debug)]
pub struct RestartPolicy {
    pub(crate) max_attempts: u32,
    pub(crate) base_delay: Duration,
    pub(crate) max_delay: Duration,
    pub(crate) min_healthy: Duration,
    pub(crate) attempts: u32,
}

impl Default for RestartPolicy {
    /// Three attempts, 20 ms doubling to 80 ms, and five seconds of streaming to earn a fresh
    /// budget. The delays are chosen against a restart that costs ~3 ms: long enough for a
    /// faulted pipe to settle, short enough that the whole tier stays far under the ~9 s the
    /// fallback costs.
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(20),
            max_delay: Duration::from_millis(200),
            min_healthy: SILENT_STREAM_TIMEOUT,
            attempts: 0,
        }
    }
}

impl RestartPolicy {
    /// The stream ended without being asked to. `uptime` is how long it ran before that.
    ///
    /// A stream that stayed up for `min_healthy` earns a fresh budget, so genuine transient
    /// stalls minutes apart never accumulate — while a stream that keeps dying immediately burns
    /// through its attempts and faults, which is what stops a restart loop from spinning
    /// forever on a board that is actually broken.
    pub fn on_failure(&mut self, uptime: Duration) -> Recovery {
        if uptime >= self.min_healthy {
            self.attempts = 0;
        }
        self.attempts += 1;
        if self.attempts > self.max_attempts {
            return Recovery::GiveUp {
                attempts: self.max_attempts,
            };
        }
        Recovery::RetryAfter {
            attempt: self.attempts,
            delay: self.backoff(),
        }
    }

    /// Restarts tried since the stream was last healthy. Zero while it is behaving.
    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    fn backoff(&self) -> Duration {
        let doublings = self.attempts.saturating_sub(1).min(u32::BITS - 1);
        self.base_delay
            .saturating_mul(1u32 << doublings)
            .min(self.max_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEALTHY: Duration = Duration::from_secs(30);
    const IMMEDIATE: Duration = Duration::from_millis(10);

    #[test]
    fn attempts_back_off_and_then_give_up() {
        let mut policy = RestartPolicy::default();
        assert_eq!(
            policy.on_failure(IMMEDIATE),
            Recovery::RetryAfter {
                attempt: 1,
                delay: Duration::from_millis(20)
            }
        );
        assert_eq!(
            policy.on_failure(IMMEDIATE),
            Recovery::RetryAfter {
                attempt: 2,
                delay: Duration::from_millis(40)
            }
        );
        assert_eq!(
            policy.on_failure(IMMEDIATE),
            Recovery::RetryAfter {
                attempt: 3,
                delay: Duration::from_millis(80)
            }
        );
        assert_eq!(
            policy.on_failure(IMMEDIATE),
            Recovery::GiveUp { attempts: 3 }
        );
        assert_eq!(policy.attempts(), 4);
    }

    /// The real stall this whole change exists for arrived 40 s into a healthy session. Spaced
    /// stalls must each get a full budget, or a long run would eventually fault on a stall it
    /// had already recovered from an hour earlier.
    #[test]
    fn a_stream_that_stayed_healthy_earns_a_fresh_budget() {
        let mut policy = RestartPolicy::default();
        for _ in 0..10 {
            assert_eq!(
                policy.on_failure(HEALTHY),
                Recovery::RetryAfter {
                    attempt: 1,
                    delay: Duration::from_millis(20)
                }
            );
        }
    }

    /// The termination argument: a restart that immediately dies again does not reset, so the
    /// loop cannot spin. Two attempts' worth of uptime is not enough to earn the reset either.
    #[test]
    fn a_stream_that_keeps_dying_immediately_still_gives_up() {
        let mut policy = RestartPolicy::default();
        assert!(matches!(
            policy.on_failure(HEALTHY),
            Recovery::RetryAfter { .. }
        ));
        for _ in 0..2 {
            assert!(matches!(
                policy.on_failure(Duration::from_secs(4)),
                Recovery::RetryAfter { .. }
            ));
        }
        assert_eq!(
            policy.on_failure(Duration::from_secs(4)),
            Recovery::GiveUp { attempts: 3 }
        );
    }

    #[test]
    fn backoff_is_capped() {
        let mut policy = RestartPolicy {
            max_attempts: 20,
            ..RestartPolicy::default()
        };
        let mut last = Duration::ZERO;
        for _ in 0..20 {
            let Recovery::RetryAfter { delay, .. } = policy.on_failure(IMMEDIATE) else {
                panic!("should still be retrying");
            };
            assert!(delay <= Duration::from_millis(200));
            assert!(delay >= last);
            last = delay;
        }
        assert_eq!(last, Duration::from_millis(200));
    }
}
