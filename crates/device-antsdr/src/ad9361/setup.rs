use super::Direction;

pub(super) const ENSM_WAIT: u8 = 0x00;
pub(super) const ENSM_ALERT: u8 = 0x01;
pub(super) const ENSM_ALERT_TX_ON: u8 = 0x05;
pub(super) const ENSM_DUPLEX: u8 = 0x21;
pub(super) const ENSM_ENABLE: u8 = 0x01;
pub(super) const DUAL_SYNTHESIZER: u8 = 0x04;
pub(super) const PARALLEL_PORT_READY: u8 = 0x02;

pub(super) const POWER_UP: &[(u16, u8)] = &[
    (0x3df, 0x01),
    (0x2a6, 0x0e),
    (0x2a8, 0x0e),
    (0x2ab, 0x07),
    (0x2ac, 0xff),
    (0x009, 0x17),
];

pub(super) const PARALLEL_PORT: &[(u16, u8)] = &[(0x010, 0xc8), (0x011, 0x00), (0x012, 0x02)];

pub(super) const INTERFACE_DELAYS: &[(u16, u8)] = &[(0x006, 0x0f), (0x007, 0x0f)];

pub(super) const AUX_DAC: &[(u16, u8)] = &[
    (0x018, 0x00),
    (0x019, 0x00),
    (0x01a, 0x00),
    (0x01b, 0x00),
    (0x023, 0xff),
    (0x026, 0x00),
    (0x030, 0x00),
    (0x031, 0x00),
    (0x032, 0x00),
    (0x033, 0x00),
    (0x022, 0x0a),
];

pub(super) const AUX_ADC: &[(u16, u8)] = &[
    (0x00b, 0x00),
    (0x00c, 0x00),
    (0x00d, 0x00),
    (0x00f, 0x04),
    (0x01c, 0x10),
    (0x01d, 0x01),
];

pub(super) const CONTROL_OUTPUTS: &[(u16, u8)] = &[(0x035, 0x01), (0x036, 0xff)];

pub(super) const GPO: &[(u16, u8)] = &[
    (0x03a, 0x27),
    (0x020, 0x00),
    (0x027, 0x03),
    (0x028, 0x00),
    (0x029, 0x00),
    (0x02a, 0x00),
    (0x02b, 0x00),
    (0x02c, 0x00),
    (0x02d, 0x00),
    (0x02e, 0x00),
    (0x02f, 0x00),
];

pub(super) const SYNTHESIZERS: &[(u16, u8)] = &[
    (0x261, 0x00),
    (0x2a1, 0x00),
    (0x248, 0x0b),
    (0x288, 0x0b),
    (0x246, 0x02),
    (0x286, 0x02),
    (0x249, 0x8e),
    (0x289, 0x8e),
    (0x23b, 0x80),
    (0x27b, 0x80),
    (0x243, 0x0d),
    (0x283, 0x0d),
    (0x23d, 0x00),
    (0x27d, 0x00),
];

pub(super) const BBPLL_LOOP_FILTER: &[(u16, u8)] = &[
    (0x048, 0xe8),
    (0x049, 0x5b),
    (0x04a, 0x35),
    (0x04b, 0xe0),
    (0x04e, 0x10),
];

pub(super) const RSSI: &[(u16, u8)] = &[
    (0x150, 0x0e),
    (0x151, 0x00),
    (0x152, 0xff),
    (0x153, 0x00),
    (0x154, 0x00),
    (0x155, 0x00),
    (0x156, 0x00),
    (0x157, 0x00),
    (0x158, 0x0d),
    (0x15c, 0x67),
];

pub(super) const MANUAL_GAIN: &[(u16, u8)] = &[
    (0x0fa, 0xe0),
    (0x0fb, 0x08),
    (0x0fc, 0x23),
    (0x0fd, 0x4c),
    (0x0fe, 0x44),
    (0x100, 0x6f),
    (0x104, 0x2f),
    (0x105, 0x3a),
    (0x107, 0x31),
    (0x108, 0x39),
    (0x109, 0x23),
    (0x10a, 0x58),
    (0x10b, 0x00),
    (0x10c, 0x23),
    (0x10d, 0x18),
    (0x10e, 0x00),
    (0x114, 0x30),
    (0x11a, 0x27),
    (0x081, 0x00),
];

pub(super) const AUTOMATIC_GAIN: &[(u16, u8)] = &[
    (0x0fb, 0x08),
    (0x0fc, 0x23),
    (0x0fd, 0x4c),
    (0x0fe, 0x44),
    (0x100, 0x6f),
    (0x101, 0x0a),
    (0x103, 0x08),
    (0x104, 0x2f),
    (0x105, 0x3a),
    (0x106, 0x22),
    (0x107, 0x2b),
    (0x108, 0x31),
    (0x111, 0x0a),
    (0x11a, 0x1c),
    (0x120, 0x0c),
    (0x121, 0x44),
    (0x122, 0x44),
    (0x123, 0x11),
    (0x124, 0xf5),
    (0x125, 0x3b),
    (0x128, 0x03),
    (0x129, 0x56),
    (0x12a, 0x22),
];

