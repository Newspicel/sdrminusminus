use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::{mode_s_append_overlaid_parity, mode_s_append_parity};
use sdrmm_modem::ppm::SlotWaveform;

const IDENT_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

const CPR_SCALE: f64 = 131_072.0;
const AIRBORNE_ZONE_DEG: f64 = 360.0;
const SURFACE_ZONE_DEG: f64 = 90.0;

const PREAMBLE_PULSES: [usize; 4] = [0, 2, 7, 9];
const PREAMBLE_CHIPS: usize = 16;

#[must_use]
pub fn squitter(icao: u32, me: [u8; 7]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14);
    frame.push(17 << 3 | 5);
    frame.extend_from_slice(&icao.to_be_bytes()[1..]);
    frame.extend_from_slice(&me);
    mode_s_append_parity(&mut frame);
    frame
}

fn put_bits(target: &mut [u8], offset: usize, len: usize, value: u64) {
    for i in 0..len {
        if value >> (len - 1 - i) & 1 == 1 {
            let bit = offset + i;
            if let Some(byte) = target.get_mut(bit / 8) {
                *byte |= 0x80 >> (bit % 8);
            }
        }
    }
}

fn put_callsign(target: &mut [u8], offset: usize, callsign: &str) {
    for (i, ch) in callsign
        .chars()
        .chain(std::iter::repeat(' '))
        .take(8)
        .enumerate()
    {
        let upper = ch.to_ascii_uppercase() as u8;
        let code = IDENT_CHARSET.iter().position(|&c| c == upper).unwrap_or(0);
        put_bits(target, offset + i * 6, 6, code as u64);
    }
}

#[must_use]
pub fn me_identification(callsign: &str) -> [u8; 7] {
    let mut me = [0u8; 7];
    put_bits(&mut me, 0, 5, 4);
    put_callsign(&mut me, 8, callsign);
    me
}

#[must_use]
pub fn me_airborne_position(alt_ft: i32, lat: f64, lon: f64, odd: bool) -> [u8; 7] {
    position_me(
        11,
        barometric_field(alt_ft),
        lat,
        lon,
        odd,
        AIRBORNE_ZONE_DEG,
    )
}

#[must_use]
pub fn me_airborne_position_gnss(alt_ft: i32, lat: f64, lon: f64, odd: bool) -> [u8; 7] {
    position_me(
        21,
        barometric_field(alt_ft),
        lat,
        lon,
        odd,
        AIRBORNE_ZONE_DEG,
    )
}

#[must_use]
pub fn position_me_raw(tc: u64, ac12: u64, lat: f64, lon: f64, odd: bool) -> [u8; 7] {
    position_me(tc, ac12, lat, lon, odd, AIRBORNE_ZONE_DEG)
}

#[must_use]
pub fn me_surface_position(lat: f64, lon: f64, odd: bool) -> [u8; 7] {
    position_me(6, 0, lat, lon, odd, SURFACE_ZONE_DEG)
}

fn position_me(tc: u64, altitude: u64, lat: f64, lon: f64, odd: bool, zone: f64) -> [u8; 7] {
    let mut me = [0u8; 7];
    put_bits(&mut me, 0, 5, tc);
    put_bits(&mut me, 8, 12, altitude);
    put_bits(&mut me, 21, 1, u64::from(odd));
    let (lat_cpr, lon_cpr) = cpr_encode(lat, lon, odd, zone);
    put_bits(&mut me, 22, 17, u64::from(lat_cpr));
    put_bits(&mut me, 39, 17, u64::from(lon_cpr));
    me
}

#[must_use]
pub fn me_velocity(ground_speed_kt: f64, track_deg: f64, vertical_rate_fpm: i32) -> [u8; 7] {
    let mut me = [0u8; 7];
    put_bits(&mut me, 0, 5, 19);
    put_bits(&mut me, 5, 3, 1);
    let (sin, cos) = track_deg.to_radians().sin_cos();
    let component = |v: f64| (v.abs().round() as u64 + 1).min(1_023);
    let east_west = ground_speed_kt * sin;
    let north_south = ground_speed_kt * cos;
    put_bits(&mut me, 13, 1, u64::from(east_west < 0.0));
    put_bits(&mut me, 14, 10, component(east_west));
    put_bits(&mut me, 24, 1, u64::from(north_south < 0.0));
    put_bits(&mut me, 25, 10, component(north_south));
    put_bits(&mut me, 36, 1, u64::from(vertical_rate_fpm < 0));
    let rate = (f64::from(vertical_rate_fpm.abs()) / 64.0).round() as u64;
    put_bits(&mut me, 37, 9, (rate + 1).min(511));
    me
}

