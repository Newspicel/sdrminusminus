use serde_json::{Value, json};

use super::frame::FrameHeader;

const LOCK_ACQUIRE_N: u32 = 3;
const LOCK_LOSE_M: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CarrierState {
    Searching,
    Acquiring,
    Locked,
}

impl CarrierState {
    fn as_str(self) -> &'static str {
        match self {
            CarrierState::Searching => "searching",
            CarrierState::Acquiring => "acquiring",
            CarrierState::Locked => "locked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LockStatus {
    pub superframe_lock: bool,
    pub dcd: bool,
    pub afc_locked: bool,
    pub carrier_state: CarrierState,
    pub match_count: u32,
    pub miss_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct SuperframeLockStateMachine {
    acquire_n: u32,
    lose_m: u32,
    previous: Option<FrameHeader>,
    match_count: u32,
    miss_count: u32,
    locked: bool,
}

fn header_self_consistent(header: &FrameHeader) -> bool {
    header.frame_counter1 == header.frame_counter2
}

fn is_in_sequence(previous: &FrameHeader, current: &FrameHeader) -> bool {
    if previous.format_id != current.format_id {
        return false;
    }
    let expected = (previous.frame_counter1 + 1) & 0xF;
    if current.frame_counter1 != expected {
        return false;
    }
    if expected == 0 {
        current.superframe == ((previous.superframe + 1) & 0xF)
            || current.superframe == previous.superframe
    } else {
        current.superframe == previous.superframe
    }
}

impl SuperframeLockStateMachine {
    pub(super) fn new() -> Self {
        Self::with_thresholds(LOCK_ACQUIRE_N, LOCK_LOSE_M)
    }

    pub(super) fn with_thresholds(acquire_n: u32, lose_m: u32) -> Self {
        Self {
            acquire_n: acquire_n.max(1),
            lose_m: lose_m.max(1),
            previous: None,
            match_count: 0,
            miss_count: 0,
            locked: false,
        }
    }

    pub(super) fn update(&mut self, header: FrameHeader) -> LockStatus {
        let consistent = header_self_consistent(&header);
        let in_sequence = consistent
            && self
                .previous
                .as_ref()
                .is_some_and(|previous| is_in_sequence(previous, &header));
        if in_sequence {
            self.match_count = self.match_count.saturating_add(1);
            self.miss_count = 0;
            if !self.locked && self.match_count >= self.acquire_n {
                self.locked = true;
            }
        } else {
            self.miss_count = self.miss_count.saturating_add(1);
            self.match_count = 0;
            if self.locked && self.miss_count >= self.lose_m {
                self.locked = false;
            }
        }
        if consistent {
            self.previous = Some(header);
        }
        self.status()
    }

    pub(super) fn status(&self) -> LockStatus {
        let carrier_state = if self.locked {
            CarrierState::Locked
        } else if self.match_count > 0 {
            CarrierState::Acquiring
        } else {
            CarrierState::Searching
        };
        LockStatus {
            superframe_lock: self.locked,
            dcd: carrier_state != CarrierState::Searching,
            afc_locked: self.locked,
            carrier_state,
            match_count: self.match_count,
            miss_count: self.miss_count,
        }
    }

    pub(super) fn details_json(&self) -> Value {
        let status = self.status();
        json!({
            "superframe_lock": status.superframe_lock,
            "dcd": status.dcd,
            "afc_locked": status.afc_locked,
            "carrier_state": status.carrier_state.as_str(),
            "match_count": status.match_count,
            "miss_count": status.miss_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(superframe: u8, counter: u8) -> FrameHeader {
        FrameHeader {
            format_id: 1,
            superframe: superframe & 0xF,
            frame_counter1: counter & 0xF,
            frame_counter2: counter & 0xF,
        }
    }

    #[test]
    fn acquires_lock_after_n_matches() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(3, 4);
        let first = machine.update(header(0, 0));
        assert!(!first.superframe_lock);
        assert_eq!(first.carrier_state, CarrierState::Searching);
        let second = machine.update(header(0, 1));
        assert_eq!(second.match_count, 1);
        assert_eq!(second.carrier_state, CarrierState::Acquiring);
        assert!(second.dcd && !second.afc_locked);
        assert!(!machine.update(header(0, 2)).superframe_lock);
        let fourth = machine.update(header(0, 3));
        assert!(fourth.superframe_lock && fourth.dcd && fourth.afc_locked);
        assert_eq!(fourth.carrier_state, CarrierState::Locked);
    }

    #[test]
    fn loses_lock_after_m_misses() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(3, 4);
        for counter in 0..=4u8 {
            machine.update(header(0, counter));
        }
        for counter in [9u8, 2, 13] {
            assert!(machine.update(header(0, counter)).superframe_lock);
        }
        let lost = machine.update(header(0, 7));
        assert!(!lost.superframe_lock && !lost.afc_locked);
        assert_eq!(lost.carrier_state, CarrierState::Searching);
    }

    #[test]
    fn inconsistent_header_is_a_miss() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(3, 4);
        machine.update(header(0, 0));
        machine.update(header(0, 1));
        let status = machine.update(FrameHeader {
            format_id: 1,
            superframe: 0,
            frame_counter1: 2,
            frame_counter2: 9,
        });
        assert_eq!(status.match_count, 0);
        assert!(!status.superframe_lock);
    }

    #[test]
    fn counter_wrap_stays_in_sequence() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(2, 4);
        machine.update(header(0, 14));
        assert_eq!(machine.update(header(0, 15)).match_count, 1);
        let wrapped = machine.update(header(0, 0));
        assert_eq!(wrapped.match_count, 2);
        assert!(wrapped.superframe_lock);
    }

    #[test]
    fn reacquires_after_loss() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(3, 4);
        for counter in 0..=4u8 {
            machine.update(header(0, counter));
        }
        for counter in [9u8, 2, 13, 7] {
            machine.update(header(0, counter));
        }
        assert!(!machine.status().superframe_lock);
        let relocked = (8..=11u8)
            .map(|counter| machine.update(header(0, counter)).superframe_lock)
            .last();
        assert_eq!(relocked, Some(true));
    }

    #[test]
    fn details_json_shape() {
        let mut machine = SuperframeLockStateMachine::new();
        for counter in 0..=4u8 {
            machine.update(header(0, counter));
        }
        let json = machine.details_json();
        assert_eq!(json["superframe_lock"], true);
        assert_eq!(json["carrier_state"], "locked");
        assert_eq!(json["afc_locked"], true);
        assert_eq!(json["dcd"], true);
    }
}
