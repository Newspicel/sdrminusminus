//! Which directions a radio can run, and which of them are running now (PLAN §6).
//!
//! Every backend needs the same arbitration and every backend would otherwise hand-roll it: the
//! HackRF's single transceiver selects one data path at a time, an RTL-SDR has no transmitter at
//! all, and a USRP-class radio runs both at once. That is three rules, not three
//! implementations, so [`DuplexState`] owns them for all of them.
//!
//! Pure: no I/O, no device, no clock. The *mechanics* of pointing a radio one way stay in its
//! backend, because they differ; deciding whether it is allowed to does not.

use crate::DeviceError;

/// One signal direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Radio to host.
    Rx,
    /// Host to radio.
    Tx,
}

impl Direction {
    /// The other one.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Rx => Self::Tx,
            Self::Tx => Self::Rx,
        }
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rx => "receiving",
            Self::Tx => "transmitting",
        })
    }
}

/// What a radio's hardware can do, and whether it can do it at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Duplex {
    /// Receive only: RTL-SDR, and every backend with no transmitter (virtual, playback, the
    /// Soapy path as this project drives it). The default, so a backend that says nothing
    /// cannot accidentally advertise a transmitter.
    #[default]
    RxOnly,
    /// Transmit only: a bench signal generator with no receive path.
    TxOnly,
    /// Both, one at a time: the HackRF, whose LPC transceiver mode selects a single data path,
    /// so changing direction means stopping the other first.
    Half,
    /// Both at once: USRP, LimeSDR, PlutoSDR, bladeRF.
    Full,
}

impl Duplex {
    /// Whether the hardware has this direction at all.
    #[must_use]
    pub const fn supports(self, direction: Direction) -> bool {
        match (self, direction) {
            (Self::RxOnly, Direction::Rx)
            | (Self::TxOnly, Direction::Tx)
            | (Self::Half | Self::Full, _) => true,
            (Self::RxOnly, Direction::Tx) | (Self::TxOnly, Direction::Rx) => false,
        }
    }

    /// Whether both directions can be live together.
    #[must_use]
    pub const fn simultaneous(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Which directions are claimed right now, and the rule that decides whether one more may be.
///
/// A claim is held for as long as a stream runs and released when it ends. Releasing one
/// direction never touches the other — that is the whole point of tracking them separately, and
/// the bug this type replaces: a `stop` that cleared "the active direction" would silence a
/// transmit burst that a *receive* teardown had no business ending.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DuplexState {
    duplex: Duplex,
    rx: bool,
    tx: bool,
}

impl DuplexState {
    /// A radio with nothing claimed.
    #[must_use]
    pub const fn new(duplex: Duplex) -> Self {
        Self {
            duplex,
            rx: false,
            tx: false,
        }
    }

    /// What the hardware can do.
    #[must_use]
    pub const fn duplex(&self) -> Duplex {
        self.duplex
    }

    /// Whether `direction` is claimed.
    #[must_use]
    pub const fn is_active(&self, direction: Direction) -> bool {
        match direction {
            Direction::Rx => self.rx,
            Direction::Tx => self.tx,
        }
    }

    /// Whether nothing at all is running.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        !self.rx && !self.tx
    }

    /// Take `direction` for a stream that is about to start.
    ///
    /// # Errors
    /// [`DeviceError::Unsupported`] if the hardware has no such direction,
    /// [`DeviceError::AlreadyStreaming`] if it is already claimed, and
    /// [`DeviceError::DuplexConflict`] if the other direction holds a half-duplex radio.
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

    /// Give `direction` back. Idempotent, and never clears the other one.
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

    /// The HackRF rule: one transceiver, one data path, so the second direction has to wait.
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

    /// The finding this type exists for: tearing down a receive stream must not silence a
    /// transmit burst that is still on the air.
    #[test]
    fn releasing_one_direction_leaves_the_other_claimed() {
        let mut state = DuplexState::new(Duplex::Full);
        state.claim(Direction::Tx).expect("tx");
        state.claim(Direction::Rx).expect("rx");
        state.release(Direction::Rx);
        assert!(state.is_active(Direction::Tx), "tx must survive an rx stop");
        // Releasing a direction that was never claimed is a no-op, not a way to clear the other.
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
