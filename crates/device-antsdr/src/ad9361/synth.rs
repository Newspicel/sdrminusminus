use std::ops::RangeInclusive;

pub(super) const BASEBAND_MODULUS: u32 = 2_088_960;
pub(super) const RF_MODULUS: u32 = 8_388_593;
const BASEBAND_VCO: RangeInclusive<f64> = 672e6..=1430e6;
const RF_VCO: RangeInclusive<f64> = 6e9..=12e9;
const CHARGE_PUMP_BASELINE_A: f64 = 150e-6;
const CHARGE_PUMP_BASELINE_HZ: f64 = 1280e6;
const CHARGE_PUMP_STEP_A: f64 = 25e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pll {
    pub(super) divider_code: u8,
    pub(super) integer: u32,
    pub(super) fraction: u32,
    pub(super) vco_hz: f64,
    pub(super) output_hz: f64,
}

pub(super) fn baseband(rate_hz: f64, reference_hz: f64) -> Option<Pll> {
    (1..=6u8).find_map(|code| {
        let divider = f64::from(1u32 << code);
        let wanted = rate_hz * divider;
        BASEBAND_VCO.contains(&wanted).then(|| {
            let (integer, fraction) = split(wanted / reference_hz, BASEBAND_MODULUS, f64::round);
            let vco_hz = reference_hz * ratio(integer, fraction, BASEBAND_MODULUS);
            Pll {
                divider_code: code,
                integer,
                fraction,
                vco_hz,
                output_hz: vco_hz / divider,
            }
        })
    })
}

pub(super) fn rf(lo_hz: f64, reference_hz: f64) -> Option<Pll> {
    (0..=6u8).find_map(|code| {
        let divider = f64::from(2u32 << code);
        let wanted = lo_hz * divider;
        RF_VCO.contains(&wanted).then(|| {
            let (integer, fraction) = split(wanted / reference_hz, RF_MODULUS, f64::floor);
            let vco_hz = reference_hz * ratio(integer, fraction, RF_MODULUS);
            Pll {
                divider_code: code,
                integer,
                fraction,
                vco_hz,
                output_hz: vco_hz / divider,
            }
        })
    })
}

fn split(multiple: f64, modulus: u32, round: fn(f64) -> f64) -> (u32, u32) {
    let mut integer = multiple.floor() as u32;
    let mut fraction = round((multiple - f64::from(integer)) * f64::from(modulus)) as u32;
    if fraction >= modulus {
        integer += 1;
        fraction -= modulus;
    }
    (integer, fraction)
}

fn ratio(integer: u32, fraction: u32, modulus: u32) -> f64 {
    f64::from(integer) + f64::from(fraction) / f64::from(modulus)
}

