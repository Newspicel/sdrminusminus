use sdrmm_device::DeviceError;

use crate::ad9361::{MAX_RATE, MIN_RATE};

pub(crate) const MAX_DECIMATION: u32 = 256;
pub(crate) const MIN_SAMPLE_RATE: f64 = 100e3;
pub(crate) const LINK_SAMPLES_PER_SECOND: f64 = 25e6;
const CORDIC_GAIN: f64 = 1.648;
const SCALE_ONE: f64 = 65536.0;
const FULL_SCALE: f64 = 32767.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plan {
    pub(crate) tick: f64,
    pub(crate) decimation: u32,
}

pub(crate) fn max_sample_rate(lanes: usize) -> f64 {
    let lanes = lanes.max(1) as f64;
    (MAX_RATE / lanes).min(LINK_SAMPLES_PER_SECOND / lanes)
}

pub(crate) fn plan(rate: f64, lanes: usize) -> Result<Plan, DeviceError> {
    let converter = MAX_RATE / lanes.max(1) as f64;
    if !rate.is_finite() || rate < MIN_SAMPLE_RATE || rate > max_sample_rate(lanes) + 1.0 {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate} Hz: this radio streams {MIN_SAMPLE_RATE} to {} Hz on {lanes} lanes",
            max_sample_rate(lanes)
        )));
    }
    let mut decimation = 1;
    while decimation < MAX_DECIMATION && rate * f64::from(decimation * 2) <= converter {
        decimation *= 2;
    }
    let tick = rate * f64::from(decimation);
    if tick < MIN_RATE {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate} Hz is below what the converter reaches"
        )));
    }
    Ok(Plan { tick, decimation })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stage {
    pub(crate) word: u32,
    pub(crate) scale: u32,
    pub(crate) host_scale: f32,
}

struct Split {
    first: bool,
    second: bool,
    cic: u32,
}

fn split(factor: u32) -> Split {
    let mut cic = factor.max(1);
    let first = cic.is_multiple_of(2);
    if first {
        cic /= 2;
    }
    let second = cic.is_multiple_of(2);
    if second {
        cic /= 2;
    }
    Split { first, second, cic }
}

fn fixed_point(cic: u32, order: i32) -> (u32, f64) {
    let gain = f64::from(cic).powi(order);
    let adjustment = 2f64.powf(gain.log2().ceil()) / (CORDIC_GAIN * gain);
    let target = SCALE_ONE * adjustment;
    let actual = target.round();
    (actual as u32, target / actual)
}

pub(crate) fn decimator(factor: u32) -> Stage {
    let split = split(factor);
    let (scale, correction) = fixed_point(split.cic, 4);
    Stage {
        word: u32::from(split.first) << 9 | u32::from(split.second) << 8 | split.cic & 0xff,
        scale,
        host_scale: (correction / FULL_SCALE) as f32,
    }
}

pub(crate) fn interpolator(factor: u32) -> Stage {
    let split = split(factor);
    let (scale, correction) = fixed_point(split.cic, 3);
    Stage {
        word: u32::from(split.second) << 9 | u32::from(split.first) << 8 | split.cic & 0xff,
        scale,
        host_scale: (correction * FULL_SCALE) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_converter_runs_a_power_of_two_above_the_stream() {
        let plan = plan(2.048e6, 1).expect("plan");
        assert_eq!(plan.decimation, 16);
        assert!((plan.tick - 32.768e6).abs() < 1.0);
        let wide = plan_or(20e6, 1);
        assert_eq!(wide.decimation, 2);
        let two = plan_or(10e6, 2);
        assert_eq!(two.decimation, 2);
        assert!(two.tick <= MAX_RATE / 2.0);
        assert_eq!(plan_or(100e3, 1).decimation, MAX_DECIMATION);
    }

    fn plan_or(rate: f64, lanes: usize) -> Plan {
        plan(rate, lanes).expect("plan")
    }

    #[test]
    fn a_rate_the_link_cannot_carry_is_refused() {
        assert!(plan(30e6, 1).is_err());
        assert!(plan(15e6, 2).is_err());
        assert!(plan(50e3, 1).is_err());
        assert!(plan(f64::NAN, 1).is_err());
    }

    #[test]
    fn halfbands_take_the_even_part_of_the_decimation() {
        let stage = decimator(16);
        assert_eq!(stage.word, 1 << 9 | 1 << 8 | 4);
        assert_eq!(decimator(1).word, 1);
        assert_eq!(decimator(2).word, 1 << 9 | 1);
        assert_eq!(interpolator(2).word, 1 << 8 | 1);
    }

    #[test]
    fn a_bare_stage_only_removes_the_cordic_gain() {
        let stage = decimator(4);
        assert_eq!(stage.scale, 39767);
        assert!((stage.host_scale * 32767.0 - 1.0).abs() < 1e-4);
        let tx = interpolator(4);
        assert_eq!(tx.scale, 39767);
        assert!((tx.host_scale / 32767.0 - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_cic_gain_is_normalised_back_to_unity() {
        let stage = decimator(64);
        let cic = 16f64;
        let gain = cic.powi(4) * CORDIC_GAIN;
        let net = gain * f64::from(stage.scale) / SCALE_ONE / 2f64.powf(cic.powi(4).log2().ceil());
        assert!((net - 1.0).abs() < 1e-3, "{net}");
    }
}
