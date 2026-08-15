use std::{collections::VecDeque, f64::consts::PI};

use crate::ChannelError;

const RAMP_MS: f64 = 2.0;

pub(crate) const MAX_QUEUE_S: f64 = 5.0;

pub(crate) struct TxQueue<T> {
    mode: &'static str,
    capacity: usize,
    items: VecDeque<T>,
}

impl<T> TxQueue<T> {
    pub(crate) fn new(mode: &'static str, rate: f64) -> Self {
        Self {
            mode,
            capacity: (MAX_QUEUE_S * rate) as usize,
            items: VecDeque::new(),
        }
    }

    pub(crate) fn accept(&self, n: usize) -> Result<(), ChannelError> {
        if self.items.len() + n <= self.capacity {
            return Ok(());
        }
        Err(ChannelError::InvalidPayload(format!(
            "{} transmit queue holds at most {MAX_QUEUE_S} s, {} items queued",
            self.mode,
            self.items.len()
        )))
    }

    pub(crate) fn push(&mut self, item: T) {
        self.items.push_back(item);
    }

    pub(crate) fn extend(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
    }

    pub(crate) fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}

enum State {
    Idle,
    Rising(usize),
    Running,
    Falling(usize),
}

pub(crate) struct Burst {
    ramp_len: usize,
    state: State,
}

impl Burst {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            ramp_len: (RAMP_MS / 1_000.0 * rate) as usize,
            state: State::Idle,
        }
    }

    #[cfg(test)]
    pub(crate) fn ramp_len(&self) -> usize {
        self.ramp_len
    }

    pub(crate) fn next(&mut self, more: bool) -> Option<f32> {
        match self.state {
            State::Idle if !more => None,
            State::Idle => {
                self.state = State::Rising(1);
                Some(self.ramp(0))
            }
            State::Running if !more => {
                self.state = State::Falling(1);
                Some(self.ramp(self.ramp_len - 1))
            }
            State::Running => Some(1.0),
            State::Rising(k) => {
                self.state = if k + 1 < self.ramp_len {
                    State::Rising(k + 1)
                } else {
                    State::Running
                };
                Some(self.ramp(k))
            }
            State::Falling(k) => {
                self.state = if k + 1 < self.ramp_len {
                    State::Falling(k + 1)
                } else {
                    State::Idle
                };
                Some(self.ramp(self.ramp_len - 1 - k))
            }
        }
    }

    fn ramp(&self, k: usize) -> f32 {
        (0.5 * (1.0 - (PI * (k + 1) as f64 / self.ramp_len as f64).cos())) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    #[test]
    fn an_empty_payload_never_keys_the_carrier() {
        let mut burst = Burst::new(RATE);
        assert_eq!(burst.next(false), None);
        assert_eq!(burst.next(false), None);
    }

    #[test]
    fn the_envelope_rises_holds_and_falls_to_silence() {
        let mut burst = Burst::new(RATE);
        let ramp = burst.ramp_len();
        let rise: Vec<f32> = (0..ramp).map(|_| burst.next(true).unwrap()).collect();
        assert!(rise[0] < 0.01, "first sample {}", rise[0]);
        assert!(rise.windows(2).all(|p| p[1] > p[0]), "rise not monotonic");
        assert!(
            (rise[ramp - 1] - 1.0).abs() < 1e-6,
            "peak {}",
            rise[ramp - 1]
        );
        assert_eq!(burst.next(true), Some(1.0));

        let fall: Vec<f32> = std::iter::from_fn(|| burst.next(false)).collect();
        assert_eq!(fall.len(), ramp);
        assert!(fall.windows(2).all(|p| p[1] < p[0]), "fall not monotonic");
        assert!(fall[ramp - 1] < 1e-3, "last sample {}", fall[ramp - 1]);
        assert_eq!(burst.next(false), None);
    }

    #[test]
    fn the_queue_refuses_a_backlog_past_the_bound_without_taking_any_of_it() {
        let mut queue = TxQueue::new("test", 10.0);
        let capacity = (MAX_QUEUE_S * 10.0) as usize;
        assert!(queue.accept(capacity).is_ok());
        queue.extend(std::iter::repeat_n(1u8, capacity));
        assert!(matches!(
            queue.accept(1),
            Err(ChannelError::InvalidPayload(_))
        ));
        assert_eq!(queue.pop(), Some(1));
    }
}
