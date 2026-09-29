pub(crate) const READBACK: u32 = 32;

pub(crate) mod core {
    pub(crate) const SPI_DIVIDER: u32 = 8;
    pub(crate) const SPI_CONTROL: u32 = 9;
    pub(crate) const SPI_DATA: u32 = 10;
    pub(crate) const MISC: u32 = 16;
    pub(crate) const SYNC: u32 = 48;
}

pub(crate) mod radio {
    pub(crate) const ATR_IDLE: u32 = 12;
    pub(crate) const ATR_RX: u32 = 13;
    pub(crate) const ATR_TX: u32 = 14;
    pub(crate) const ATR_DUPLEX: u32 = 15;
    pub(crate) const ATR_DISABLE: u32 = 17;
    pub(crate) const TEST: u32 = 21;
    pub(crate) const CODEC_IDLE: u32 = 22;
    pub(crate) const TX_ERROR_POLICY: u32 = 64;
    pub(crate) const TX_ACK_CYCLES: u32 = 66;
    pub(crate) const TX_ACK_PACKETS: u32 = 67;
    pub(crate) const RX_COMMAND: u32 = 96;
    pub(crate) const RX_COMMAND_TIME_HI: u32 = 97;
    pub(crate) const RX_COMMAND_TIME_LO: u32 = 98;
    pub(crate) const RX_PACKET_SAMPLES: u32 = 100;
    pub(crate) const RX_STREAM_ID: u32 = 101;
    pub(crate) const TIME_HI: u32 = 128;
    pub(crate) const TIME_LO: u32 = 129;
    pub(crate) const TIME_CONTROL: u32 = 130;
    pub(crate) const RX_FORMAT: u32 = 136;
    pub(crate) const TX_FORMAT: u32 = 138;
    pub(crate) const DDC_FREQUENCY: u32 = 144;
    pub(crate) const DDC_SCALE: u32 = 145;
    pub(crate) const DDC_DECIMATION: u32 = 146;
    pub(crate) const DDC_MUX: u32 = 147;
    pub(crate) const DUC_FREQUENCY: u32 = 184;
    pub(crate) const DUC_SCALE: u32 = 185;
    pub(crate) const DUC_INTERPOLATION: u32 = 186;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Half {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Readback {
    pub(crate) word: u32,
    pub(crate) half: Half,
}

const fn low(word: u32) -> Readback {
    Readback {
        word,
        half: Half::Low,
    }
}

pub(crate) const COMPAT: u32 = 0;
pub(crate) const SPI_READBACK: Readback = low(1);
pub(crate) const STATUS_READBACK: Readback = Readback {
    word: 2,
    half: Half::High,
};
pub(crate) const TEST_READBACK: Readback = low(0);
pub(crate) const TIME_NOW: u32 = 1;
pub(crate) const CODEC_READBACK: u32 = 3;

pub(crate) const SIGNATURE: u32 = 0xACE0_BA5E;
pub(crate) const FPGA_COMPAT: u16 = 16;
