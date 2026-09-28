use std::{sync::OnceLock, time::Instant};

static START: OnceLock<Instant> = OnceLock::new();

pub fn init_clock() {
    START.get_or_init(Instant::now);
}

#[must_use]
pub fn now_ns() -> u64 {
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ns_never_goes_backwards() {
        init_clock();
        let mut last = now_ns();
        for _ in 0..10_000 {
            let now = now_ns();
            assert!(now >= last, "{now} < {last}");
            last = now;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(now_ns() >= last + 1_000_000);
    }

    #[test]
    fn a_second_init_keeps_the_origin() {
        init_clock();
        let before = now_ns();
        init_clock();
        assert!(now_ns() >= before);
    }
}