#[must_use]
pub fn mb_identification(callsign: &str) -> [u8; 7] {
    let mut mb = [0u8; 7];
    put_bits(&mut mb, 0, 8, 0x20);
    put_callsign(&mut mb, 8, callsign);
    mb
}

#[must_use]
pub fn all_call_reply(icao: u32, capability: u64, interrogator: u32) -> Vec<u8> {
    let mut frame = vec![0u8; 4];
    put_bits(&mut frame, 0, 5, 11);
    put_bits(&mut frame, 5, 3, capability);
    put_bits(&mut frame, 8, 24, u64::from(icao));
    mode_s_append_overlaid_parity(&mut frame, interrogator);
    frame
}

#[must_use]
pub fn altitude_reply(icao: u32, alt_ft: i32, flight_status: u64) -> Vec<u8> {
    reply(4, icao, flight_status, altitude_field13(alt_ft), None)
}

#[must_use]
pub fn identity_reply(icao: u32, squawk: &str, flight_status: u64) -> Vec<u8> {
    reply(5, icao, flight_status, identity_field13(squawk), None)
}

#[must_use]
pub fn comm_b_altitude_reply(icao: u32, alt_ft: i32, flight_status: u64, mb: [u8; 7]) -> Vec<u8> {
    reply(20, icao, flight_status, altitude_field13(alt_ft), Some(mb))
}

#[must_use]
pub fn comm_b_identity_reply(icao: u32, squawk: &str, flight_status: u64, mb: [u8; 7]) -> Vec<u8> {
    reply(21, icao, flight_status, identity_field13(squawk), Some(mb))
}

fn reply(df: u64, icao: u32, flight_status: u64, field13: u64, mb: Option<[u8; 7]>) -> Vec<u8> {
    let mut frame = vec![0u8; 4];
    put_bits(&mut frame, 0, 5, df);
    put_bits(&mut frame, 5, 3, flight_status);
    put_bits(&mut frame, 19, 13, field13);
    if let Some(mb) = mb {
        frame.extend_from_slice(&mb);
    }
    mode_s_append_overlaid_parity(&mut frame, icao);
    frame
}

fn altitude_field13(alt_ft: i32) -> u64 {
    let ac12 = barometric_field(alt_ft);
    (ac12 & 0xFC0) << 1 | (ac12 & 0x03F)
}

fn identity_field13(squawk: &str) -> u64 {
    let digits: Vec<u64> = squawk
        .chars()
        .filter_map(|c| c.to_digit(8))
        .map(u64::from)
        .collect();
    let [a, b, c, d] = digits[..] else {
        panic!("squawk must be four octal digits, got {squawk:?}")
    };
    let mut field = 0u64;
    for (value, places) in [
        (a, [1, 3, 5]),
        (b, [7, 9, 11]),
        (c, [0, 2, 4]),
        (d, [8, 10, 12]),
    ] {
        for (weight, index) in places.into_iter().enumerate() {
            field |= (value >> weight & 1) << (12 - index);
        }
    }
    field
}

fn barometric_field(alt_ft: i32) -> u64 {
    if alt_ft <= 50_175 {
        let n = (f64::from(alt_ft + 1_000) / 25.0).round().max(0.0) as u64;
        return (n & 0x7F0) << 1 | 0x10 | (n & 0x0F);
    }
    let steps = (f64::from(alt_ft) / 100.0).round() as i64 + 13;
    let five_hundreds = ((steps - 1) / 5).clamp(0, 255) as u32;
    let hundreds = (steps - i64::from(five_hundreds) * 5) as u32;
    let mut c = if five_hundreds & 1 == 1 {
        6 - hundreds
    } else {
        hundreds
    };
    if c & 5 == 5 {
        c ^= 2;
    }
    let gray_five = gray_encode(five_hundreds);
    let gray_c = gray_encode(c);
    let bit = |value: u32, index: u32| u64::from(value >> index & 1);
    bit(gray_c, 2) << 11
        | bit(gray_five, 5) << 10
        | bit(gray_c, 1) << 9
        | bit(gray_five, 4) << 8
        | bit(gray_c, 0) << 7
        | bit(gray_five, 3) << 6
        | bit(gray_five, 2) << 5
        | bit(gray_five, 1) << 3
        | bit(gray_five, 7) << 2
        | bit(gray_five, 0) << 1
        | bit(gray_five, 6)
}

