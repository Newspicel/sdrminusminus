use sdrmm_device::DeviceError;

use crate::{
    control::{Control, Target},
    rate::Stage,
    regs::{self, radio},
};

pub(crate) const RX_STREAM_IDS: [u32; 2] = [0xa0, 0xb0];
pub(crate) const TX_STREAM_IDS: [u32; 2] = [0x50, 0x60];
const INVERT_I_AND_Q: u32 = 0x0c;
const NEXT_PACKET_ON_UNDERFLOW: u32 = 1 << 1;
const ACK_EVERY_PACKETS: u32 = 30;
const ENABLED: u32 = 1 << 31;
const LATCH_SYNC: u32 = 1 << 2;
const SELF_TEST_ROUNDS: u32 = 16;
const STREAM_NOW: u32 = 1 << 31;
const CHAIN: u32 = 1 << 30;
const RELOAD: u32 = 1 << 29;
const STOP: u32 = 1 << 28;

const TX_ENABLE: u32 = 1 << 7;
const DUPLEX_RX_SWITCH: u32 = 1 << 6;
const DUPLEX_TX_SWITCH: u32 = 1 << 5;
const RX_ONLY_TX_SWITCH: u32 = 1 << 3;
const RX_LED: u32 = 1 << 2;
const TX_LED: u32 = 1 << 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    StartNow,
    StartAt(u64),
    Stop,
}

impl Command {
    const fn word(self) -> u32 {
        match self {
            Self::StartNow => STREAM_NOW | CHAIN | RELOAD | 1,
            Self::StartAt(_) => CHAIN | RELOAD | 1,
            Self::Stop => STREAM_NOW | STOP,
        }
    }

    const fn time(self) -> u64 {
        match self {
            Self::StartAt(ticks) => ticks,
            Self::StartNow | Self::Stop => 0,
        }
    }
}

pub(crate) fn stream(control: &Control, lane: usize, command: Command) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    let time = command.time();
    control.poke(target, radio::RX_COMMAND, command.word())?;
    control.poke(target, radio::RX_COMMAND_TIME_HI, (time >> 32) as u32)?;
    control.poke(target, radio::RX_COMMAND_TIME_LO, time as u32)?;
    control.settle()
}

pub(crate) fn frame_rx(control: &Control, lane: usize, samples: usize) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    control.poke(target, radio::RX_FORMAT, 0)?;
    control.poke(target, radio::RX_PACKET_SAMPLES, samples as u32)?;
    control.poke(target, radio::RX_STREAM_ID, RX_STREAM_IDS[lane])
}

pub(crate) fn frame_tx(control: &Control, lane: usize) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    control.poke(target, radio::TX_FORMAT, 0)?;
    control.poke(target, radio::TX_ERROR_POLICY, NEXT_PACKET_ON_UNDERFLOW)?;
    control.poke(target, radio::TX_ACK_CYCLES, 0)?;
    control.poke(target, radio::TX_ACK_PACKETS, ENABLED | ACK_EVERY_PACKETS)
}

pub(crate) fn set_decimator(
    control: &Control,
    lane: usize,
    stage: Stage,
) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    control.poke(target, radio::DDC_DECIMATION, stage.word)?;
    control.poke(target, radio::DDC_SCALE, stage.scale)?;
    control.poke(target, radio::DDC_FREQUENCY, 0)
}

pub(crate) fn set_interpolator(
    control: &Control,
    lane: usize,
    stage: Stage,
) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    control.poke(target, radio::DUC_INTERPOLATION, stage.word)?;
    control.poke(target, radio::DUC_SCALE, stage.scale)?;
    control.poke(target, radio::DUC_FREQUENCY, 0)
}

pub(crate) fn prepare(control: &Control, lane: usize) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    control.poke(target, radio::ATR_DISABLE, 0)?;
    control.poke(
        target,
        radio::DDC_MUX,
        if lane == 1 { INVERT_I_AND_Q } else { 0 },
    )?;
    set_switches(control, lane, false, false)
}

