use std::f64::consts::{LN_2, PI};

const RX_FILTER_SPAN: (f64, f64) = (0.143e6, 28e6);
const TX_FILTER_SPAN: (f64, f64) = (0.391e6, 20e6);
const TX_SECONDARY_SPAN: (f64, f64) = (0.54e6, 20e6);
const TIA_SPAN: (f64, f64) = (0.4e6, 28e6);
const ADC_SPAN_MHZ: (f64, f64) = (0.2, 28.0);
const RX_FILTER_MARGIN: f64 = 1.4;
const TX_FILTER_MARGIN: f64 = 1.6;
const TX_SECONDARY_MARGIN: f64 = 5.0;
const TUNE_DIVIDER_MAX: u16 = 511;
const KHZ_STEP: f64 = 7.8125;

fn byte(value: f64) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn half_band(rf_bandwidth: f64, baseband_hz: f64, span: (f64, f64)) -> f64 {
    (rf_bandwidth / 2.0)
        .min(baseband_hz / 2.0)
        .clamp(span.0, span.1)
}

fn tune_divider(bbpll_hz: f64, bandwidth: f64, margin: f64) -> u16 {
    let clock = margin * bandwidth * 2.0 * PI / LN_2;
    ((bbpll_hz / clock).ceil() as u16).min(TUNE_DIVIDER_MAX)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RxBaseband {
    pub(super) bandwidth: f64,
    pub(super) tune_divider: u16,
    pub(super) mhz: u8,
    pub(super) khz_steps: u8,
}

pub(super) fn rx_baseband(rf_bandwidth: f64, baseband_hz: f64, bbpll_hz: f64) -> RxBaseband {
    let bandwidth = half_band(rf_bandwidth, baseband_hz, RX_FILTER_SPAN);
    let mhz = bandwidth / 1e6;
    let fraction = (mhz - mhz.floor()) * 1000.0 / KHZ_STEP;
    RxBaseband {
        bandwidth,
        tune_divider: tune_divider(bbpll_hz, bandwidth, RX_FILTER_MARGIN),
        mhz: byte(mhz.floor()),
        khz_steps: byte((fraction + 0.5).floor()).min(127),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TxBaseband {
    pub(super) bandwidth: f64,
    pub(super) tune_divider: u16,
}

pub(super) fn tx_baseband(rf_bandwidth: f64, baseband_hz: f64, bbpll_hz: f64) -> TxBaseband {
    let bandwidth = half_band(rf_bandwidth, baseband_hz, TX_FILTER_SPAN);
    TxBaseband {
        bandwidth,
        tune_divider: tune_divider(bbpll_hz, bandwidth, TX_FILTER_MARGIN),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TxSecondary {
    pub(super) bandwidth_code: u8,
    pub(super) resistor_code: u8,
    pub(super) capacitor: u8,
}

pub(super) fn tx_secondary(rf_bandwidth: f64, baseband_hz: f64) -> (f64, TxSecondary) {
    let bandwidth = half_band(rf_bandwidth, baseband_hz, TX_SECONDARY_SPAN);
    let mhz = bandwidth / 1e6;
    let corner = TX_SECONDARY_MARGIN * mhz * 2.0 * PI;
    let mut resistor = 100.0;
    let mut capacitor = i32::MAX;
    for _ in 0..4 {
        capacitor = (0.5 + 1e6 / (corner * resistor)).floor() as i32 - 12;
        if capacitor <= 63 {
            break;
        }
        resistor *= 2.0;
    }
    let bandwidth_code = match mhz * 2.0 {
        wide if wide <= 9.0 => 0x59,
        wide if wide <= 24.0 => 0x56,
        _ => 0x57,
    };
    let resistor_code = match resistor as u32 {
        200 => 0x04,
        400 => 0x03,
        800 => 0x01,
        _ => 0x0c,
    };
    (
        bandwidth,
        TxSecondary {
            bandwidth_code,
            resistor_code,
            capacitor: capacitor.clamp(0, 63) as u8,
        },
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FilterTrim {
    pub(super) c3_msb: u8,
    pub(super) c3_lsb: u8,
    pub(super) r2346: u8,
}

impl FilterTrim {
    fn capacitance_ff(self) -> f64 {
        f64::from(self.c3_msb) * 160.0 + f64::from(self.c3_lsb) * 10.0 + 140.0
    }

    fn resistance(self) -> f64 {
        18300.0 * f64::from(self.r2346 & 0x07)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tia {
    pub(super) bandwidth_code: u8,
    pub(super) c1: u8,
    pub(super) c1_msb: u8,
    pub(super) c2: u8,
    pub(super) c2_msb: u8,
}

pub(super) fn rx_tia(rf_bandwidth: f64, baseband_hz: f64, trim: FilterTrim) -> (f64, Tia) {
    let bandwidth = half_band(rf_bandwidth, baseband_hz, TIA_SPAN);
    let mhz = (bandwidth / 1e6).ceil();
    let tia_ff = trim.capacitance_ff() * trim.resistance() * 0.56 / 3500.0;
    let bandwidth_code = if mhz <= 3.0 {
        0xe0
    } else if mhz <= 10.0 {
        0x60
    } else {
        0x20
    };
    let tia = if tia_ff > 2920.0 {
        let msb = byte((0.5 + (tia_ff - 400.0) / 320.0).floor()).min(127);
        Tia {
            bandwidth_code,
            c1: 0x40,
            c1_msb: msb,
            c2: 0x40,
            c2_msb: msb,
        }
    } else {
        let lsb = byte((0.5 + (tia_ff - 400.0) / 40.0).floor() + 64.0);
        Tia {
            bandwidth_code,
            c1: lsb,
            c1_msb: 0,
            c2: lsb,
            c2_msb: 0,
        }
    };
    (bandwidth, tia)
}

pub(super) fn adc_config(
    bbpll_hz: f64,
    tune_divider: u16,
    adc_hz: f64,
    trim: FilterTrim,
) -> [u8; 40] {
    let bandwidth_mhz = ((bbpll_hz / 1e6) / f64::from(tune_divider.max(1)) * LN_2
        / (RX_FILTER_MARGIN * 2.0 * PI))
        .clamp(ADC_SPAN_MHZ.0, ADC_SPAN_MHZ.1);
    let wide = if bandwidth_mhz < 18.0 {
        1.0
    } else {
        1.0 + 0.01 * (bandwidth_mhz - 18.0)
    };
    let rc = 1.0
        / (RX_FILTER_MARGIN
            * 2.0
            * PI
            * trim.resistance()
            * trim.capacitance_ff()
            * 1e-15
            * bandwidth_mhz
            * 1e6
            * wide);
    let scale = (1.0 / rc).sqrt();
    let fs = adc_hz / 1e6;
    let snr = if adc_hz < 80e6 { 1.0 } else { 1.584_893_192 };
    let max_snr = 4.0;
    let reach = (max_snr * fs / 640.0).sqrt().min(1.0);
    let slow = 640.0 / fs;
    let droop = 0.98 + 0.02 * (slow / max_snr).max(1.0);
    let mut data = [0u8; 40];
    data[3] = 0x24;
    data[4] = 0x24;
    data[7] = byte((-0.5 + 80.0 * snr * scale * reach).floor()).min(124);
    data[8] = byte((0.5 + 20.0 * slow * (f64::from(data[7]) / 80.0) / (scale * scale)).floor());
    data[10] = byte((-0.5 + 77.0 * scale * reach).floor()).min(127);
    data[9] = byte((0.8 * f64::from(data[10])).floor()).min(127);
    data[11] = byte((0.5 + 20.0 * slow * (f64::from(data[10]) / 77.0) / (scale * scale)).floor());
    data[12] = byte((-0.5 + 80.0 * scale * reach).floor()).min(127);
    data[13] = byte((-1.5 + 20.0 * slow * (f64::from(data[12]) / 80.0) / (scale * scale)).floor());
    data[14] = 21u8.saturating_mul(byte((0.1 * slow).floor()));
    for (first, source) in [(15, 7), (18, 10), (21, 12)] {
        let factor = if source == 7 { 1.025 } else { 0.975 };
        data[first] = byte(factor * f64::from(data[source])).min(127);
        data[first + 1] = byte((f64::from(data[first]) * droop).floor()).min(127);
        data[first + 2] = data[first];
    }
    data[24] = 0x2e;
    let ratio = fs / 640.0;
    for first in [25, 28, 31] {
        data[first] = byte((128.0 + (63.0 * ratio).min(63.0)).floor());
        data[first + 1] = byte((63.0 * ratio * (0.92 + 0.08 * slow)).min(63.0).floor());
    }
    data[27] = byte((32.0 * ratio.sqrt()).min(63.0).floor());
    data[30] = data[27];
    data[33] = byte((63.0 * ratio.sqrt()).min(63.0).floor());
    data[34] = byte((64.0 * ratio.sqrt()).floor()).min(127);
    data[35] = 0x40;
    data[36] = 0x40;
    data[37] = 0x2c;
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIM: FilterTrim = FilterTrim {
        c3_msb: 0x13,
        c3_lsb: 0x40,
        r2346: 0x02,
    };

    #[test]
    fn the_receive_filter_is_set_to_half_the_rf_bandwidth() {
        let filter = rx_baseband(10e6, 30.72e6, 983.04e6);
        assert_eq!(filter.bandwidth, 5e6);
        assert_eq!(filter.mhz, 5);
        assert_eq!(filter.khz_steps, 0);
        assert_eq!(filter.tune_divider, 16);
        let fine = rx_baseband(3.5e6, 30.72e6, 983.04e6);
        assert_eq!(fine.mhz, 1);
        assert_eq!(fine.khz_steps, 96);
    }

    #[test]
    fn a_filter_is_never_wider_than_the_converter_rate_allows() {
        assert_eq!(rx_baseband(56e6, 10e6, 983.04e6).bandwidth, 5e6);
        assert_eq!(tx_baseband(56e6, 61.44e6, 983.04e6).bandwidth, 20e6);
        assert_eq!(rx_baseband(1e3, 10e6, 983.04e6).bandwidth, 0.143e6);
    }

    #[test]
    fn a_narrow_secondary_filter_needs_a_bigger_resistor() {
        let (_, wide) = tx_secondary(40e6, 61.44e6);
        assert_eq!(wide.resistor_code, 0x0c);
        assert_eq!(wide.bandwidth_code, 0x57);
        let (bandwidth, narrow) = tx_secondary(1e6, 61.44e6);
        assert_eq!(bandwidth, 0.54e6);
        assert_eq!(narrow.bandwidth_code, 0x59);
        assert_ne!(narrow.resistor_code, 0x0c);
        assert!(narrow.capacitor <= 63);
    }

    #[test]
    fn the_tia_follows_the_trim_the_baseband_calibration_left() {
        let (_, tia) = rx_tia(20e6, 61.44e6, TRIM);
        assert_eq!(tia.bandwidth_code, 0x60);
        assert!(tia.c1 >= 0x40);
        assert_eq!(tia.c1, tia.c2);
    }

    #[test]
    fn the_adc_is_configured_within_its_register_limits() {
        let data = adc_config(983.04e6, 16, 245.76e6, TRIM);
        assert_eq!(data[3], 0x24);
        assert!(data[7] <= 124);
        assert!(data[10] <= 127 && data[12] <= 127);
        assert_eq!(data[17], data[15]);
        assert!(data[25] >= 128);
        assert_eq!(data[37], 0x2c);
    }

    #[test]
    fn an_untrimmed_filter_still_produces_register_values() {
        let data = adc_config(983.04e6, 0, 245.76e6, FilterTrim::default());
        assert_eq!(data[35], 0x40);
    }
}
