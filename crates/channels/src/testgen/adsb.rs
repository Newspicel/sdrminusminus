//! Mode S reference modulator (PLAN §14): DF17 extended squitters and the DF4/5/11/20/21
//! replies a transponder sends, PPM-modulated onto complex baseband at 1 Mbit/s.
//!
//! The CPR, Gillham and 6-bit-callsign encoders here are written straight from DO-260B rather
//! than reused from [`crate::adsb`]. That is deliberate: NL(lat) is the closed form here and a
//! 58-entry table there, so a mistyped table digit surfaces as a failing position test instead
//! of cancelling out between generator and decoder.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::{mode_s_append_overlaid_parity, mode_s_append_parity};
use sdrmm_modem::ppm::SlotWaveform;

/// 6-bit identification charset (DO-260B §2.2.3.2.5.2).
const IDENT_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

const CPR_SCALE: f64 = 131_072.0;
const AIRBORNE_ZONE_DEG: f64 = 360.0;
const SURFACE_ZONE_DEG: f64 = 90.0;

/// Preamble pulse positions in half-chips: 0.0, 1.0, 3.5 and 4.5 µs into the 8 µs preamble.
const PREAMBLE_PULSES: [usize; 4] = [0, 2, 7, 9];
const PREAMBLE_CHIPS: usize = 16;

/// Build a DF17 extended squitter (parity appended) for the given ICAO and ME field.
#[must_use]
pub fn squitter(icao: u32, me: [u8; 7]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14);
    // DF 17, CA 5: a level-2 transponder that is airborne — what every airliner sends.
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

/// 8 six-bit characters at `offset`, upper-cased and space-padded — how both the extended
/// squitter's identification field and the BDS 2,0 register spell a callsign.
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

/// Aircraft identification (TC 4, category set A). The callsign is upper-cased, padded with
/// spaces and truncated to the 8 characters the field holds.
#[must_use]
pub fn me_identification(callsign: &str) -> [u8; 7] {
    let mut me = [0u8; 7];
    put_bits(&mut me, 0, 5, 4);
    put_callsign(&mut me, 8, callsign);
    me
}

/// Airborne position with barometric altitude (TC 11). `alt_ft` is rounded to the nearest
/// step the encoding carries: 25 ft up to 50 175 ft, 100 ft above it.
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

/// Airborne position with GNSS height above the ellipsoid (TC 21). The type code changes the
/// altitude *source*, not its encoding — the AC12 field is the same Q-bit / Gillham code in
/// feet that TC 9–18 uses.
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

/// An airborne position frame with the AC12 field written verbatim, so a test can exercise
/// the "altitude not available" code that no altitude-taking builder can express.
#[must_use]
pub fn position_me_raw(tc: u64, ac12: u64, lat: f64, lon: f64, odd: bool) -> [u8; 7] {
    position_me(tc, ac12, lat, lon, odd, AIRBORNE_ZONE_DEG)
}

/// Surface position (TC 6). Movement and ground track are left at "no information".
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

/// Airborne velocity, subtype 1 (TC 19). Velocities are whole knots and the vertical rate is
/// a multiple of 64 ft/min, so the decoder round-trips them to within one quantisation step.
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

/// The BDS 2,0 Comm-B register: the register's own code followed by the same 8 six-bit
/// characters an identification squitter sends (DO-181E §2.2.19.1.12).
#[must_use]
pub fn mb_identification(callsign: &str) -> [u8; 7] {
    let mut mb = [0u8; 7];
    put_bits(&mut mb, 0, 8, 0x20);
    put_callsign(&mut mb, 8, callsign);
    mb
}

/// An all-call reply (DF11): the address in the clear, and the parity keyed with the
/// identifier of the interrogator that triggered it.
#[must_use]
pub fn all_call_reply(icao: u32, capability: u64, interrogator: u32) -> Vec<u8> {
    let mut frame = vec![0u8; 4];
    put_bits(&mut frame, 0, 5, 11);
    put_bits(&mut frame, 5, 3, capability);
    put_bits(&mut frame, 8, 24, u64::from(icao));
    mode_s_append_overlaid_parity(&mut frame, interrogator);
    frame
}

