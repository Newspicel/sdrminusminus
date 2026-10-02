use sdrmm_wire::{RadiosondeFrame, SondeType};

use super::fields::{
    Geodetic, Motion, civil_time, empty_frame, finite_f32, gps_time, plausible, set_motion,
    set_position, steinhart_hart,
};

pub(crate) const BAUD: f64 = 9_616.0;
pub(crate) const LEN_BYTE: u8 = 0x64;
pub(crate) const TYPE_TRIMBLE: u8 = 0x9F;
pub(crate) const TYPE_GTOP: u8 = 0xAF;

pub(crate) const POS_VEL_EAST: usize = 0x04;
pub(crate) const POS_VEL_NORTH: usize = 0x06;
pub(crate) const POS_VEL_UP: usize = 0x08;
pub(crate) const POS_TOW: usize = 0x0A;
pub(crate) const POS_LAT: usize = 0x0E;
pub(crate) const POS_LON: usize = 0x12;
pub(crate) const POS_ALT: usize = 0x16;
pub(crate) const POS_SATS: usize = 0x1E;
pub(crate) const POS_UTC_OFFSET: usize = 0x1F;
pub(crate) const POS_WEEK: usize = 0x20;
pub(crate) const POS_HUMIDITY_REF: usize = 0x32;
pub(crate) const POS_HUMIDITY_COUNT: usize = 0x35;
pub(crate) const POS_THERMISTOR_RANGE: usize = 0x3E;
pub(crate) const POS_THERMISTOR_ADC: usize = 0x3F;
pub(crate) const POS_BATTERY: usize = 0x45;
pub(crate) const POS_SERIAL: usize = 0x5D;
pub(crate) const POS_COUNTER: usize = 0x62;

pub(crate) const GTOP_LAT: usize = 0x04;
pub(crate) const GTOP_LON: usize = 0x08;
pub(crate) const GTOP_ALT: usize = 0x0C;
pub(crate) const GTOP_VEL: usize = 0x0F;
pub(crate) const GTOP_TIME: usize = 0x15;
pub(crate) const GTOP_DATE: usize = 0x18;

pub(crate) const ANGLE_SCALE: f64 = (1u64 << 32) as f64 / 360.0;
pub(crate) const VELOCITY_SCALE: f64 = 200.0;
pub(crate) const THERMISTOR_ADC_BIAS: u16 = 0xA000;
pub(crate) const ADC_FULL_SCALE: f64 = 4_095.0;
pub(crate) const SERIES_OHM: [f64; 3] = [12.1e3, 36.5e3, 475.0e3];
pub(crate) const PARALLEL_OHM: [f64; 3] = [f64::INFINITY, 330.0e3, 2_000.0e3];
pub(crate) const THERMISTOR: [f64; 4] = [
    1.073_035_16e-3,
    2.412_967_33e-4,
    2.267_441_54e-6,
    6.528_551_81e-8,
];
pub(crate) const BATTERY_SCALE: f64 = 2.709 * 2.5 / 1_023.0;

const WEEK_ROLLOVER_BEFORE: u16 = 1_304;

pub(crate) fn be_i16(frame: &[u8], at: usize) -> i16 {
    i16::from_be_bytes([frame[at], frame[at + 1]])
}

pub(crate) fn be_i32(frame: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]])
}

pub(crate) fn be_u24(frame: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([0, frame[at], frame[at + 1], frame[at + 2]])
}

pub(crate) fn le_u24(frame: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], 0])
}

pub(crate) fn le_u16(frame: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([frame[at], frame[at + 1]])
}

pub(crate) fn gps_week(raw: u16) -> Option<i64> {
    match raw {
        0..WEEK_ROLLOVER_BEFORE => Some(i64::from(raw) + 1_024),
        WEEK_ROLLOVER_BEFORE..=4_000 => Some(i64::from(raw)),
        _ => None,
    }
}

pub(crate) fn serial(frame: &[u8]) -> String {
    let raw = &frame[POS_SERIAL..POS_SERIAL + 5];
    let word = u16::from_le_bytes([raw[3], raw[4]]);
    format!(
        "{:X}{:02}-{:X}-{}{:04}",
        raw[2] >> 4,
        raw[2] & 0xF,
        raw[0] & 0xF,
        (word >> 13) & 0x7,
        word & 0x1FFF
    )
}