fn gray_encode(value: u32) -> u32 {
    value ^ (value >> 1)
}

fn modulo(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

#[must_use]
pub fn cpr_nl(lat: f64) -> i32 {
    let lat = lat.abs();
    if lat >= 87.0 {
        return 1;
    }
    if lat == 0.0 {
        return 59;
    }
    let a = 1.0 - (1.0 - (PI / 30.0).cos()) / (lat.to_radians().cos().powi(2));
    (TAU / a.acos()).floor() as i32
}

#[must_use]
pub fn cpr_encode(lat: f64, lon: f64, odd: bool, zone: f64) -> (u32, u32) {
    let i = f64::from(i32::from(odd));
    let dlat = zone / (60.0 - i);
    let yz = (CPR_SCALE * modulo(lat, dlat) / dlat + 0.5).floor();
    let rlat = dlat * (yz / CPR_SCALE + (lat / dlat).floor());

    let ni = f64::from((cpr_nl(rlat) - i32::from(odd)).max(1));
    let dlon = zone / ni;
    let xz = (CPR_SCALE * modulo(lon, dlon) / dlon + 0.5).floor();

    (modulo(yz, CPR_SCALE) as u32, modulo(xz, CPR_SCALE) as u32)
}

fn half_chips(frame: &[u8]) -> Vec<bool> {
    let mut chips = vec![false; PREAMBLE_CHIPS + frame.len() * 8 * 2];
    for pulse in PREAMBLE_PULSES {
        if let Some(chip) = chips.get_mut(pulse) {
            *chip = true;
        }
    }
    for (i, byte) in frame.iter().enumerate() {
        for k in 0..8 {
            let one = byte >> (7 - k) & 1 == 1;
            let at = PREAMBLE_CHIPS + (i * 8 + k) * 2;
            if let Some(chip) = chips.get_mut(if one { at } else { at + 1 }) {
                *chip = true;
            }
        }
    }
    chips
}

#[must_use]
pub fn transmission(frames: &[Vec<u8>], gap_us: f64, level: f32, rate: f64) -> Vec<Complex<f32>> {
    transmission_at_phase(frames, gap_us, level, rate, 0.0)
}

#[must_use]
pub fn transmission_at_phase(
    frames: &[Vec<u8>],
    gap_us: f64,
    level: f32,
    rate: f64,
    phase: f64,
) -> Vec<Complex<f32>> {
    let samples_per_us = rate / 1e6;
    let gap = (gap_us * samples_per_us).round().max(0.0) as usize;
    let waveform = SlotWaveform::new(samples_per_us * 0.5, phase, level);
    let mut iq: Vec<Complex<f32>> = Vec::new();
    for frame in frames {
        iq.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), gap));
        waveform.render(&half_chips(frame), &mut iq);
    }
    iq.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), gap));
    iq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identification_matches_the_published_frame() {
        let me = me_identification("KLM1023");
        assert_eq!(me, [0x20, 0x2C, 0xC3, 0x71, 0xC3, 0x2C, 0xE0]);
    }

    #[test]
    fn airborne_position_matches_the_published_frame() {
        let me = me_airborne_position(38_000, 52.257_2, 3.919_37, false);
        assert_eq!(me[0] >> 3, 11);
        let cpr_lat = u32::from(me[2] & 0x03) << 15 | u32::from(me[3]) << 7 | u32::from(me[4]) >> 1;
        let cpr_lon = u32::from(me[4] & 0x01) << 16 | u32::from(me[5]) << 8 | u32::from(me[6]);
        assert!(cpr_lat.abs_diff(93_000) <= 2, "lat cpr {cpr_lat}");
        assert!(cpr_lon.abs_diff(51_372) <= 2, "lon cpr {cpr_lon}");
    }

    #[test]
    fn the_preamble_lands_on_the_documented_samples() {
        let iq = transmission(&[vec![0x00; 7]], 0.0, 1.0, 2_000_000.0);
        let high: Vec<usize> = iq[..16]
            .iter()
            .enumerate()
            .filter(|(_, s)| s.norm() > 0.5)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(high, vec![0, 2, 7, 9]);
    }

    #[test]
    fn a_frame_is_one_microsecond_per_bit() {
        let iq = transmission(&[vec![0x00; 14]], 10.0, 1.0, 2_000_000.0);
        assert_eq!(iq.len(), 2 * (10 + 8 + 112 + 10));
    }
}