pub(super) const BB_DC_CALIBRATION: &[(u16, u8)] =
    &[(0x18b, 0x83), (0x193, 0x3f), (0x190, 0x0f), (0x194, 0x01)];

pub(super) const RX_QUADRATURE_CALIBRATION: &[(u16, u8)] = &[
    (0x168, 0x03),
    (0x16e, 0x25),
    (0x16a, 0x75),
    (0x16b, 0x95),
    (0x057, 0x33),
    (0x169, 0xc0),
];

pub(super) const TX_QUADRATURE_CALIBRATION: &[(u16, u8)] = &[
    (0x0a1, 0x7b),
    (0x0a9, 0xff),
    (0x0a2, 0x7f),
    (0x0a5, 0x01),
    (0x0a6, 0x01),
];

pub(super) const TX_QUADRATURE_FINISH: &[(u16, u8)] = &[(0x0a4, 0xf0), (0x0ae, 0x00)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FilterChain {
    pub(super) rx: u8,
    pub(super) tx: u8,
    pub(super) divider: u32,
    pub(super) fir_factor: u32,
}

const fn chain(rx: u8, tx: u8, divider: u32, fir_factor: u32) -> FilterChain {
    FilterChain {
        rx,
        tx,
        divider,
        fir_factor,
    }
}

pub(super) fn filter_chain(rate: f64) -> FilterChain {
    if rate < 0.33e6 {
        chain(0xef, 0xef, 48, 4)
    } else if rate < 0.66e6 {
        chain(0xdf, 0xdf, 32, 4)
    } else if rate <= 20e6 {
        chain(0xde, 0xde, 16, 2)
    } else if rate < 23e6 {
        chain(0xee, 0xe6, 24, 2)
    } else if rate < 41e6 {
        chain(0xde, 0xce, 16, 2)
    } else if rate <= 58e6 {
        chain(0xe6, 0xe2, 12, 2)
    } else {
        chain(0xce, 0xd2, 8, 2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Synthesizer {
    pub(super) vco_output: u16,
    pub(super) vco_varactor: u16,
    pub(super) vco_bias: u16,
    pub(super) vco_cal_offset: u16,
    pub(super) vco_varactor_control: u16,
    pub(super) vco_varactor_ref: u16,
    pub(super) vco_varactor_ref_tcf: u16,
    pub(super) charge_pump: u16,
    pub(super) loop_filter_1: u16,
    pub(super) loop_filter_2: u16,
    pub(super) loop_filter_3: u16,
    pub(super) fraction_low: u16,
    pub(super) fraction_mid: u16,
    pub(super) fraction_high: u16,
    pub(super) integer_high: u16,
    pub(super) integer_low: u16,
    pub(super) lock: u16,
}

impl Synthesizer {
    pub(super) const fn of(direction: Direction) -> Self {
        let base = match direction {
            Direction::Rx => 0x200,
            Direction::Tx => 0x240,
        };
        Self {
            vco_output: base + 0x3a,
            vco_varactor: base + 0x39,
            vco_bias: base + 0x42,
            vco_cal_offset: base + 0x38,
            vco_varactor_control: base + 0x45,
            vco_varactor_ref: base + 0x51,
            vco_varactor_ref_tcf: base + 0x50,
            charge_pump: base + 0x3b,
            loop_filter_1: base + 0x3e,
            loop_filter_2: base + 0x3f,
            loop_filter_3: base + 0x40,
            fraction_low: base + 0x33,
            fraction_mid: base + 0x34,
            fraction_high: base + 0x35,
            integer_high: base + 0x32,
            integer_low: base + 0x31,
            lock: base + 0x47,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_chain_keeps_the_converter_inside_its_clock_range() {
        for rate in [0.25e6, 0.5e6, 2e6, 16e6, 20e6, 21e6, 30.72e6, 50e6, 61.44e6] {
            let chain = filter_chain(rate);
            let adc = rate * f64::from(chain.divider);
            assert!((10e6..=715e6).contains(&adc), "{rate}: {adc}");
            assert_eq!(chain.rx & 0xc0, 0xc0, "both lanes run while calibrating");
        }
    }

    #[test]
    fn the_transmit_synthesizer_mirrors_the_receive_one() {
        let rx = Synthesizer::of(Direction::Rx);
        let tx = Synthesizer::of(Direction::Tx);
        assert_eq!(rx.lock, 0x247);
        assert_eq!(tx.lock, 0x287);
        assert_eq!(tx.fraction_low - rx.fraction_low, 0x40);
    }
}
