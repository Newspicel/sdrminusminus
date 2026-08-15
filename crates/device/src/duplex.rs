use sdrmm_wire::{Direction, Duplex};

use crate::DeviceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DuplexState {
    duplex: Duplex,
    rx: bool,
    tx: bool,
}

impl DuplexState {
    #[must_use]
    pub const fn new(duplex: Duplex) -> Self {
        Self {
            duplex,
            rx: false,
            tx: false,
        }
    }

    #[must_use]
    pub const fn duplex(&self) -> Duplex {
        self.duplex
    }

    #[must_use]
    pub const fn is_active(&self, direction: Direction) -> bool {
        match direction {
            Direction::Rx => self.rx,
            Direction::Tx => self.tx,
        }
    }

    #[must_use]
    pub const fn is_idle(&self) -> bool {
        !self.rx && !self.tx
    }

    pub fn claim(&mut self, direction: Direction) -> Result<(), DeviceError> {
        if !self.duplex.supports(direction) {
            return Err(DeviceError::Unsupported(format!(
                "this device does not support {direction}"
            )));
        }
        if self.is_active(direction) {
            return Err(DeviceError::AlreadyStreaming);
        }
        let other = direction.opposite();
        if self.is_active(other) && !self.duplex.simultaneous() {
            return Err(DeviceError::DuplexConflict {
                active: other,
                requested: direction,
            });
        }
        self.set(direction, true);
        Ok(())
    }

    pub const fn release(&mut self, direction: Direction) {
        self.set(direction, false);
    }

    const fn set(&mut self, direction: Direction, held: bool) {
        match direction {
            Direction::Rx => self.rx = held,
            Direction::Tx => self.tx = held,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rx_only_radio_refuses_transmit_outright() {
        let mut state = DuplexState::new(Duplex::RxOnly);
        assert!(matches!(
            state.claim(Direction::Tx),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(state.claim(Direction::Rx).is_ok());
        assert!(state.is_active(Direction::Rx));
    }

    #[test]
    fn a_tx_only_radio_refuses_receive_outright() {
        let mut state = DuplexState::new(Duplex::TxOnly);
        assert!(matches!(
            state.claim(Direction::Rx),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(state.claim(Direction::Tx).is_ok());
    }

    #[test]
    fn half_duplex_admits_one_direction_at_a_time() {
        let mut state = DuplexState::new(Duplex::Half);
        state.claim(Direction::Rx).expect("first claim");
        assert!(matches!(
            state.claim(Direction::Tx),
            Err(DeviceError::DuplexConflict {
                active: Direction::Rx,
                requested: Direction::Tx
            })
        ));
        state.release(Direction::Rx);
        state.claim(Direction::Tx).expect("the path is free now");
        assert!(matches!(
            state.claim(Direction::Rx),
            Err(DeviceError::DuplexConflict { .. })
        ));
    }

    #[test]
    fn full_duplex_admits_both_at_once() {
        let mut state = DuplexState::new(Duplex::Full);
        state.claim(Direction::Rx).expect("rx");
        state.claim(Direction::Tx).expect("tx alongside rx");
        assert!(!state.is_idle());
        state.release(Direction::Rx);
        assert!(state.is_active(Direction::Tx));
    }

    #[test]
    fn claiming_the_same_direction_twice_is_already_streaming() {
        for duplex in [Duplex::RxOnly, Duplex::Half, Duplex::Full] {
            let mut state = DuplexState::new(duplex);
            state.claim(Direction::Rx).expect("first");
            assert!(
                matches!(
                    state.claim(Direction::Rx),
                    Err(DeviceError::AlreadyStreaming)
                ),
                "{duplex:?}"
            );
        }
    }

    #[test]
    fn releasing_one_direction_leaves_the_other_claimed() {
        let mut state = DuplexState::new(Duplex::Full);
        state.claim(Direction::Tx).expect("tx");
        state.claim(Direction::Rx).expect("rx");
        state.release(Direction::Rx);
        assert!(state.is_active(Direction::Tx), "tx must survive an rx stop");
        state.release(Direction::Rx);
        assert!(state.is_active(Direction::Tx));
        state.release(Direction::Tx);
        assert!(state.is_idle());
    }

    #[test]
    fn a_backend_that_declares_nothing_is_receive_only() {
        assert_eq!(Duplex::default(), Duplex::RxOnly);
        assert!(Duplex::default().supports(Direction::Rx));
        assert!(!Duplex::default().supports(Direction::Tx));
        assert!(!Duplex::default().simultaneous());
    }
}
