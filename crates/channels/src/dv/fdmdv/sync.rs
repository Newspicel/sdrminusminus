const WORD_LEN: u32 = 6;
const WORD_MASK: u8 = (1 << WORD_LEN) - 1;
const RELIABLE_WORD: u8 = 0b01_0101;
const INVERTED_WORD: u8 = !RELIABLE_WORD & WORD_MASK;
const TENTATIVE_FRAMES: u8 = 25;
const FADE_FRAMES: u8 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lock {
    Search,
    Tentative(u8),
    Locked,
    Fading(u8),
}

pub(super) struct SyncTracker {
    history: u8,
    seen: u32,
    lock: Lock,
}

impl SyncTracker {
    pub(super) fn new() -> Self {
        Self {
            history: 0,
            seen: 0,
            lock: Lock::Search,
        }
    }

    pub(super) fn locked(&self) -> bool {
        self.lock != Lock::Search
    }

    pub(super) fn update(&mut self, sync_bit: bool) -> (bool, bool) {
        self.history = (self.history << 1 | u8::from(sync_bit)) & WORD_MASK;
        self.seen = (self.seen + 1).min(WORD_LEN);
        let complete = self.seen == WORD_LEN;
        let reliable = complete && self.history == RELIABLE_WORD;
        let word = reliable || complete && self.history == INVERTED_WORD;
        self.lock = next(self.lock, word);
        (self.locked(), reliable)
    }
}

fn next(lock: Lock, word: bool) -> Lock {
    match (lock, word) {
        (Lock::Search, true) => Lock::Tentative(0),
        (Lock::Search, false) => Lock::Search,
        (Lock::Tentative(frames), true) if frames + 1 == TENTATIVE_FRAMES => Lock::Locked,
        (Lock::Tentative(frames), true) => Lock::Tentative(frames + 1),
        (Lock::Tentative(_), false) => Lock::Search,
        (Lock::Locked | Lock::Fading(_), true) => Lock::Locked,
        (Lock::Locked, false) => Lock::Fading(0),
        (Lock::Fading(frames), false) if frames + 1 == FADE_FRAMES => Lock::Search,
        (Lock::Fading(frames), false) => Lock::Fading(frames + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tracker: &mut SyncTracker, bits: impl IntoIterator<Item = bool>) -> Vec<(bool, bool)> {
        bits.into_iter().map(|bit| tracker.update(bit)).collect()
    }

    fn alternating(start: bool, len: usize) -> impl Iterator<Item = bool> {
        (0..len).map(move |n| start ^ (n % 2 == 1))
    }

    #[test]
    fn six_alternating_bits_start_a_tentative_lock() {
        let mut tracker = SyncTracker::new();
        let results = feed(&mut tracker, alternating(false, 6));
        assert!(results[..5].iter().all(|&(sync, _)| !sync));
        assert_eq!(results[5], (true, true));
        assert_eq!(tracker.lock, Lock::Tentative(0));
    }

    #[test]
    fn a_partial_word_after_start_up_is_not_a_match() {
        let mut tracker = SyncTracker::new();
        let results = feed(&mut tracker, alternating(true, 5));
        assert!(results.iter().all(|&result| result == (false, false)));
    }

    #[test]
    fn only_the_upright_word_is_reliable() {
        let mut tracker = SyncTracker::new();
        let results = feed(&mut tracker, alternating(true, 7));
        assert_eq!(results[5], (true, false));
        assert_eq!(results[6], (true, true));
    }

    #[test]
    fn lock_is_held_through_a_fade_and_then_dropped() {
        let mut tracker = SyncTracker::new();
        feed(&mut tracker, alternating(false, 6 + 25));
        assert_eq!(tracker.lock, Lock::Locked);
        let fade = feed(&mut tracker, std::iter::repeat_n(false, 51));
        assert!(fade[..50].iter().all(|&(sync, _)| sync));
        assert_eq!(fade[50], (false, false));
    }

    #[test]
    fn a_tentative_lock_falls_back_at_once() {
        let mut tracker = SyncTracker::new();
        feed(&mut tracker, alternating(false, 10));
        assert_eq!(tracker.update(true), (false, false));
    }

    #[test]
    fn the_word_returning_during_a_fade_restores_the_lock() {
        assert_eq!(next(Lock::Fading(30), true), Lock::Locked);
        assert_eq!(next(Lock::Locked, true), Lock::Locked);
        assert_eq!(next(Lock::Tentative(24), true), Lock::Locked);
    }
}
