pub struct AutoOff<T> {
    pub pending: Option<T>,
    pub explained: bool,
}

impl<T> AutoOff<T> {
    #[must_use]
    pub fn new(explained: bool) -> Self {
        Self {
            pending: None,
            explained,
        }
    }

    pub fn leave(&mut self, auto: bool, proceed: T) -> Option<T> {
        if !auto || self.explained {
            return Some(proceed);
        }
        self.pending = Some(proceed);
        None
    }

    pub fn confirm(&mut self, never_again: bool) -> Option<T> {
        if never_again {
            self.explained = true;
        }
        self.pending.take()
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunes_at_once_when_the_radio_is_already_manual() {
        let mut gate = AutoOff::new(false);
        assert_eq!(gate.leave(false, 1), Some(1));
        assert!(gate.pending.is_none());
    }

    #[test]
    fn asks_first_on_auto_and_tunes_only_once_confirmed() {
        let mut gate = AutoOff::new(false);
        assert_eq!(gate.leave(true, 1), None);
        assert_eq!(gate.confirm(false), Some(1));
        assert_eq!(gate.leave(true, 2), None);
        assert!(gate.pending.is_some());
    }

    #[test]
    fn drops_the_change_when_cancelled() {
        let mut gate = AutoOff::new(false);
        assert_eq!(gate.leave(true, 1), None);
        gate.cancel();
        assert!(gate.pending.is_none());
        assert_eq!(gate.confirm(false), None);
    }

    #[test]
    fn stops_asking_once_told_never_again() {
        let mut gate = AutoOff::new(false);
        assert_eq!(gate.leave(true, 1), None);
        assert_eq!(gate.confirm(true), Some(1));
        assert_eq!(gate.leave(true, 2), Some(2));
    }
}