/// A surveillance altitude reply (DF4).
#[must_use]
pub fn altitude_reply(icao: u32, alt_ft: i32, flight_status: u64) -> Vec<u8> {
    reply(4, icao, flight_status, altitude_field13(alt_ft), None)
}

/// A surveillance identity reply (DF5) carrying the four octal squawk digits.
///
/// # Panics
/// If `squawk` is not four octal digits.
#[must_use]
pub fn identity_reply(icao: u32, squawk: &str, flight_status: u64) -> Vec<u8> {
    reply(5, icao, flight_status, identity_field13(squawk), None)
}

/// A Comm-B altitude reply (DF20): a [`altitude_reply`] with 56 further bits of register.
#[must_use]
pub fn comm_b_altitude_reply(icao: u32, alt_ft: i32, flight_status: u64, mb: [u8; 7]) -> Vec<u8> {
    reply(20, icao, flight_status, altitude_field13(alt_ft), Some(mb))
}

/// A Comm-B identity reply (DF21): an [`identity_reply`] with 56 further bits of register.
///
/// # Panics
/// If `squawk` is not four octal digits.
#[must_use]
pub fn comm_b_identity_reply(icao: u32, squawk: &str, flight_status: u64, mb: [u8; 7]) -> Vec<u8> {
    reply(21, icao, flight_status, identity_field13(squawk), Some(mb))
}

/// A reply to a roll-call interrogation: DF(5) FS(3) DR(5) UM(6) then the 13-bit AC or ID
/// field, optionally 56 bits of Comm-B, and the parity with the address keyed onto it. DR and
/// UM are left at "no request" and "no information", which is what a transponder with nothing
/// pending sends.
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

/// The 13-bit AC field of a surveillance reply: the extended squitter's 12-bit field with the
/// M bit — metric units, which nothing sends — inserted clear after A4.
fn altitude_field13(alt_ft: i32) -> u64 {
    let ac12 = barometric_field(alt_ft);
    (ac12 & 0xFC0) << 1 | (ac12 & 0x03F)
}

/// The 13-bit ID field, from four octal digits. Each digit's bits are named for their weight
/// and interleaved with the others': C1 A1 C2 A2 C4 A4 X B1 D1 B2 D2 B4 D4.
///
/// # Panics
/// If `squawk` is not four octal digits.
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
    // (weight bit of the digit, index in the field), most significant index first.
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

