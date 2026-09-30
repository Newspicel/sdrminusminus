const WINDOW: usize = 64;

#[derive(Clone, Debug)]
pub struct PoseClock {
    offsets: [i64; WINDOW],
    next: usize,
    filled: usize,
}

impl Default for PoseClock {
    fn default() -> Self {
        Self::new()
    }
}

impl PoseClock {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            offsets: [0; WINDOW],
            next: 0,
            filled: 0,
        }
    }

    pub fn map(&mut self, fix_time_ns: i64, received_ns: i64) -> i64 {
        let offset = received_ns.saturating_sub(fix_time_ns);
        self.offsets[self.next] = offset;
        self.next = (self.next + 1) % WINDOW;
        self.filled = (self.filled + 1).min(WINDOW);
        let least = self.offsets[..self.filled]
            .iter()
            .copied()
            .fold(offset, i64::min);
        fix_time_ns.saturating_add(least)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    #[test]
    fn pose_clock_maps_fix_time_through_the_minimum_offset() {
        let mut clock = PoseClock::new();
        assert_eq!(clock.map(1_000 * MS, 1_300 * MS), 1_300 * MS);
        assert_eq!(clock.map(1_050 * MS, 1_170 * MS), 1_170 * MS);
        assert_eq!(
            clock.map(1_100 * MS, 1_400 * MS),
            1_220 * MS,
            "a late delivery keeps the smallest offset seen"
        );
        for step in 0..62 {
            let fix = 1_150 * MS + step * 50 * MS;
            assert_eq!(clock.map(fix, fix + 200 * MS), fix + 120 * MS);
        }
        let fix = 5_000 * MS;
        assert_eq!(
            clock.map(fix, fix + 200 * MS),
            fix + 200 * MS,
            "an offset older than 64 poses is forgotten"
        );
    }

    #[test]
    fn a_phone_clock_ahead_of_the_host_maps_back() {
        let mut clock = PoseClock::default();
        assert_eq!(clock.map(10_000 * MS, 8_100 * MS), 8_100 * MS);
        assert_eq!(clock.map(10_050 * MS, 8_400 * MS), 8_150 * MS);
    }

    #[test]
    fn jittered_delivery_maps_to_an_even_track() {
        let mut clock = PoseClock::new();
        let jitter = [50, 300, 120, 55, 260, 180, 51, 90, 240, 70];
        let mapped: Vec<i64> = (0..200)
            .map(|step| {
                let fix = step * 50 * MS;
                clock.map(fix, fix + jitter[step as usize % jitter.len()] * MS) - fix
            })
            .collect();
        assert!(mapped[10..].iter().all(|&offset| offset == 50 * MS));
    }
}
