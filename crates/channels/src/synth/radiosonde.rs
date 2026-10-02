use std::f64::consts::TAU;

use jiff::Timestamp;
use num_complex::Complex;
use sdrmm_dsp::design_gaussian;
use sdrmm_wire::SondeType;

use super::{fm_modulate, resample};
use crate::radiosonde::{
    INPUT_RATE_HZ, dfm,
    fields::{GPS_EPOCH_UNIX, GPS_UTC_LEAP_SECONDS, SECONDS_PER_WEEK, WGS84_A, WGS84_B},
    imet, m10, m20, meteomodem,
    rs41::{self, ptu},
};

pub const RATE: f64 = INPUT_RATE_HZ;

const METERS_PER_DEGREE: f64 = 111_320.0;
const START_UNIX: i64 = 1_790_858_096;
const LEAD_IN_S: f64 = 0.1;
const RS41_DEVIATION_HZ: f64 = 2_400.0;
const RS41_BT: f64 = 0.5;
const DFM_DEVIATION_HZ: f64 = 2_000.0;
const METEOMODEM_DEVIATION_HZ: f64 = 3_500.0;
const IMET_DEVIATION_HZ: f64 = 3_000.0;
const DFM_FRAMES_PER_CYCLE: usize = 5;
const DFM_CHANNELS: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 0xA];
const DFM_SERIES_OHM: f64 = 20e3;
const RS41_REF_LOW_OHM: f64 = 750.0;
const RS41_REF_HIGH_OHM: f64 = 1_100.0;
const RS41_COUNTS_PER_OHM: f64 = 100.0;
const RS41_TEMPERATURE_POLY: [f32; 3] = [-243.911, 0.187_654, 8.2e-6];
const RS41_HUMIDITY_CAL: f32 = 47.0;
const RS41_PRESSURE_SCALE: f32 = 1_000.0;
const RS41_COUNT_LOW: f64 = 100_000.0;
const RS41_COUNT_HIGH: f64 = 200_000.0;
const M10_SERIAL_RAW: [u8; 5] = [0x02, 0x00, 0x98, 0x7B, 0x20];
const M20_SERIAL_RAW: u32 = (1234 << 10) | (1 << 7) | 14;
const M20_FIRMWARE: u8 = 0x06;

#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    pub sonde: SondeType,
    pub serial: String,
    pub first_frame: u32,
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
    pub east_ms: f64,
    pub north_ms: f64,
    pub up_ms: f64,
    pub temperature_c: f64,
    pub humidity_pct: f64,
    pub pressure_hpa: f64,
    pub battery_v: f64,
    pub satellites: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub frame: u32,
    pub unix: i64,
    pub time: String,
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
}

impl Flight {
    #[must_use]
    pub fn fix(&self, index: u32) -> Fix {
        let k = f64::from(index);
        let unix = START_UNIX + i64::from(index);
        Fix {
            frame: self.first_frame + index,
            unix,
            time: iso(unix),
            lat: self.lat + self.north_ms * k / METERS_PER_DEGREE,
            lon: self.lon + self.east_ms * k / (METERS_PER_DEGREE * self.lat.to_radians().cos()),
            alt: self.alt + self.up_ms * k,
        }
    }

    #[must_use]
    pub fn speed_ms(&self) -> f64 {
        self.east_ms.hypot(self.north_ms)
    }

    #[must_use]
    pub fn heading_deg(&self) -> f64 {
        self.east_ms
            .atan2(self.north_ms)
            .to_degrees()
            .rem_euclid(360.0)
    }
}

