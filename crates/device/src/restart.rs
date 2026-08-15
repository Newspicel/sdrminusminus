use std::time::Duration;

pub const SILENT_STREAM_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recovery {
    RetryAfter { attempt: u32, delay: Duration },
    GiveUp { attempts: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct RestartPolicy {
    pub(crate) max_attempts: u32,
    pub(crate) base_delay: Duration,
    pub(crate) max_delay: Duration,
    pub(crate) min_healthy: Duration,
    pub(crate) attempts: u32,
}

impl Default for RestartPolicy {
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