pub(super) fn baseband_charge_pump(vco_hz: f64) -> u8 {
    let current = CHARGE_PUMP_BASELINE_A * vco_hz / CHARGE_PUMP_BASELINE_HZ;
    ((current / CHARGE_PUMP_STEP_A) as i32 - 1).clamp(0, 0x3f) as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VcoSettings {
    pub(super) output_level: u8,
    pub(super) varactor: u8,
    pub(super) bias_ref: u8,
    pub(super) bias_tcf: u8,
    pub(super) cal_offset: u8,
    pub(super) varactor_ref: u8,
    pub(super) charge_pump: u8,
    pub(super) loop_c1: u8,
    pub(super) loop_c2: u8,
    pub(super) loop_r1: u8,
    pub(super) loop_c3: u8,
    pub(super) loop_r3: u8,
}

const VCO_BAND_MHZ: [f64; 53] = [
    12605.0, 12245.0, 11906.0, 11588.0, 11288.0, 11007.0, 10742.0, 10492.0, 10258.0, 10036.0,
    9827.8, 9631.1, 9445.3, 9269.8, 9103.6, 8946.3, 8797.0, 8655.3, 8520.6, 8392.3, 8269.9, 8153.1,
    8041.4, 7934.4, 7831.8, 7733.2, 7638.4, 7547.1, 7459.0, 7374.0, 7291.9, 7212.4, 7135.5, 7061.0,
    6988.7, 6918.6, 6850.6, 6784.6, 6720.5, 6658.2, 6597.8, 6539.2, 6482.3, 6427.0, 6373.4, 6321.4,
    6270.9, 6222.0, 6174.5, 6128.4, 6083.6, 6040.1, 5997.7,
];

const CHARGE_PUMP: [u8; 53] = [
    8, 9, 10, 11, 11, 12, 13, 13, 14, 15, 15, 16, 17, 18, 18, 19, 14, 14, 15, 15, 16, 16, 17, 17,
    18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26, 27, 27, 18, 18, 18, 19,
    19, 19, 19, 20, 20,
];

struct VcoGroup {
    first: usize,
    varactor: u8,
    bias_ref: u8,
    bias_tcf: u8,
    cal_offset: u8,
    varactor_ref: u8,
}

const fn group(first: usize, fields: [u8; 5]) -> VcoGroup {
    VcoGroup {
        first,
        varactor: fields[0],
        bias_ref: fields[1],
        bias_tcf: fields[2],
        cal_offset: fields[3],
        varactor_ref: fields[4],
    }
}

const VCO_GROUPS: [VcoGroup; 8] = [
    group(0, [0, 4, 0, 15, 8]),
    group(5, [0, 4, 0, 14, 8]),
    group(7, [0, 5, 1, 14, 9]),
    group(11, [0, 5, 1, 13, 9]),
    group(16, [1, 6, 1, 15, 11]),
    group(29, [1, 7, 2, 15, 12]),
    group(32, [1, 7, 2, 15, 14]),
    group(44, [3, 7, 3, 15, 12]),
];

pub(super) fn vco_settings(vco_hz: f64) -> VcoSettings {
    let index = VCO_BAND_MHZ
        .iter()
        .position(|band| vco_hz > band * 1e6)
        .unwrap_or(VCO_BAND_MHZ.len() - 1);
    let group = VCO_GROUPS
        .iter()
        .rev()
        .find(|group| group.first <= index)
        .unwrap_or(&VCO_GROUPS[0]);
    VcoSettings {
        output_level: 10,
        varactor: group.varactor,
        bias_ref: group.bias_ref,
        bias_tcf: group.bias_tcf,
        cal_offset: group.cal_offset,
        varactor_ref: group.varactor_ref,
        charge_pump: CHARGE_PUMP[index],
        loop_c1: 4,
        loop_c2: 13,
        loop_r1: 13,
        loop_c3: 15,
        loop_r3: 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: f64 = 40e6;

    #[test]
    fn the_converter_clock_divides_a_vco_inside_its_range() {
        let pll = baseband(16e6 * 16.0, REFERENCE).expect("reachable");
        assert_eq!(pll.divider_code, 2);
        assert!(BASEBAND_VCO.contains(&pll.vco_hz));
        assert!((pll.output_hz - 256e6).abs() < 1.0);
        assert_eq!(pll.integer, 25);
        assert_eq!(pll.fraction, 1_253_376);
    }

    #[test]
    fn a_rate_no_divider_reaches_is_refused() {
        assert!(baseband(1e6, REFERENCE).is_none());
        assert!(baseband(800e6, REFERENCE).is_none());
    }

    #[test]
    fn a_local_oscillator_lands_within_a_hertz_of_the_request() {
        for lo in [70e6, 100e6, 433.92e6, 1575.42e6, 2.4e9, 5.8e9, 6e9] {
            let pll = rf(lo, 2.0 * REFERENCE).expect("reachable");
            assert!(RF_VCO.contains(&pll.vco_hz), "{lo}");
            assert!(
                (pll.output_hz - lo).abs() < 1.0,
                "{lo} -> {}",
                pll.output_hz
            );
        }
        assert_eq!(rf(100e6, 80e6).expect("100 MHz").divider_code, 5);
        assert!(rf(40e6, 80e6).is_none());
    }

    #[test]
    fn a_fraction_that_rounds_up_to_the_modulus_carries_into_the_integer() {
        assert_eq!(split(3.999_999_999_99, 1000, f64::round), (4, 0));
    }

    #[test]
    fn the_vco_table_follows_the_frequency_down() {
        let top = vco_settings(12.7e9);
        assert_eq!(top.charge_pump, 8);
        assert_eq!(top.varactor_ref, 8);
        let bottom = vco_settings(5.9e9);
        assert_eq!(bottom.charge_pump, 20);
        assert_eq!(bottom.varactor, 3);
        assert_eq!(vco_settings(8.0e9).varactor_ref, 11);
    }

    #[test]
    fn the_baseband_charge_pump_scales_with_the_vco() {
        assert_eq!(baseband_charge_pump(1300e6), 5);
        assert_eq!(baseband_charge_pump(672e6), 2);
    }
}