fn iso(unix: i64) -> String {
    Timestamp::from_second(unix)
        .map(|stamp| stamp.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

#[must_use]
pub fn flight(sonde: SondeType) -> Flight {
    let base = Flight {
        sonde,
        serial: String::new(),
        first_frame: 0,
        lat: 0.0,
        lon: 0.0,
        alt: 0.0,
        east_ms: 0.0,
        north_ms: 0.0,
        up_ms: 0.0,
        temperature_c: 0.0,
        humidity_pct: 0.0,
        pressure_hpa: 0.0,
        battery_v: 0.0,
        satellites: 0,
    };
    match sonde {
        SondeType::Rs41 => Flight {
            serial: "W3420567".to_owned(),
            first_frame: 3,
            lat: 48.123_456_7,
            lon: 11.543_210_0,
            alt: 5_432.1,
            east_ms: 7.5,
            north_ms: -3.2,
            up_ms: 5.1,
            temperature_c: -12.5,
            humidity_pct: 63.0,
            pressure_hpa: 512.5,
            battery_v: 2.9,
            satellites: 9,
            ..base
        },
        SondeType::Dfm => Flight {
            serial: "23034567".to_owned(),
            first_frame: 0x42,
            lat: 52.520_000_0,
            lon: 13.405_000_0,
            alt: 1_234.56,
            east_ms: -4.0,
            north_ms: 6.0,
            up_ms: 4.8,
            temperature_c: -8.25,
            battery_v: 3.25,
            satellites: 8,
            ..base
        },
        SondeType::M10 => Flight {
            serial: "908-2-10123".to_owned(),
            first_frame: 17,
            lat: 43.604_652,
            lon: 1.444_209,
            alt: 8_765.4,
            east_ms: 12.0,
            north_ms: 5.0,
            up_ms: 4.5,
            temperature_c: -12.3,
            humidity_pct: 41.0,
            battery_v: 6.1,
            satellites: 7,
            ..base
        },
        SondeType::M20 => Flight {
            serial: "103-2-01234".to_owned(),
            first_frame: 200,
            lat: 45.764_043,
            lon: 4.835_659,
            alt: 3_210.98,
            east_ms: -6.5,
            north_ms: -2.0,
            up_ms: 5.5,
            temperature_c: 4.75,
            humidity_pct: 55.0,
            battery_v: 3.1,
            ..base
        },
        SondeType::Imet4 => Flight {
            serial: imet::serial_for(seconds_of_day(START_UNIX) - 1_000),
            first_frame: 1_000,
            lat: 39.739_236,
            lon: -104.990_251,
            alt: 2_345.0,
            temperature_c: -21.37,
            humidity_pct: 34.56,
            pressure_hpa: 812.34,
            battery_v: 5.4,
            satellites: 10,
            ..base
        },
    }
}

fn seconds_of_day(unix: i64) -> i64 {
    unix.rem_euclid(86_400)
}

fn gps_week_tow(unix: i64) -> (i64, i64) {
    let gps = unix - GPS_EPOCH_UNIX + GPS_UTC_LEAP_SECONDS;
    (gps / SECONDS_PER_WEEK, gps % SECONDS_PER_WEEK)
}

fn geodetic_to_ecef(lat: f64, lon: f64, alt: f64) -> [f64; 3] {
    let e2 = 1.0 - (WGS84_B * WGS84_B) / (WGS84_A * WGS84_A);
    let (phi, lam) = (lat.to_radians(), lon.to_radians());
    let n = WGS84_A / (1.0 - e2 * phi.sin().powi(2)).sqrt();
    [
        (n + alt) * phi.cos() * lam.cos(),
        (n + alt) * phi.cos() * lam.sin(),
        (n * (1.0 - e2) + alt) * phi.sin(),
    ]
}

fn enu_to_ecef(lat: f64, lon: f64, enu: [f64; 3]) -> [f64; 3] {
    let (phi, lam) = (lat.to_radians(), lon.to_radians());
    let [e, n, u] = enu;
    [
        -lam.sin() * e - phi.sin() * lam.cos() * n + phi.cos() * lam.cos() * u,
        lam.cos() * e - phi.sin() * lam.sin() * n + phi.cos() * lam.sin() * u,
        phi.cos() * n + phi.sin() * u,
    ]
}

fn thermistor_ohm(coefficients: [f64; 4], celsius: f64) -> f64 {
    let target = 1.0 / (celsius + 273.15);
    let (mut low, mut high) = (0.0f64, 20.0f64);
    for _ in 0..100 {
        let mid = 0.5 * (low + high);
        let value = coefficients[0]
            + coefficients[1] * mid
            + coefficients[2] * mid * mid
            + coefficients[3] * mid * mid * mid;
        if value > target {
            high = mid;
        } else {
            low = mid;
        }
    }
    (0.5 * (low + high)).exp()
}

fn push_f32(bytes: &mut [u8], at: usize, value: f32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn rs41_calibration() -> [u8; ptu::CALIBRATION_LEN] {
    let mut cal = [0u8; ptu::CALIBRATION_LEN];
    push_f32(&mut cal, ptu::REF_RESISTOR_LOW, RS41_REF_LOW_OHM as f32);
    push_f32(&mut cal, ptu::REF_RESISTOR_HIGH, RS41_REF_HIGH_OHM as f32);
    for (k, value) in RS41_TEMPERATURE_POLY.iter().enumerate() {
        push_f32(&mut cal, ptu::TEMPERATURE_POLY + 4 * k, *value);
    }
    push_f32(&mut cal, ptu::TEMPERATURE_CAL, 1.0);
    push_f32(&mut cal, ptu::HUMIDITY_CAL, RS41_HUMIDITY_CAL);
    cal[ptu::SONDE_TYPE..ptu::SONDE_TYPE + 8].copy_from_slice(b"RS41-SGP");
    for (offset, index) in ptu::PRESSURE_COEFFICIENTS {
        let value = match index {
            4 => 1.0,
            ptu::PRESSURE_SCALE_INDEX => RS41_PRESSURE_SCALE,
            _ => 0.0,
        };
        push_f32(&mut cal, offset, value);
    }
    let check = rs41::crc(&cal[2..50 * ptu::SUBFRAME_LEN]);
    cal[..2].copy_from_slice(&check.to_le_bytes());
    cal
}

fn rs41_temperature_ohm(celsius: f64) -> f64 {
    let [p0, p1, p2] = RS41_TEMPERATURE_POLY.map(f64::from);
    (-p1 + (p1 * p1 - 4.0 * p2 * (p0 - celsius)).sqrt()) / (2.0 * p2)
}

fn put_u24(bytes: &mut Vec<u8>, value: f64) {
    let raw = value.round().clamp(0.0, f64::from(0xFF_FFFFu32)) as u32;
    bytes.extend_from_slice(&raw.to_le_bytes()[..3]);
}

fn rs41_ptu(flight: &Flight) -> Vec<u8> {
    let low = RS41_REF_LOW_OHM * RS41_COUNTS_PER_OHM;
    let high = RS41_REF_HIGH_OHM * RS41_COUNTS_PER_OHM;
    let temperature = rs41_temperature_ohm(flight.temperature_c) * RS41_COUNTS_PER_OHM;
    let humidity = ptu::humidity_ratio(
        f64::from(RS41_HUMIDITY_CAL),
        flight.humidity_pct,
        flight.temperature_c,
    );
    let pressure = f64::from(RS41_PRESSURE_SCALE) / flight.pressure_hpa;
    let span = RS41_COUNT_HIGH - RS41_COUNT_LOW;
    let mut payload = Vec::with_capacity(42);
    for value in [
        temperature,
        low,
        high,
        RS41_COUNT_LOW + humidity * span,
        RS41_COUNT_LOW,
        RS41_COUNT_HIGH,
        temperature,
        low,
        high,
        RS41_COUNT_LOW + pressure * span,
        RS41_COUNT_LOW,
        RS41_COUNT_HIGH,
    ] {
        put_u24(&mut payload, value);
    }
    payload.extend_from_slice(&[0, 0]);
    let sensor = (flight.temperature_c * 100.0).round() as i16;
    payload.extend_from_slice(&sensor.to_le_bytes());
    payload.extend_from_slice(&[0, 0]);
    payload
}

fn put_block(frame: &mut [u8], pos: usize, id: u8, payload: &[u8]) -> usize {
    frame[pos] = id;
    frame[pos + 1] = payload.len() as u8;
    frame[pos + 2..pos + 2 + payload.len()].copy_from_slice(payload);
    let end = pos + 2 + payload.len();
    frame[end..end + 2].copy_from_slice(&rs41::crc(payload).to_le_bytes());
    end + 2
}

fn rs41_status(flight: &Flight, fix: &Fix, calibration: &[u8]) -> Vec<u8> {
    let mut status = vec![0u8; 40];
    status[..2].copy_from_slice(&(fix.frame as u16).to_le_bytes());
    status[2..10].copy_from_slice(flight.serial.as_bytes());
    status[10] = (flight.battery_v * 10.0).round() as u8;
    let index = fix.frame as usize % ptu::SUBFRAMES;
    status[rs41::STATUS_CALIBRATION_INDEX] = index as u8;
    let start = index * ptu::SUBFRAME_LEN;
    status[rs41::STATUS_CALIBRATION_INDEX + 1..]
        .copy_from_slice(&calibration[start..start + ptu::SUBFRAME_LEN]);
    status
}

fn rs41_position(flight: &Flight, fix: &Fix) -> Vec<u8> {
    let mut payload = Vec::with_capacity(21);
    for axis in geodetic_to_ecef(fix.lat, fix.lon, fix.alt) {
        payload.extend_from_slice(&((axis * 100.0).round() as i32).to_le_bytes());
    }
    let velocity = enu_to_ecef(
        fix.lat,
        fix.lon,
        [flight.east_ms, flight.north_ms, flight.up_ms],
    );
    for axis in velocity {
        payload.extend_from_slice(&((axis * 100.0).round() as i16).to_le_bytes());
    }
    payload.extend_from_slice(&[flight.satellites, 5, 15]);
    payload
}

fn rs41_parity(frame: &mut [u8; rs41::EXTENDED_LEN], len: usize) {
    let code = rs41::reed_solomon();
    let k = rs41::message_len(len);
    for lane in 0..2 {
        let mut codeword = rs41::gather(frame, len, lane);
        let mut encoded = Vec::with_capacity(k + rs41::PARITY_LEN);
        code.encode(&codeword[..k], &mut encoded);
        codeword[k..k + rs41::PARITY_LEN].copy_from_slice(&encoded[k..]);
        rs41::scatter(frame, len, lane, &codeword);
    }
}

#[must_use]
pub fn rs41_frame(index: u32) -> Vec<u8> {
    let flight = flight(SondeType::Rs41);
    let fix = flight.fix(index);
    let calibration = rs41_calibration();
    let mut frame = [0u8; rs41::EXTENDED_LEN];
    frame[rs41::FRAME_TYPE_POS] = rs41::STANDARD_TYPE;
    let (week, tow) = gps_week_tow(fix.unix);
    let mut info = vec![0u8; 30];
    info[..2].copy_from_slice(&(week as u16).to_le_bytes());
    info[2..6].copy_from_slice(&((tow * 1_000) as u32).to_le_bytes());
    let mut pos = rs41::BLOCKS_POS;
    let status = rs41_status(&flight, &fix, &calibration);
    pos = put_block(&mut frame, pos, rs41::BLOCK_STATUS, &status);
    pos = put_block(&mut frame, pos, rs41::BLOCK_PTU, &rs41_ptu(&flight));
    pos = put_block(&mut frame, pos, rs41::BLOCK_GPS_INFO, &info);
    pos = put_block(&mut frame, pos, rs41::BLOCK_GPS_RAW, &[0u8; 89]);
    pos = put_block(
        &mut frame,
        pos,
        rs41::BLOCK_GPS_POSITION,
        &rs41_position(&flight, &fix),
    );
    put_block(&mut frame, pos, rs41::BLOCK_EMPTY, &[0u8; 17]);
    rs41_parity(&mut frame, rs41::STANDARD_LEN);
    let mut air = frame[..rs41::STANDARD_LEN].to_vec();
    rs41::whiten(&mut air);
    air[..rs41::HEADER.len()].copy_from_slice(&rs41::HEADER);
    air
}

fn float24(value: f64) -> u32 {
    let exponent = (0..=15u32)
        .rev()
        .find(|&e| value * f64::from(1u32 << e) < f64::from(1u32 << 20))
        .unwrap_or(0);
    (exponent << 20) | (value * f64::from(1u32 << exponent)).round() as u32
}

fn dfm_conf(flight: &Flight, channel: u8, serial_visit: usize) -> u32 {
    let ohm = thermistor_ohm(dfm::THERMISTOR, flight.temperature_c);
    let serial: u32 = flight.serial.parse().unwrap_or(0);
    match channel {
        0 => float24(ohm + DFM_SERIES_OHM),
        3 => float24(DFM_SERIES_OHM),
        4 => float24(220e3),
        5 => ((flight.battery_v * 1_000.0).round() as u32) << 4,
        6 => 2_150 << 4,
        0xA => {
            let half = (serial_visit % 2) as u32;
            let word = if half == 0 {
                serial >> 16
            } else {
                serial & 0xFFFF
            };
            (u32::from(dfm::SERIAL_MARK) << 20) | (word << 4) | half
        }
        _ => float24(1_000.0),
    }
}

fn dfm_data(flight: &Flight, cycle: u32, id: u8) -> u64 {
    let fix = flight.fix(cycle);
    let stamp = Timestamp::from_second(fix.unix)
        .map(|s| s.to_zoned(jiff::tz::TimeZone::UTC))
        .ok();
    let put = |value: i64, len: u32, start: u32| -> u64 {
        ((value as u64) & ((1u64 << len) - 1)) << (48 - start - len)
    };
    let payload = match (id, stamp) {
        (0, _) => put(i64::from(dfm::MODE_GPS), 8, 16) | put(i64::from(fix.frame), 8, 24),
        (1, Some(z)) => put(i64::from(z.second()) * 1_000, 16, 32),
        (2, _) => {
            put((fix.lat * 1e7).round() as i64, 32, 0)
                | put((flight.speed_ms() * 100.0).round() as i64, 16, 32)
        }
        (3, _) => {
            put((fix.lon * 1e7).round() as i64, 32, 0)
                | put((flight.heading_deg() * 100.0).round() as i64, 16, 32)
        }
        (4, _) => {
            put((fix.alt * 100.0).round() as i64, 32, 0)
                | put((flight.up_ms * 100.0).round() as i64, 16, 32)
        }
        (8, Some(z)) => {
            put(i64::from(z.year()), 12, 0)
                | put(i64::from(z.month()), 4, 12)
                | put(i64::from(z.day()), 5, 16)
                | put(i64::from(z.hour()), 5, 21)
                | put(i64::from(z.minute()), 6, 26)
                | put(i64::from(flight.satellites), 8, 32)
        }
        _ => 0,
    };
    (payload << 4) | u64::from(id)
}

fn dfm_section(nibbles: &[u8], bits: &mut Vec<bool>) {
    let count = nibbles.len();
    let codes: Vec<[bool; 8]> = nibbles.iter().map(|&n| dfm::hamming_encode(n)).collect();
    let mut section = vec![false; 8 * count];
    for (codeword, code) in codes.iter().enumerate() {
        for (bit, &value) in code.iter().enumerate() {
            section[dfm::interleaved_index(count, codeword, bit)] = value;
        }
    }
    bits.extend(section);
}

fn nibbles(value: u64, count: usize) -> Vec<u8> {
    (0..count)
        .map(|k| ((value >> (4 * (count - 1 - k))) & 0xF) as u8)
        .collect()
}

#[must_use]
pub fn dfm_payload(frame: usize) -> Vec<bool> {
    let flight = flight(SondeType::Dfm);
    let channel = DFM_CHANNELS[frame % DFM_CHANNELS.len()];
    let conf = (u64::from(channel) << 24)
        | u64::from(dfm_conf(&flight, channel, frame / DFM_CHANNELS.len()));
    let mut bits = Vec::with_capacity(dfm::PAYLOAD_BITS);
    dfm_section(&nibbles(conf, dfm::CONF_CODEWORDS), &mut bits);
    for slot in 0..2 {
        let sequence = 2 * (frame % DFM_FRAMES_PER_CYCLE) + slot;
        let cycle = (frame / DFM_FRAMES_PER_CYCLE) as u32;
        let data = dfm_data(&flight, cycle, sequence as u8);
        dfm_section(&nibbles(data, dfm::DAT_CODEWORDS), &mut bits);
    }
    bits
}

fn manchester(bit: bool) -> [bool; 2] {
    [!bit, bit]
}

fn dfm_symbols(cycles: u32) -> Vec<bool> {
    let mut symbols = alternating((LEAD_IN_S * dfm::BAUD) as usize);
    for frame in 0..cycles as usize * DFM_FRAMES_PER_CYCLE {
        symbols.extend(dfm::RAW_HEADER.iter().map(|&c| c == b'1'));
        for bit in dfm_payload(frame) {
            symbols.extend(manchester(bit));
        }
    }
    symbols
}

fn put_be(frame: &mut [u8], at: usize, bytes: &[u8]) {
    frame[at..at + bytes.len()].copy_from_slice(bytes);
}

fn finish_meteomodem(frame: &mut [u8]) {
    let len = usize::from(frame[0]);
    let sum = meteomodem::checksum(&frame[..len - 1]);
    put_be(frame, len - 1, &sum.to_be_bytes());
}

fn adc_for(ohm: f64, series: f64) -> f64 {
    m10::ADC_FULL_SCALE / (1.0 + series / ohm)
}

#[must_use]
pub fn m10_frame(index: u32) -> Vec<u8> {
    let flight = flight(SondeType::M10);
    let fix = flight.fix(index);
    let mut frame = vec![0u8; usize::from(m10::LEN_BYTE) + 1];
    frame[0] = m10::LEN_BYTE;
    frame[1] = m10::TYPE_TRIMBLE;
    frame[2] = 0x20;
    for (at, value) in [
        (m10::POS_VEL_EAST, flight.east_ms),
        (m10::POS_VEL_NORTH, flight.north_ms),
        (m10::POS_VEL_UP, flight.up_ms),
    ] {
        put_be(
            &mut frame,
            at,
            &((value * m10::VELOCITY_SCALE).round() as i16).to_be_bytes(),
        );
    }
    let (week, tow) = gps_week_tow(fix.unix);
    put_be(
        &mut frame,
        m10::POS_TOW,
        &((tow * 1_000) as u32).to_be_bytes(),
    );
    put_be(
        &mut frame,
        m10::POS_LAT,
        &((fix.lat * m10::ANGLE_SCALE).round() as i32).to_be_bytes(),
    );
    put_be(
        &mut frame,
        m10::POS_LON,
        &((fix.lon * m10::ANGLE_SCALE).round() as i32).to_be_bytes(),
    );
    put_be(
        &mut frame,
        m10::POS_ALT,
        &((fix.alt * 1_000.0).round() as i32).to_be_bytes(),
    );
    frame[m10::POS_SATS] = flight.satellites;
    frame[m10::POS_UTC_OFFSET] = GPS_UTC_LEAP_SECONDS as u8;
    put_be(&mut frame, m10::POS_WEEK, &(week as u16).to_be_bytes());
    let reference = 1_000_000.0;
    let ratio = m10_humidity_ratio(flight.humidity_pct, flight.temperature_c);
    let mut counts = Vec::new();
    put_u24(&mut counts, reference);
    put_u24(&mut counts, ratio * reference);
    put_be(&mut frame, m10::POS_HUMIDITY_REF, &counts);
    let ohm = thermistor_ohm(m10::THERMISTOR, flight.temperature_c);
    let adc = adc_for(ohm, m10::SERIES_OHM[0]).round() as u16 + m10::THERMISTOR_ADC_BIAS;
    put_be(&mut frame, m10::POS_THERMISTOR_ADC, &adc.to_le_bytes());
    let battery = (flight.battery_v / m10::BATTERY_SCALE).round() as u16;
    put_be(&mut frame, m10::POS_BATTERY, &battery.to_le_bytes());
    put_be(&mut frame, m10::POS_SERIAL, &M10_SERIAL_RAW);
    frame[m10::POS_COUNTER] = fix.frame as u8;
    finish_meteomodem(&mut frame);
    frame
}

fn m10_humidity_ratio(humidity: f64, celsius: f64) -> f64 {
    let mut raw = humidity;
    if celsius < -30.0 {
        raw /= 1.0 + (-30.0 - celsius) / 75.0;
    }
    if celsius < 0.0 {
        raw += celsius / 5.5;
    }
    0.8955 + 0.002 * raw
}

fn m20_humidity_count(target: f64, calibration: u16, sensor_celsius: f64) -> u16 {
    (0..m20::MAX_HUMIDITY_COUNT)
        .min_by(|&a, &b| {
            let error = |count| {
                m20::humidity_from_counts(count, calibration, sensor_celsius)
                    .map_or(f64::INFINITY, |h| (h - target).abs())
            };
            error(a).total_cmp(&error(b))
        })
        .unwrap_or(0)
}

#[must_use]
pub fn m20_frame(index: u32) -> Vec<u8> {
    let flight = flight(SondeType::M20);
    let fix = flight.fix(index);
    let mut frame = vec![0u8; usize::from(m20::LEN_BYTE) + 1];
    frame[0] = m20::LEN_BYTE;
    frame[1] = m20::TYPE;
    let sensor_ohm = m20::HUMIDITY_R25_OHM
        * (m20::HUMIDITY_BETA * (1.0 / (flight.temperature_c + 273.15) - 1.0 / 298.15)).exp();
    let sensor_adc = adc_for(sensor_ohm, m20::HUMIDITY_SERIES_OHM).round() as u16;
    let sensor_celsius = m20::humidity_sensor_celsius(sensor_adc).unwrap_or(flight.temperature_c);
    let humidity = m20_humidity_count(flight.humidity_pct, 0, sensor_celsius);
    put_be(&mut frame, m20::POS_HUMIDITY, &humidity.to_le_bytes());
    let ohm = thermistor_ohm(m10::THERMISTOR, flight.temperature_c);
    let adc = adc_for(ohm, m10::SERIES_OHM[0]).round() as u16;
    put_be(&mut frame, m20::POS_THERMISTOR, &adc.to_le_bytes());
    put_be(
        &mut frame,
        m20::POS_HUMIDITY_THERMISTOR,
        &sensor_adc.to_le_bytes(),
    );
    put_be(
        &mut frame,
        m20::POS_ALT,
        &((fix.alt * 100.0).round() as u32).to_be_bytes()[1..],
    );
    for (at, value) in [
        (m20::POS_VEL_EAST, flight.east_ms),
        (m20::POS_VEL_NORTH, flight.north_ms),
        (m20::POS_VEL_UP, flight.up_ms),
    ] {
        put_be(
            &mut frame,
            at,
            &((value * 100.0).round() as i16).to_be_bytes(),
        );
    }
    let (week, tow) = gps_week_tow(fix.unix);
    put_be(&mut frame, m20::POS_TOW, &(tow as u32).to_be_bytes()[1..]);
    put_be(
        &mut frame,
        m20::POS_SERIAL,
        &M20_SERIAL_RAW.to_le_bytes()[..3],
    );
    frame[m20::POS_COUNTER] = fix.frame as u8;
    put_be(&mut frame, m20::POS_WEEK, &(week as u16).to_be_bytes());
    put_be(
        &mut frame,
        m20::POS_LAT,
        &((fix.lat * 1e6).round() as i32).to_be_bytes(),
    );
    put_be(
        &mut frame,
        m20::POS_LON,
        &((fix.lon * 1e6).round() as i32).to_be_bytes(),
    );
    frame[m20::POS_BATTERY] = (flight.battery_v / m20::BATTERY_SCALE).round() as u8;
    frame[m20::POS_FIRMWARE] = M20_FIRMWARE;
    let check = m20::block_check(&frame);
    put_be(&mut frame, m20::POS_BLOCK_CHECK, &check.to_be_bytes());
    finish_meteomodem(&mut frame);
    frame
}

fn imet_packet(mut body: Vec<u8>) -> Vec<u8> {
    let check = imet::crc(&body);
    body.extend_from_slice(&check.to_be_bytes());
    body
}

#[must_use]
pub fn imet_packets(index: u32) -> Vec<u8> {
    let flight = flight(SondeType::Imet4);
    let fix = flight.fix(index);
    let second = seconds_of_day(fix.unix);
    let mut gps = vec![imet::SOH, imet::PACKET_GPS];
    gps.extend_from_slice(&(fix.lat as f32).to_le_bytes());
    gps.extend_from_slice(&(fix.lon as f32).to_le_bytes());
    let altitude = (fix.alt + imet::ALTITUDE_OFFSET_M).round() as u16;
    gps.extend_from_slice(&altitude.to_le_bytes());
    gps.extend_from_slice(&[
        flight.satellites,
        (second / 3_600) as u8,
        ((second / 60) % 60) as u8,
        (second % 60) as u8,
    ]);
    let mut ptu = vec![imet::SOH, imet::PACKET_EXTENDED_PTU];
    ptu.extend_from_slice(&(fix.frame as u16).to_le_bytes());
    ptu.extend_from_slice(&((flight.pressure_hpa * 100.0).round() as u32).to_le_bytes()[..3]);
    ptu.extend_from_slice(&((flight.temperature_c * 100.0).round() as i16).to_le_bytes());
    ptu.extend_from_slice(&((flight.humidity_pct * 100.0).round() as u16).to_le_bytes());
    ptu.push((flight.battery_v * 10.0).round() as u8);
    ptu.extend_from_slice(&[0; 6]);
    let mut bytes = imet_packet(gps);
    bytes.extend(imet_packet(ptu));
    bytes
}

fn alternating(count: usize) -> Vec<bool> {
    (0..count).map(|k| k % 2 == 0).collect()
}

fn bits_lsb_first(bytes: &[u8]) -> impl Iterator<Item = bool> + '_ {
    bytes
        .iter()
        .flat_map(|&byte| (0..8).map(move |shift| (byte >> shift) & 1 == 1))
}

fn bits_msb_first(bytes: &[u8]) -> impl Iterator<Item = bool> + '_ {
    bytes
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |shift| (byte >> shift) & 1 == 1))
}

