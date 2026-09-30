use std::time::Duration;

const BASE: Duration = Duration::from_millis(500);
const CAP: Duration = Duration::from_secs(30);
const FLOOR: Duration = Duration::from_millis(100);
const MAX_ATTEMPT: u32 = 16;

#[derive(Clone, Copy, Debug)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Backoff {
    attempt: u32,
    rng: XorShift64,
}

impl Backoff {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            attempt: 0,
            rng: XorShift64::new(seed),
        }
    }

    pub(crate) fn next_delay(&mut self) -> Duration {
        let ceiling = BASE
            .checked_mul(1u32 << self.attempt.min(MAX_ATTEMPT))
            .map_or(CAP, |delay| delay.min(CAP));
        self.attempt = self.attempt.saturating_add(1).min(MAX_ATTEMPT);
        ceiling.mul_f64(self.rng.unit()).max(FLOOR)
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }

    pub(crate) fn attempt(&self) -> u32 {
        self.attempt
    }
}

pub(crate) fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    (nanos as u64) ^ ((nanos >> 64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_with_full_jitter_and_caps() {
        let mut backoff = Backoff::new(42);
        for attempt in 0..12u32 {
            let ceiling = (BASE * 2u32.pow(attempt)).min(CAP);
            let delay = backoff.next_delay();
            assert!(delay >= FLOOR, "attempt {attempt}: {delay:?}");
            assert!(delay <= ceiling, "attempt {attempt}: {delay:?}");
        }
        assert_eq!(BASE * 2u32.pow(6), Duration::from_secs(32));
        let late: Vec<Duration> = (0..200).map(|_| backoff.next_delay()).collect();
        assert!(late.iter().all(|delay| *delay <= CAP));
        assert!(late.iter().any(|delay| *delay > CAP / 2));
        assert_eq!(backoff.attempt(), MAX_ATTEMPT);
    }

    #[test]
    fn backoff_resets_after_a_minute_live() {
        let mut backoff = Backoff::new(7);
        for _ in 0..5 {
            backoff.next_delay();
        }
        assert_eq!(backoff.attempt(), 5);
        backoff.reset();
        assert_eq!(backoff.attempt(), 0);
        assert!(backoff.next_delay() <= BASE);
    }

    #[test]
    fn the_same_seed_gives_the_same_delays() {
        let mut first = Backoff::new(99);
        let mut second = Backoff::new(99);
        for _ in 0..10 {
            assert_eq!(first.next_delay(), second.next_delay());
        }
        assert_ne!(seed(), 0);
    }
}
