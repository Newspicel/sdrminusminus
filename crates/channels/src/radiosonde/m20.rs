use sdrmm_wire::{RadiosondeFrame, SondeType};

use super::{
    fields::{
        GPS_UTC_LEAP_SECONDS, Geodetic, Motion, empty_frame, finite_f32, gps_time, plausible,
        set_motion, set_position, steinhart_hart,
    },
    m10::{self, be_i16, be_i32, be_u24, gps_week, le_u16, thermistor_ohm},
    meteomodem::update_checksum,
};

pub(crate) const BAUD: f64 = 9_600.0;
pub(crate) const LEN_BYTE: u8 = 0x45;
pub(crate) const TYPE: u8 = 0x20;

pub(crate) const POS_HUMIDITY: usize = 0x02;
pub(crate) const POS_THERMISTOR: usize = 0x04;
pub(crate) const POS_HUMIDITY_THERMISTOR: usize = 0x06;
pub(crate) const POS_ALT: usize = 0x08;
pub(crate) const POS_VEL_EAST: usize = 0x0B;
pub(crate) const POS_VEL_NORTH: usize = 0x0D;
pub(crate) const POS_TOW: usize = 0x0F;
pub(crate) const POS_SERIAL: usize = 0x12;
pub(crate) const POS_COUNTER: usize = 0x15;
pub(crate) const POS_BLOCK_CHECK: usize = 0x16;
pub(crate) const POS_VEL_UP: usize = 0x18;
pub(crate) const POS_WEEK: usize = 0x1A;
pub(crate) const POS_LAT: usize = 0x1C;
pub(crate) const POS_LON: usize = 0x20;
pub(crate) const POS_BATTERY: usize = 0x26;
pub(crate) const POS_HUMIDITY_CAL: usize = 0x2F;
pub(crate) const POS_FIRMWARE: usize = 0x43;
pub(crate) const BLOCK_CHECK_LEN: u8 = 0x16;
pub(crate) const FIRMWARE_WITHOUT_BLOCK_CHECK: u8 = 0x07;

pub(crate) const HUMIDITY_SERIES_OHM: f64 = 22.1e3;
pub(crate) const HUMIDITY_R25_OHM: f64 = 2.2e3;
pub(crate) const HUMIDITY_BETA: f64 = 3_650.0;
pub(crate) const MAX_HUMIDITY_COUNT: u16 = 48_000;
pub(crate) const BATTERY_SCALE: f64 = 3.3 / 255.0;

const RANGE_SPAN: u16 = 4_096;

pub(crate) fn block_check(frame: &[u8]) -> u16 {
    let start = update_checksum(0, BLOCK_CHECK_LEN);
    frame[2..POS_BLOCK_CHECK]
        .iter()
        .fold(start, |sum, &byte| update_checksum(sum, byte))
}

pub(crate) fn serial(frame: &[u8]) -> String {
    let raw = u32::from_le_bytes([
        frame[POS_SERIAL],
        frame[POS_SERIAL + 1],
        frame[POS_SERIAL + 2],
        0,
    ]);
    let year_month = raw & 0x7F;
    format!(
        "{}{:02}-{}-{}{:04}",
        year_month / 12,
        year_month % 12 + 1,
        ((raw >> 7) & 0x7) + 1,
        (raw >> 23) & 0x1,
        (raw >> 10) & 0x1FFF
    )
}

fn temperature(frame: &[u8]) -> Option<f64> {
    let adc = le_u16(frame, POS_THERMISTOR);
    let range = (adc / RANGE_SPAN).min(2);
    let ohm = thermistor_ohm(range as u8, f64::from(adc - range * RANGE_SPAN))?;
    let celsius = steinhart_hart(m10::THERMISTOR, ohm)?;
    (-120.0..=60.0).contains(&celsius).then_some(celsius)
}

pub(crate) fn humidity_sensor_celsius(adc: u16) -> Option<f64> {
    let adc = f64::from(adc);
    if adc <= 0.0 || adc >= m10::ADC_FULL_SCALE {
        return None;
    }
    let ohm = HUMIDITY_SERIES_OHM * adc / (m10::ADC_FULL_SCALE - adc);
    let kelvin = 1.0 / (1.0 / 298.15 + (ohm / HUMIDITY_R25_OHM).ln() / HUMIDITY_BETA);
    Some(kelvin - 273.15)
}

pub(crate) fn humidity_from_counts(
    count: u16,
    calibration: u16,
    sensor_celsius: f64,
) -> Option<f64> {
    if count >= MAX_HUMIDITY_COUNT {
        return None;
    }
    let scale = 6.4e8 / (f64::from(calibration) + 80_000.0);
    let x = (f64::from(count) + 80_000.0) * scale * (1.0 - 5.8e-4 * (sensor_celsius - 25.0));
    let y = 4.16e9 / x;
    let humidity = 10.087 * y * y * y - 211.62 * y * y + 1_388.2 * y - 2_797.0;
    (humidity > -20.0 && humidity < 120.0).then(|| humidity.clamp(0.0, 100.0))
}

fn humidity(frame: &[u8]) -> Option<f64> {
    let sensor = humidity_sensor_celsius(le_u16(frame, POS_HUMIDITY_THERMISTOR))?;
    humidity_from_counts(
        le_u16(frame, POS_HUMIDITY),
        le_u16(frame, POS_HUMIDITY_CAL),
        sensor,
    )
}

fn gps(frame: &[u8], out: &mut RadiosondeFrame) {
    let week = gps_week(u16::from_be_bytes([frame[POS_WEEK], frame[POS_WEEK + 1]]));
    let tow = i64::from(be_u24(frame, POS_TOW));
    out.time = week.and_then(|week| gps_time(week, tow, GPS_UTC_LEAP_SECONDS));
    let position = Geodetic {
        lat: f64::from(be_i32(frame, POS_LAT)) / 1e6,
        lon: f64::from(be_i32(frame, POS_LON)) / 1e6,
        alt: f64::from(be_u24(frame, POS_ALT)) / 100.0,
    };
    if !plausible(position) {
        return;
    }
    set_position(out, position);
    let velocity = |at| f64::from(be_i16(frame, at)) / 100.0;
    set_motion(
        out,
        Motion::from_enu(
            velocity(POS_VEL_EAST),
            velocity(POS_VEL_NORTH),
            velocity(POS_VEL_UP),
        ),
    );
}

pub(crate) fn decode(frame: &[u8]) -> Option<RadiosondeFrame> {
    if frame.len() <= POS_FIRMWARE {
        return None;
    }
    if frame[POS_FIRMWARE] < FIRMWARE_WITHOUT_BLOCK_CHECK
        && block_check(frame)
            != u16::from_be_bytes([frame[POS_BLOCK_CHECK], frame[POS_BLOCK_CHECK + 1]])
    {
        return None;
    }
    let mut out = empty_frame(SondeType::M20, serial(frame));
    out.frame = Some(u32::from(frame[POS_COUNTER]));
    gps(frame, &mut out);
    out.temperature_c = temperature(frame).and_then(finite_f32);
    out.humidity_pct = humidity(frame).and_then(finite_f32);
    out.battery_v = finite_f32(f64::from(frame[POS_BATTERY]) * BATTERY_SCALE);
    Some(out)
}