fn padded_seconds(symbols: &mut Vec<bool>, baud: f64, seconds: usize) {
    let target = (LEAD_IN_S * baud) as usize + (seconds as f64 * baud).round() as usize;
    while symbols.len() < target {
        let fill = symbols.len().is_multiple_of(2);
        symbols.push(fill);
    }
}

fn rs41_symbols(frames: u32) -> Vec<bool> {
    let mut symbols = alternating((LEAD_IN_S * rs41::BAUD) as usize);
    for index in 0..frames {
        symbols.extend(bits_lsb_first(&rs41_frame(index)));
        padded_seconds(&mut symbols, rs41::BAUD, index as usize + 1);
    }
    symbols
}

fn meteomodem_symbols(frames: u32, baud: f64, frame: fn(u32) -> Vec<u8>) -> Vec<bool> {
    let mut symbols = alternating((LEAD_IN_S * baud) as usize);
    for index in 0..frames {
        symbols.extend(meteomodem::RAW_HEADER.iter().map(|&c| c == b'1'));
        let mut previous = false;
        for bit in bits_msb_first(&frame(index)) {
            let level = if bit { previous } else { !previous };
            symbols.extend([level, !level]);
            previous = level;
        }
        padded_seconds(&mut symbols, baud, index as usize + 1);
    }
    symbols
}