/// The 12-bit AC field: 25 ft steps with the Q bit set, Gillham-coded 100 ft steps above
/// 50 175 ft where the 11-bit counter runs out.
fn barometric_field(alt_ft: i32) -> u64 {
    if alt_ft <= 50_175 {
        let n = (f64::from(alt_ft + 1_000) / 25.0).round().max(0.0) as u64;
        return (n & 0x7F0) << 1 | 0x10 | (n & 0x0F);
    }
    let steps = (f64::from(alt_ft) / 100.0).round() as i64 + 13;
    let five_hundreds = ((steps - 1) / 5).clamp(0, 255) as u32;
    let hundreds = (steps - i64::from(five_hundreds) * 5) as u32;
    // Inverse of the decoder's two involutions: odd 500 ft bands count backwards, and 5 and 7
    // are exchanged in the C bits.
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
    // Field order C1 A1 C2 A2 C4 A4 B1 D1 B2 D2 B4 D4, D1 (the Q bit) clear.
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

/// Positive-remainder modulo, as CPR is defined with.
fn modulo(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// Longitude zones as a function of latitude (DO-260B Appendix A), in closed form.
#[must_use]
pub fn cpr_nl(lat: f64) -> i32 {
    let lat = lat.abs();
    if lat >= 87.0 {
        return 1;
    }
    // The formula is exactly 60 at the equator, where the zone count is defined as 59.
    if lat == 0.0 {
        return 59;
    }
    let a = 1.0 - (1.0 - (PI / 30.0).cos()) / (lat.to_radians().cos().powi(2));
    (TAU / a.acos()).floor() as i32
}

/// CPR encoding of a position into the 17-bit lat/lon pair of one frame (DO-260B
/// §2.2.3.2.6.4). `zone` is 360° for airborne frames, 90° for surface ones.
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

/// The half-chip (0.5 µs) on/off timeline of one frame: 8 µs preamble then two chips per bit,
/// energy in the first half for a 1 and the second half for a 0.
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

/// PPM-modulate `frames` into complex baseband IQ at `rate`, at amplitude `level`, with
/// `gap_us` of silence before each frame and after the last. Every frame lands phase-0: its
/// chip timeline starts exactly on its first sample.
#[must_use]
pub fn transmission(frames: &[Vec<u8>], gap_us: f64, level: f32, rate: f64) -> Vec<Complex<f32>> {
    transmission_at_phase(frames, gap_us, level, rate, 0.0)
}

/// [`transmission`] with each frame's chip timeline starting `phase` samples (fractional,
/// `0..1`) after the frame's first sample — the alignment a transmitter's bit clock never
/// owes the receiver's sample grid, and the one thing a decoder test must sweep: at phase 0
/// an idealized generator's sample-to-chip mapping can coincide with the decoder's windows,
/// and such a test proves only that the two share their arithmetic. This one did, and the
/// green suite hid a decoder that decoded nothing off the grid.
///
/// Rendering is `modem::ppm`'s [`SlotWaveform`] — the library's keyed-slot transmitter, which
/// integrates a sub-sample-resolved waveform over each output sample period the way a
/// receiver's own decimation chain delivers it: a pulse edge inside a sample reads as partial
/// amplitude, not as the hard cliff no band-limited front end can produce. At an integer
/// samples-per-chip and phase 0 the chip edges coincide with the sample apertures and the
/// output is exactly the ideal rectangular waveform. What stays here is the Mode S *timeline*
/// ([`half_chips`]): which slots radiate is protocol, how they are rendered is modulation.
#[must_use]
pub fn transmission_at_phase(
    frames: &[Vec<u8>],
    gap_us: f64,
    level: f32,
    rate: f64,
    phase: f64,
) -> Vec<Complex<f32>> {
    let samples_per_us = rate / 1e6;
    // Gaps and frame extents are whole output samples, so every frame starts on the output
    // grid and `phase` alone says where its chips sit within a sample.
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

    /// The published DF17 identification squitter for ICAO 4840D6 / KLM1023.
    #[test]
    fn identification_matches_the_published_frame() {
        let me = me_identification("KLM1023");
        assert_eq!(me, [0x20, 0x2C, 0xC3, 0x71, 0xC3, 0x2C, 0xE0]);
    }

    /// The published even frame for ICAO 40621D at 38 000 ft: CPR (93000, 51372).
    #[test]
    fn airborne_position_matches_the_published_frame() {
        let me = me_airborne_position(38_000, 52.257_2, 3.919_37, false);
        assert_eq!(me[0] >> 3, 11);
        let cpr_lat = u32::from(me[2] & 0x03) << 15 | u32::from(me[3]) << 7 | u32::from(me[4]) >> 1;
        let cpr_lon = u32::from(me[4] & 0x01) << 16 | u32::from(me[5]) << 8 | u32::from(me[6]);
        // The published position is quoted to 4 decimals, which is ±1 CPR count.
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
        // 10 µs lead-in + 8 µs preamble + 112 µs of bits + 10 µs lead-out, at 2 samples/µs.
        assert_eq!(iq.len(), 2 * (10 + 8 + 112 + 10));
    }
}