pub(crate) fn thermistor_ohm(range: u8, adc: f64) -> Option<f64> {
    let range = usize::from(range);
    if range >= SERIES_OHM.len() || adc <= 0.0 {
        return None;
    }
    let ratio = (ADC_FULL_SCALE - adc) / adc;
    let ohm = SERIES_OHM[range] / (ratio - SERIES_OHM[range] / PARALLEL_OHM[range]);
    (ohm > 0.0).then_some(ohm)
}

fn temperature(frame: &[u8]) -> Option<f64> {
    let adc = le_u16(frame, POS_THERMISTOR_ADC).wrapping_sub(THERMISTOR_ADC_BIAS);
    let ohm = thermistor_ohm(frame[POS_THERMISTOR_RANGE], f64::from(adc))?;
    let celsius = steinhart_hart(THERMISTOR, ohm)?;
    (-120.0..=60.0).contains(&celsius).then_some(celsius)
}

pub(crate) fn humidity_from_ratio(ratio: f64, celsius: f64) -> f64 {
    let mut humidity = (ratio - 0.8955) / 0.002;
    if celsius < 0.0 {
        humidity -= celsius / 5.5;
    }
    if celsius < -30.0 {
        humidity *= 1.0 + (-30.0 - celsius) / 75.0;
    }
    humidity.clamp(0.0, 100.0)
}

fn humidity(frame: &[u8], celsius: f64) -> Option<f64> {
    let reference = f64::from(le_u24(frame, POS_HUMIDITY_REF));
    let count = f64::from(le_u24(frame, POS_HUMIDITY_COUNT));
    (reference > 0.0).then(|| humidity_from_ratio(count / reference, celsius))
}

fn trimble(frame: &[u8], out: &mut RadiosondeFrame) {
    let week = gps_week(u16::from_be_bytes([frame[POS_WEEK], frame[POS_WEEK + 1]]));
    let tow_ms = i64::from(be_i32(frame, POS_TOW));
    let leap = i64::from(frame[POS_UTC_OFFSET]);
    out.time = week.and_then(|week| gps_time(week, tow_ms.div_euclid(1_000), leap));
    out.satellites = Some(frame[POS_SATS]);
    let position = Geodetic {
        lat: f64::from(be_i32(frame, POS_LAT)) / ANGLE_SCALE,
        lon: f64::from(be_i32(frame, POS_LON)) / ANGLE_SCALE,
        alt: f64::from(be_i32(frame, POS_ALT)) / 1_000.0,
    };
    if !plausible(position) {
        return;
    }
    set_position(out, position);
    let velocity = |at| f64::from(be_i16(frame, at)) / VELOCITY_SCALE;
    set_motion(
        out,
        Motion::from_enu(
            velocity(POS_VEL_EAST),
            velocity(POS_VEL_NORTH),
            velocity(POS_VEL_UP),
        ),
    );
}

fn gtop(frame: &[u8], out: &mut RadiosondeFrame) {
    let time = be_u24(frame, GTOP_TIME);
    let date = be_u24(frame, GTOP_DATE);
    out.time = civil_time(
        2_000 + (date % 100) as i16,
        ((date / 100) % 100) as i8,
        (date / 10_000) as i8,
        (time / 10_000) as i8,
        ((time / 100) % 100) as i8,
        (time % 100) as i8,
    );
    let altitude = ((be_u24(frame, GTOP_ALT) << 8) as i32) >> 8;
    let position = Geodetic {
        lat: f64::from(be_i32(frame, GTOP_LAT)) / 1e6,
        lon: f64::from(be_i32(frame, GTOP_LON)) / 1e6,
        alt: f64::from(altitude) / 100.0,
    };
    if !plausible(position) {
        return;
    }
    set_position(out, position);
    let velocity = |k: usize| f64::from(be_i16(frame, GTOP_VEL + 2 * k)) / 100.0;
    set_motion(out, Motion::from_enu(velocity(0), velocity(1), velocity(2)));
}

pub(crate) fn decode(frame: &[u8]) -> Option<RadiosondeFrame> {
    if frame.len() <= POS_COUNTER {
        return None;
    }
    let mut out = empty_frame(SondeType::M10, serial(frame));
    out.frame = Some(u32::from(frame[POS_COUNTER]));
    if frame[1] == TYPE_GTOP {
        gtop(frame, &mut out);
    } else {
        trimble(frame, &mut out);
    }
    let celsius = temperature(frame);
    out.temperature_c = celsius.and_then(finite_f32);
    out.humidity_pct = celsius
        .and_then(|celsius| humidity(frame, celsius))
        .and_then(finite_f32);
    out.battery_v = finite_f32(f64::from(le_u16(frame, POS_BATTERY)) * BATTERY_SCALE);
    Some(out)
}