pub(crate) const fn switches(lane: usize, rx: bool, tx: bool) -> [u32; 4] {
    let rx_only = if !rx {
        0
    } else if lane == 0 {
        DUPLEX_RX_SWITCH | DUPLEX_TX_SWITCH | RX_LED
    } else {
        DUPLEX_RX_SWITCH | RX_ONLY_TX_SWITCH | RX_LED
    };
    let tx_only = if tx {
        TX_ENABLE | DUPLEX_RX_SWITCH | DUPLEX_TX_SWITCH | TX_LED
    } else {
        0
    };
    let duplex = match (rx, tx) {
        (true, true) => TX_ENABLE | DUPLEX_RX_SWITCH | DUPLEX_TX_SWITCH | TX_LED | RX_LED,
        (true, false) => rx_only,
        (false, true) => tx_only,
        (false, false) => 0,
    };
    [0, rx_only, tx_only, duplex]
}

pub(crate) fn set_switches(
    control: &Control,
    lane: usize,
    rx: bool,
    tx: bool,
) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    let [idle, rx_only, tx_only, duplex] = switches(lane, rx, tx);
    control.poke(target, radio::ATR_IDLE, idle)?;
    control.poke(target, radio::ATR_RX, rx_only)?;
    control.poke(target, radio::ATR_TX, tx_only)?;
    control.poke(target, radio::ATR_DUPLEX, duplex)
}

pub(crate) fn register_self_test(control: &Control, lane: usize) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    let mut word = 0x1234_5678u32 ^ lane as u32;
    for _ in 0..SELF_TEST_ROUNDS {
        word = word.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        control.poke(target, radio::TEST, word)?;
        let back = control.peek32(target, regs::TEST_READBACK)?;
        if back != word {
            return Err(DeviceError::Io(format!(
                "radio {lane} register test wrote {word:#010x} and read {back:#010x}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn interface_self_test(control: &Control, lane: usize) -> Result<(), DeviceError> {
    let target = Target::Radio(lane);
    let mut word = 0x0bad_f00du32 ^ lane as u32;
    for _ in 0..SELF_TEST_ROUNDS {
        word = word.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let pattern = word & 0xfff0_fff0;
        control.poke(target, radio::CODEC_IDLE, pattern)?;
        let back = control.peek64(target, regs::CODEC_READBACK)?;
        let (tx, rx) = ((back >> 32) as u32, back as u32);
        if tx != pattern || rx != pattern {
            return Err(DeviceError::Io(format!(
                "radio {lane} sample interface sent {pattern:#010x}, saw {tx:#010x} and {rx:#010x}"
            )));
        }
    }
    control.poke(target, radio::CODEC_IDLE, 0)
}

pub(crate) fn zero_time(control: &Control, lanes: usize, source: u32) -> Result<(), DeviceError> {
    for lane in 0..lanes {
        let target = Target::Radio(lane);
        control.poke(target, radio::TIME_HI, 0)?;
        control.poke(target, radio::TIME_LO, 0)?;
        control.poke(target, radio::TIME_CONTROL, LATCH_SYNC)?;
    }
    control.poke(Target::Local, regs::core::SYNC, LATCH_SYNC | source)?;
    control.poke(Target::Local, regs::core::SYNC, source)?;
    control.settle()
}

pub(crate) fn time_now(control: &Control) -> Result<u64, DeviceError> {
    control.peek64(Target::Radio(0), regs::TIME_NOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_commands_carry_the_framer_instruction_bits() {
        assert_eq!(Command::StartNow.word(), 0xe000_0001);
        assert_eq!(Command::StartAt(9).word(), 0x6000_0001);
        assert_eq!(Command::StartAt(9).time(), 9);
        assert_eq!(Command::Stop.word(), 0x9000_0000);
    }

    #[test]
    fn the_front_end_switches_follow_what_each_lane_does() {
        assert_eq!(switches(0, true, false), [0, 0x64, 0, 0x64]);
        assert_eq!(switches(1, true, false), [0, 0x4c, 0, 0x4c]);
        assert_eq!(switches(0, false, true), [0, 0, 0xe1, 0xe1]);
        assert_eq!(switches(0, true, true), [0, 0x64, 0xe1, 0xe5]);
        assert_eq!(switches(1, false, false), [0; 4]);
    }
}