fn modulate(
    symbols: &[bool],
    baud: f64,
    deviation_hz: f64,
    gaussian_bt: Option<f64>,
) -> Vec<Complex<f32>> {
    let sps = RATE / baud;
    let len = (symbols.len() as f64 * sps) as usize;
    let mut levels: Vec<f32> = (0..len)
        .map(|n| {
            let index = ((n as f64 / sps) as usize).min(symbols.len() - 1);
            if symbols[index] { 1.0 } else { -1.0 }
        })
        .collect();
    if let Some(bt) = gaussian_bt {
        let taps = design_gaussian(sps, bt, 4);
        let half = taps.len() / 2;
        let source = levels.clone();
        for (n, level) in levels.iter_mut().enumerate() {
            *level = taps
                .iter()
                .enumerate()
                .map(|(k, &tap)| {
                    let at = (n + k).saturating_sub(half).min(source.len() - 1);
                    tap * source[at]
                })
                .sum();
        }
    }
    fm_modulate(&levels, deviation_hz, RATE)
}

fn imet_audio(frames: u32) -> Vec<f32> {
    let mut bits = vec![true; (LEAD_IN_S * imet::BAUD) as usize];
    for index in 0..frames {
        for &byte in &imet_packets(index) {
            bits.push(false);
            bits.extend((0..8).map(|shift| (byte >> shift) & 1 == 1));
            bits.push(true);
        }
        let target = (LEAD_IN_S * imet::BAUD) as usize + (index as usize + 1) * imet::BAUD as usize;
        bits.resize(target, true);
    }
    let sps = (RATE / imet::BAUD) as usize;
    let mut phase = 0.0f64;
    bits.iter()
        .flat_map(|&bit| std::iter::repeat_n(bit, sps))
        .map(|bit| {
            let freq = if bit { imet::MARK_HZ } else { imet::SPACE_HZ };
            phase = (phase + TAU * freq / RATE).rem_euclid(TAU);
            phase.cos() as f32
        })
        .collect()
}

#[must_use]
pub fn transmission(sonde: SondeType, frames: u32, rate: f64) -> Vec<Complex<f32>> {
    let iq = match sonde {
        SondeType::Rs41 => modulate(
            &rs41_symbols(frames),
            rs41::BAUD,
            RS41_DEVIATION_HZ,
            Some(RS41_BT),
        ),
        SondeType::Dfm => modulate(&dfm_symbols(frames), dfm::BAUD, DFM_DEVIATION_HZ, None),
        SondeType::M10 => modulate(
            &meteomodem_symbols(frames, m10::BAUD, m10_frame),
            m10::BAUD,
            METEOMODEM_DEVIATION_HZ,
            None,
        ),
        SondeType::M20 => modulate(
            &meteomodem_symbols(frames, m20::BAUD, m20_frame),
            m20::BAUD,
            METEOMODEM_DEVIATION_HZ,
            None,
        ),
        SondeType::Imet4 => fm_modulate(&imet_audio(frames), IMET_DEVIATION_HZ, RATE),
    };
    if (rate - RATE).abs() < f64::EPSILON {
        iq
    } else {
        resample(&iq, RATE, rate)
    }
}
