#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossAction {
    Continuous,
    Gap(u64),
    Reset(u64),
}

pub struct LossTracker {
    max_gap: u64,
    last: Option<u64>,
    learned: Option<u64>,
}

impl LossTracker {
    #[must_use]
    pub const fn new(max_gap: u64) -> Self {
        Self {
            max_gap,
            last: None,
            learned: None,
        }
    }

    #[must_use]
    pub const fn packet_frames(&self) -> Option<u64> {
        self.learned
    }

    pub fn next(&mut self, timestamp: u64) -> LossAction {
        let Some(last) = self.last.replace(timestamp) else {
            return LossAction::Continuous;
        };
        if timestamp <= last {
            self.learned = None;
            return LossAction::Reset(0);
        }
        let delta = timestamp - last;
        let learned = match self.learned {
            Some(learned) if delta >= learned => learned,
            _ => {
                self.learned = Some(delta);
                return LossAction::Continuous;
            }
        };
        match delta - learned {
            0 => LossAction::Continuous,
            gap if gap > self.max_gap => LossAction::Reset(gap),
            gap => LossAction::Gap(gap),
        }
    }

    pub fn reset(&mut self) {
        self.last = None;
        self.learned = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_GAP: u64 = 19_200;

    #[test]
    fn learns_the_packet_duration_and_stays_continuous_without_loss() {
        let mut tracker = LossTracker::new(MAX_GAP);
        for timestamp in [0, 960, 1_920, 2_880] {
            assert_eq!(tracker.next(timestamp), LossAction::Continuous);
        }
    }

    #[test]
    fn reports_the_exact_number_of_missing_frames_on_a_gap() {
        let mut tracker = LossTracker::new(MAX_GAP);
        tracker.next(0);
        tracker.next(960);
        assert_eq!(tracker.next(3_840), LossAction::Gap(1_920));
        assert_eq!(tracker.next(4_800), LossAction::Continuous);
    }

    #[test]
    fn resets_on_a_hole_too_wide_to_conceal_keeping_the_learned_duration() {
        let mut tracker = LossTracker::new(MAX_GAP);
        tracker.next(0);
        tracker.next(960);
        assert_eq!(tracker.next(1_920 + 20_000), LossAction::Reset(20_000));
        assert_eq!(tracker.next(1_920 + 20_000 + 960), LossAction::Continuous);
    }

    #[test]
    fn resets_on_a_non_monotonic_timestamp_and_relearns() {
        let mut tracker = LossTracker::new(MAX_GAP);
        tracker.next(0);
        tracker.next(960);
        assert_eq!(tracker.next(0), LossAction::Reset(0));
        assert_eq!(tracker.next(960), LossAction::Continuous);
        assert_eq!(tracker.next(2_880), LossAction::Gap(960));
    }

    #[test]
    fn adopts_a_smaller_delta_when_the_first_was_itself_a_gap() {
        let mut tracker = LossTracker::new(MAX_GAP);
        tracker.next(0);
        assert_eq!(tracker.next(1_920), LossAction::Continuous);
        assert_eq!(tracker.next(2_880), LossAction::Continuous);
        assert_eq!(tracker.next(3_840), LossAction::Continuous);
        assert_eq!(tracker.next(5_760), LossAction::Gap(960));
    }

    #[test]
    fn reset_forgets_history_so_a_rebound_stream_starts_clean() {
        let mut tracker = LossTracker::new(MAX_GAP);
        tracker.next(0);
        tracker.next(960);
        tracker.reset();
        assert_eq!(tracker.next(0), LossAction::Continuous);
        assert_eq!(tracker.next(960), LossAction::Continuous);
        assert_eq!(tracker.packet_frames(), Some(960));
    }
}
