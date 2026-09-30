use std::sync::{Arc, Mutex};

use crate::{DeviceError, lock};

pub const UNKNOWN_ERROR: u64 = u64::MAX;
pub const MARK_SLOTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneMark {
    NoiseSource { on: bool, in_flight: u64 },
    Retuned { in_flight: u64 },
    GainChanged { in_flight: u64 },
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uncertainty {
    EstimatedGap,
    Rearmed,
    RateWrite,
    Overflow,
    Reset,
    Unaligned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapScope {
    Lane,
    Device,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneEvent {
    Mark {
        at: u64,
        mark: LaneMark,
    },
    Uncertain {
        at: u64,
        error: u64,
        scope: GapScope,
        cause: Uncertainty,
    },
    HardwareTime {
        at: u64,
        ns: i64,
    },
}

#[derive(Clone)]
pub struct MarkPoster(Arc<Mutex<rtrb::Producer<LaneMark>>>);

impl MarkPoster {
    pub(crate) fn channel() -> (Self, rtrb::Consumer<LaneMark>) {
        let (producer, consumer) = rtrb::RingBuffer::new(MARK_SLOTS);
        (Self(Arc::new(Mutex::new(producer))), consumer)
    }

    pub fn post(&self, mark: LaneMark) -> Result<(), DeviceError> {
        let mut marks = lock(&self.0);
        if marks.is_abandoned() {
            return Err(DeviceError::Io("lane stopped".to_string()));
        }
        marks
            .push(mark)
            .map_err(|_| DeviceError::Io("lane marks full".to_string()))
    }
}

impl std::fmt::Debug for MarkPoster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MarkPoster")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poster_fills_exactly_its_slots() {
        let (poster, mut marks) = MarkPoster::channel();
        let mark = LaneMark::Retuned { in_flight: 7 };
        for _ in 0..MARK_SLOTS {
            poster.post(mark).expect("a free slot");
        }
        assert!(poster.post(mark).is_err());
        assert_eq!(marks.pop(), Ok(mark));
        poster.post(mark).expect("a freed slot");
    }

    #[test]
    fn a_mark_for_a_stopped_lane_is_an_error_not_a_loss() {
        let (poster, marks) = MarkPoster::channel();
        drop(marks);
        match poster.post(LaneMark::Retuned { in_flight: 0 }) {
            Err(DeviceError::Io(message)) => assert_eq!(message, "lane stopped"),
            other => panic!("a stopped lane must refuse marks, got {other:?}"),
        }
    }

    #[test]
    fn a_cloned_poster_feeds_the_same_lane() {
        let (poster, mut marks) = MarkPoster::channel();
        let other = poster.clone();
        poster
            .post(LaneMark::GainChanged { in_flight: 1 })
            .expect("post");
        other
            .post(LaneMark::NoiseSource {
                on: true,
                in_flight: 2,
            })
            .expect("post");
        assert_eq!(marks.pop(), Ok(LaneMark::GainChanged { in_flight: 1 }));
        assert_eq!(
            marks.pop(),
            Ok(LaneMark::NoiseSource {
                on: true,
                in_flight: 2
            })
        );
    }
}
