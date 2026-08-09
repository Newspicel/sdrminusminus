//! ADS-B / Mode S decoder (PLAN §13 P2): 1090 MHz PPM at 1 Mbit/s, preamble correlation
//! and the Mode S CRC-24. The channel runs at exactly 2 samples per bit, which makes the
//! 8 µs preamble 16 samples and every bit a clean two-half comparison.
//!
//! Only DF17/DF18 extended squitters are accepted. Every other downlink format overlays the
//! aircraft address (or the interrogator id) on the parity, so a zero syndrome is not
//! evidence that the frame is real — accepting them off-air would mean inventing aircraft.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{bits_be, mode_s_fix_single_bit, mode_s_syndrome};
use sdrmm_wire::{
    AdsbMessage, AdsbParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

/// 2 Msps: 1 bit = 1 µs = 2 samples.
pub(crate) const INPUT_RATE_HZ: f64 = 2_000_000.0;

const SAMPLES_PER_BIT: usize = 2;
/// 8 µs preamble with 0.5 µs pulses at 0.0, 1.0, 3.5 and 4.5 µs (ICAO Annex 10 Vol IV
/// §3.1.2.3.1) — samples 0, 2, 7, 9 of a 16-sample window.
const PREAMBLE_SAMPLES: usize = 16;
const SHORT_BYTES: usize = 7;
const LONG_BYTES: usize = 14;
const LONG_FRAME_SAMPLES: usize = PREAMBLE_SAMPLES + LONG_BYTES * 8 * SAMPLES_PER_BIT;
/// Largest ratio tolerated between the strongest and weakest preamble pulse. A real preamble
/// is four equal pulses; one noise spike plus three background samples is not one.
const PULSE_SPREAD: f32 = 4.0;

/// Bit offsets into a long frame (DO-260B §2.2.3): DF(5) CA(3) AA(24) ME(56) PI(24).
const ICAO_OFFSET_BITS: usize = 8;
const ME_OFFSET_BITS: usize = 32;

/// 6-bit identification charset (DO-260B §2.2.3.2.5.2): index 0 and the reserved ranges are
/// `#`, 32 is a space.
const IDENT_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

/// CPR zone height: 360° for airborne frames, 90° for surface ones (DO-260B §2.2.3.2.6.4).
const AIRBORNE_ZONE_DEG: f64 = 360.0;
const SURFACE_ZONE_DEG: f64 = 90.0;
const CPR_SCALE: f64 = 131_072.0;

/// Aircraft held for CPR even/odd pairing. A busy urban receiver hears 30–50 airframes at
/// once; past that the least recently heard entry is evicted, so an airshow (or a noisy
/// antenna inventing addresses) cannot grow this without bound.
const CPR_CACHE_LEN: usize = 64;
/// An even/odd pair may only be solved globally while both frames are fresh: DO-260B
/// §2.2.3.2.6.5 allows 10 s, beyond which the aircraft has flown out of its own zone.
const CPR_PAIR_MAX_AGE_SAMPLES: u64 = (10.0 * INPUT_RATE_HZ) as u64;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "adsb".to_owned(),
    name: "ADS-B (1090ES)".to_owned(),
    bandwidth_hz: INPUT_RATE_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("adsb".to_owned()),
});

#[derive(Clone, Copy)]
struct CprFix {
    lat: u32,
    lon: u32,
    /// Absolute stream position of the frame, in samples — the DSP plane's only clock.
    at: u64,
}

struct Aircraft {
    icao: u32,
    even: Option<CprFix>,
    odd: Option<CprFix>,
    last: u64,
}

impl Aircraft {
    fn new(icao: u32, at: u64) -> Self {
        Self {
            icao,
            even: None,
            odd: None,
            last: at,
        }
    }
}

pub struct AdsbChannel {
    crc_fix: bool,
    reference: Option<(f64, f64)>,
    /// Sample magnitudes: the tail of the previous block followed by the current one.
    mag: Vec<f32>,
    /// Absolute stream index of `mag[0]`.
    stream_pos: u64,
    cpr: Vec<Aircraft>,
}

fn params(settings: &ChannelSettings) -> Result<&AdsbParams, ChannelError> {
    match &settings.params {
        ChannelParams::Adsb(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "adsb channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &AdsbParams) -> Result<(), ChannelError> {
    if let Some(lat) = p.ref_lat
        && !(lat.is_finite() && (-90.0..=90.0).contains(&lat))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "adsb ref_lat must be within ±90°, got {lat}"
        )));
    }
    if let Some(lon) = p.ref_lon
        && !(lon.is_finite() && (-180.0..=180.0).contains(&lon))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "adsb ref_lon must be within ±180°, got {lon}"
        )));
    }
    if p.ref_lat.is_some() != p.ref_lon.is_some() {
        return Err(ChannelError::InvalidSettings(
            "adsb reference position needs both ref_lat and ref_lon".to_owned(),
        ));
    }
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band() -> (f64, f64) {
    let half = INPUT_RATE_HZ / 2.0;
    (-half, half)
}

/// ADS-B keeps the DDC's full output band: the pulses are 0.5 µs, so every extra filter
/// stage costs rise time the bit slicer needs. The DDC's own anti-alias response is the
/// channel selectivity here.
pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Passthrough
}

/// Preamble correlation over one 16-sample window. The accept threshold is derived from the
/// pulses themselves — receive levels differ by tens of dB between an overhead aircraft and
/// one at the horizon, so no fixed level can gate this.
fn preamble_ok(w: &[f32; PREAMBLE_SAMPLES]) -> bool {
    // Cheapest discriminator first: noise fails one of these four most of the time, which is
    // what keeps the per-sample cost near the magnitude computation itself.
    if !(w[0] > w[1] && w[2] > w[3] && w[7] > w[8] && w[9] > w[10]) {
        return false;
    }
    let pulses = [w[0], w[2], w[7], w[9]];
    let gaps = [
        w[1], w[3], w[4], w[5], w[6], w[8], w[10], w[11], w[12], w[13], w[14], w[15],
    ];
    let mean = pulses.iter().sum::<f32>() * 0.25;
    if mean <= 0.0 {
        return false;
    }
    let weakest = pulses.iter().copied().fold(f32::INFINITY, f32::min);
    let strongest = pulses.iter().copied().fold(0.0, f32::max);
    if strongest > weakest * PULSE_SPREAD {
        return false;
    }
    let threshold = mean * 0.5;
    weakest > threshold && gaps.iter().all(|&g| g < threshold)
}

/// PPM slicing: a 1 is energy in the first half of the bit, a 0 in the second.
fn slice_bits(body: &[f32], frame: &mut [u8; LONG_BYTES]) {
    let (bytes, _) = body.as_chunks::<{ SAMPLES_PER_BIT * 8 }>();
    for (byte, samples) in frame.iter_mut().zip(bytes) {
        let mut value = 0u8;
        for &[first, second] in samples.as_chunks::<SAMPLES_PER_BIT>().0 {
            value = value << 1 | u8::from(first > second);
        }
        *byte = value;
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        for nibble in [b >> 4, b & 0x0F] {
            if let Some(c) = char::from_digit(u32::from(nibble), 16) {
                out.push(c.to_ascii_uppercase());
            }
        }
    }
    out
}

/// Gray code to binary, for the Gillham altitude fields (any width up to 16 bits).
fn gray_decode(gray: u32) -> u32 {
    let mut b = gray;
    b ^= b >> 8;
    b ^= b >> 4;
    b ^= b >> 2;
    b ^= b >> 1;
    b
}

/// 12-bit AC field of an airborne position frame (DO-260B §2.2.3.2.3.4.3). Bit 4 of the
/// field is Q: set for the 25 ft encoding, clear for the Gillham-coded 100 ft one used above
/// 50 175 ft. `None` when the transmitter reports no altitude.
fn barometric_altitude(ac12: u32) -> Option<i32> {
    if ac12 == 0 {
        return None;
    }
    if ac12 & 0x10 != 0 {
        let n = ((ac12 & 0x0FE0) >> 1) | (ac12 & 0x000F);
        return Some(i32::try_from(n).ok()? * 25 - 1000);
    }
    gillham_altitude(ac12)
}

/// Mode C altitude from the 12-bit AC field with Q clear. Field order is
/// C1 A1 C2 A2 C4 A4 B1 D1 B2 D2 B4 D4 — the 13-bit interrogation field without its M bit.
fn gillham_altitude(ac12: u32) -> Option<i32> {
    let bit = |index: u32| (ac12 >> (11 - index)) & 1;
    let (c1, a1, c2, a2, c4, a4) = (bit(0), bit(1), bit(2), bit(3), bit(4), bit(5));
    let (b1, b2, d2, b4, d4) = (bit(6), bit(8), bit(9), bit(10), bit(11));
    // D1 (bit 7 here) is the Q bit and is zero on this path, so the 500 ft Gray code starts
    // at D2.
    let five_hundreds =
        gray_decode(d2 << 7 | d4 << 6 | a1 << 5 | a2 << 4 | a4 << 3 | b1 << 2 | b2 << 1 | b4);
    let mut hundreds = gray_decode(c1 << 2 | c2 << 1 | c4);
    // The C bits count 1..5 with 5 and 7 exchanged, and run backwards inside odd 500 ft bands.
    if hundreds & 5 == 5 {
        hundreds ^= 2;
    }
    if !(1..=5).contains(&hundreds) {
        return None;
    }
    if five_hundreds & 1 == 1 {
        hundreds = 6 - hundreds;
    }
    let steps = i32::try_from(five_hundreds * 5 + hundreds).ok()? - 13;
    (steps >= -12).then_some(steps * 100)
}

fn callsign(frame: &[u8]) -> Option<String> {
    let mut text = String::with_capacity(8);
    for i in 0..8 {
        let code = bits_be(frame, ME_OFFSET_BITS + 8 + i * 6, 6) as usize;
        text.push(char::from(*IDENT_CHARSET.get(code)?));
    }
    let trimmed = text.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Airborne velocity, TC 19 subtypes 1 and 2 (DO-260B §2.2.3.2.6.1). Subtypes 3/4 report
/// airspeed and heading instead of a ground vector and are deliberately left undecoded.
fn velocity(frame: &[u8], msg: &mut AdsbMessage) {
    let subtype = bits_be(frame, ME_OFFSET_BITS + 5, 3);
    if subtype == 1 || subtype == 2 {
        // Subtype 2 is the supersonic scale: the same fields in 4 kt steps.
        let scale = if subtype == 2 { 4.0 } else { 1.0 };
        let east_west = bits_be(frame, ME_OFFSET_BITS + 14, 10);
        let north_south = bits_be(frame, ME_OFFSET_BITS + 25, 10);
        // Zero means "no velocity information"; the encoded value is the speed plus one.
        if east_west != 0 && north_south != 0 {
            let sign = |bit: usize| {
                if bits_be(frame, ME_OFFSET_BITS + bit, 1) == 1 {
                    -1.0
                } else {
                    1.0
                }
            };
            let vx = sign(13) * (east_west - 1) as f64 * scale;
            let vy = sign(24) * (north_south - 1) as f64 * scale;
            msg.ground_speed_kt = Some(vx.hypot(vy));
            msg.track_deg = Some(vx.atan2(vy).to_degrees().rem_euclid(360.0));
        }
    }
    let rate = bits_be(frame, ME_OFFSET_BITS + 37, 9);
    if rate != 0 {
        let fpm = (rate as i32 - 1) * 64;
        msg.vertical_rate_fpm = Some(if bits_be(frame, ME_OFFSET_BITS + 36, 1) == 1 {
            -fpm
        } else {
            fpm
        });
    }
}

/// Positive-remainder modulo. CPR is built on it: `%` would return a negative remainder for
/// southern latitudes and western longitudes and put the aircraft a zone away.
fn modulo(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// Longitude zones as a function of latitude (DO-260B Appendix A table A-2): the table lists
/// the northern limit of each zone count, from 59 down to 2.
const NL_BOUNDARIES: [f64; 58] = [
    10.470_471_30,
    14.828_174_37,
    18.186_263_57,
    21.029_394_93,
    23.545_044_87,
    25.829_247_07,
    27.938_987_10,
    29.911_356_86,
    31.772_097_08,
    33.539_934_36,
    35.228_995_98,
    36.850_251_08,
    38.412_418_92,
    39.922_566_84,
    41.386_518_32,
    42.809_140_12,
    44.194_549_51,
    45.546_267_23,
    46.867_332_52,
    48.160_391_28,
    49.427_764_39,
    50.671_501_66,
    51.893_424_69,
    53.095_161_53,
    54.278_174_72,
    55.443_784_44,
    56.593_187_56,
    57.727_473_54,
    58.847_637_76,
    59.954_592_77,
    61.049_177_74,
    62.132_166_59,
    63.204_274_79,
    64.266_165_23,
    65.318_453_10,
    66.361_710_08,
    67.396_467_74,
    68.423_220_22,
    69.442_426_31,
    70.454_510_75,
    71.459_864_73,
    72.458_845_45,
    73.451_774_42,
    74.438_934_16,
    75.420_562_57,
    76.396_843_91,
    77.367_894_61,
    78.333_740_83,
    79.294_282_25,
    80.249_232_13,
    81.198_013_49,
    82.139_569_81,
    83.071_994_45,
    83.991_735_63,
    84.891_661_91,
    85.755_416_21,
    86.535_369_98,
    87.000_000_00,
];

fn cpr_nl(lat: f64) -> i32 {
    let lat = lat.abs();
    NL_BOUNDARIES
        .iter()
        .position(|&limit| lat < limit)
        .map_or(1, |zone| 59 - zone as i32)
}

/// Global CPR: an even/odd pair fixes the position outright (DO-260B §2.2.3.2.6.5). Airborne
/// frames only — surface zones are a quarter as tall, which leaves a four-way ambiguity that
/// only a receiver reference resolves.
fn cpr_global(even: &CprFix, odd: &CprFix, latest_odd: bool) -> Option<(f64, f64)> {
    let lat_even = f64::from(even.lat) / CPR_SCALE;
    let lat_odd = f64::from(odd.lat) / CPR_SCALE;
    let lon_even = f64::from(even.lon) / CPR_SCALE;
    let lon_odd = f64::from(odd.lon) / CPR_SCALE;

    let zone = (59.0 * lat_even - 60.0 * lat_odd + 0.5).floor();
    let mut rlat_even = (AIRBORNE_ZONE_DEG / 60.0) * (modulo(zone, 60.0) + lat_even);
    let mut rlat_odd = (AIRBORNE_ZONE_DEG / 59.0) * (modulo(zone, 59.0) + lat_odd);
    // Latitudes come out in [0, 360); the southern hemisphere is the upper quarter.
    if rlat_even >= 270.0 {
        rlat_even -= 360.0;
    }
    if rlat_odd >= 270.0 {
        rlat_odd -= 360.0;
    }
    if !(-90.0..=90.0).contains(&rlat_even) || !(-90.0..=90.0).contains(&rlat_odd) {
        return None;
    }
    let nl = cpr_nl(rlat_even);
    // Straddling a zone boundary makes the pair inconsistent: wait for the next frame.
    if nl != cpr_nl(rlat_odd) {
        return None;
    }

    let (lat, lon_cpr, i) = if latest_odd {
        (rlat_odd, lon_odd, 1)
    } else {
        (rlat_even, lon_even, 0)
    };
    let ni = f64::from((nl - i).max(1));
    let m = (lon_even * f64::from(nl - 1) - lon_odd * f64::from(nl) + 0.5).floor();
    let dlon = AIRBORNE_ZONE_DEG / ni;
    let mut lon = dlon * (modulo(m, ni) + lon_cpr);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

/// Local CPR: one frame plus a reference position within half a zone (±3° airborne,
/// ±0.75° on the surface) of the aircraft.
fn cpr_local(fix: &CprFix, odd: bool, ref_lat: f64, ref_lon: f64, zone: f64) -> Option<(f64, f64)> {
    let i = i32::from(odd);
    let lat_cpr = f64::from(fix.lat) / CPR_SCALE;
    let lon_cpr = f64::from(fix.lon) / CPR_SCALE;

    let dlat = zone / f64::from(60 - i);
    let j = (ref_lat / dlat).floor() + (modulo(ref_lat, dlat) / dlat - lat_cpr + 0.5).floor();
    let lat = dlat * (j + lat_cpr);
    if !(-90.0..=90.0).contains(&lat) {
        return None;
    }

    let ni = f64::from((cpr_nl(lat) - i).max(1));
    let dlon = zone / ni;
    let m = (ref_lon / dlon).floor() + (modulo(ref_lon, dlon) / dlon - lon_cpr + 0.5).floor();
    let mut lon = dlon * (m + lon_cpr);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

impl AdsbChannel {
    /// Cache slot for `icao`, evicting the least recently heard aircraft when full.
    fn slot(&mut self, icao: u32, at: u64) -> Option<&mut Aircraft> {
        if let Some(index) = self.cpr.iter().position(|a| a.icao == icao) {
            return self.cpr.get_mut(index);
        }
        if self.cpr.len() < CPR_CACHE_LEN {
            self.cpr.push(Aircraft::new(icao, at));
            return self.cpr.last_mut();
        }
        let oldest = self
            .cpr
            .iter()
            .enumerate()
            .min_by_key(|(_, a)| a.last)
            .map(|(index, _)| index)?;
        let entry = self.cpr.get_mut(oldest)?;
        *entry = Aircraft::new(icao, at);
        Some(entry)
    }

    /// Record an airborne CPR frame and solve the position when it completes a fresh pair.
    fn pair(&mut self, icao: u32, fix: CprFix, odd: bool) -> Option<(f64, f64)> {
        let entry = self.slot(icao, fix.at)?;
        entry.last = fix.at;
        if odd {
            entry.odd = Some(fix);
        } else {
            entry.even = Some(fix);
        }
        let (Some(even), Some(odd_fix)) = (entry.even, entry.odd) else {
            return None;
        };
        (even.at.abs_diff(odd_fix.at) <= CPR_PAIR_MAX_AGE_SAMPLES)
            .then(|| cpr_global(&even, &odd_fix, odd))
            .flatten()
    }

    fn fill_position(
        &mut self,
        frame: &[u8],
        icao: u32,
        at: u64,
        zone: f64,
        msg: &mut AdsbMessage,
    ) {
        let odd = bits_be(frame, ME_OFFSET_BITS + 21, 1) == 1;
        let fix = CprFix {
            lat: bits_be(frame, ME_OFFSET_BITS + 22, 17) as u32,
            lon: bits_be(frame, ME_OFFSET_BITS + 39, 17) as u32,
            at,
        };
        let global = (zone == AIRBORNE_ZONE_DEG)
            .then(|| self.pair(icao, fix, odd))
            .flatten();
        let solved = global.or_else(|| {
            self.reference
                .and_then(|(lat, lon)| cpr_local(&fix, odd, lat, lon, zone))
        });
        if let Some((lat, lon)) = solved {
            msg.lat = Some(lat);
            msg.lon = Some(lon);
        }
    }

    fn message(&mut self, frame: &[u8], df: u8, at: u64) -> AdsbMessage {
        let icao = bits_be(frame, ICAO_OFFSET_BITS, 24) as u32;
        let type_code = bits_be(frame, ME_OFFSET_BITS, 5) as u8;
        let mut msg = AdsbMessage {
            icao: format!("{icao:06X}"),
            df,
            type_code: Some(type_code),
            raw: hex_upper(frame),
            ..AdsbMessage::default()
        };
        let altitude = || bits_be(frame, ME_OFFSET_BITS + 8, 12) as u32;
        match type_code {
            1..=4 => msg.callsign = callsign(frame),
            5..=8 => {
                msg.on_ground = Some(true);
                self.fill_position(frame, icao, at, SURFACE_ZONE_DEG, &mut msg);
            }
            9..=18 => {
                msg.on_ground = Some(false);
                msg.altitude_ft = barometric_altitude(altitude());
                self.fill_position(frame, icao, at, AIRBORNE_ZONE_DEG, &mut msg);
            }
            19 => velocity(frame, &mut msg),
            // The type code selects the altitude *source* (GNSS height above the ellipsoid
            // rather than barometric), not its encoding: the AC12 field is the same Q-bit /
            // Gillham code in feet. Reading it as metres is the mode-s.org interpretation
            // that dump1090, readsb, java-adsb and rs1090 all contradict — and 12 bits of
            // metres tops out at 13 435 ft, which cannot express the altitude of the
            // high-integrity GNSS traffic that emits these very type codes.
            20..=22 => {
                msg.on_ground = Some(false);
                msg.altitude_ft = barometric_altitude(altitude());
                self.fill_position(frame, icao, at, AIRBORNE_ZONE_DEG, &mut msg);
            }
            _ => {}
        }
        msg
    }

    /// Try to decode a frame starting at `at` in [`Self::mag`], returning the samples it
    /// consumed. `None` means "not a frame here" and the scan advances one sample.
    fn try_frame(&mut self, at: usize, out: &mut ChannelOutputs) -> Option<usize> {
        let window = self.mag.get(at..at + LONG_FRAME_SAMPLES)?;
        if !preamble_ok(window.first_chunk()?) {
            return None;
        }
        let mut frame = [0u8; LONG_BYTES];
        slice_bits(window.get(PREAMBLE_SAMPLES..)?, &mut frame);

        let long = frame.first()? >> 3 >= 16;
        let bytes = frame.get_mut(..if long { LONG_BYTES } else { SHORT_BYTES })?;
        if mode_s_syndrome(bytes) != 0 {
            // A flipped bit inside DF picks the wrong frame length, so the syndrome cannot
            // close over the right byte count: such frames are dropped, never mis-repaired.
            if !self.crc_fix || mode_s_fix_single_bit(bytes).is_none() {
                return None;
            }
        }
        let df = bytes.first()? >> 3;
        if df != 17 && df != 18 {
            return None;
        }
        let consumed = PREAMBLE_SAMPLES + bytes.len() * 8 * SAMPLES_PER_BIT;
        let message = self.message(bytes, df, self.stream_pos + at as u64);
        out.events.push(DecoderEvent::Adsb(message));
        Some(consumed)
    }
}

impl ChannelRx for AdsbChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            crc_fix: p.crc_fix,
            reference: p.ref_lat.zip(p.ref_lon),
            mag: Vec::new(),
            stream_pos: 0,
            cpr: Vec::with_capacity(CPR_CACHE_LEN),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.crc_fix = p.crc_fix;
        self.reference = p.ref_lat.zip(p.ref_lon);
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        // Steady-state cost per input sample: one magnitude (two multiplies, an add and a
        // square root — `norm()` would call `hypot`, an order of magnitude slower for
        // overflow safety this signal cannot need) plus the four-comparison preamble reject.
        self.mag
            .extend(iq.iter().map(|s| s.re.mul_add(s.re, s.im * s.im).sqrt()));

        let mut at = 0;
        while at + LONG_FRAME_SAMPLES <= self.mag.len() {
            at += self.try_frame(at, out).unwrap_or(1);
        }

        // Keep everything a frame could still start in. Only offsets with a full frame behind
        // them are scanned, and those are exactly the ones dropped here, so results never
        // depend on where the host cut the block — and no frame is emitted twice.
        let keep = self.mag.len().saturating_sub(LONG_FRAME_SAMPLES - 1);
        self.mag.drain(..keep);
        self.stream_pos += keep as u64;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{
            add_noise,
            adsb::{
                me_airborne_position, me_airborne_position_gnss, me_identification,
                me_surface_position, me_velocity, position_me_raw, squitter, transmission,
            },
        },
        testutil::settings,
    };

    /// Berlin: comfortably away from every NL zone boundary, so the decoder's table and the
    /// generator's closed-form NL cannot disagree for reasons unrelated to the test.
    const LAT: f64 = 52.257_2;
    const LON: f64 = 13.409_1;
    const LEVEL: f32 = 0.5;
    const GAP_US: f64 = 30.0;

    fn adsb_params(p: AdsbParams) -> ChannelSettings {
        settings(ChannelParams::Adsb(p))
    }

    fn channel(p: AdsbParams) -> AdsbChannel {
        AdsbChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            adsb_params(p),
        )
        .unwrap()
    }

    fn feed(chan: &mut AdsbChannel, iq: &[Complex<f32>], blocks: &[usize]) -> Vec<AdsbMessage> {
        let mut out = ChannelOutputs::default();
        let mut messages = Vec::new();
        let mut pos = 0;
        for len in blocks.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "adsb must not produce audio");
            for event in &out.events {
                let DecoderEvent::Adsb(m) = event else {
                    panic!("adsb channel emitted {}", event.kind())
                };
                messages.push(m.clone());
            }
            pos = end;
        }
        messages
    }

    fn decode(p: AdsbParams, frames: &[Vec<u8>]) -> Vec<AdsbMessage> {
        let iq = transmission(frames, GAP_US, LEVEL, INPUT_RATE_HZ);
        feed(&mut channel(p), &iq, &[4_096])
    }

    /// Frame body as published in the Mode S literature; parity is appended here.
    fn published(hex: &str) -> Vec<u8> {
        let mut frame: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        sdrmm_dsp::mode_s_append_parity(&mut frame);
        frame
    }

    fn only(messages: Vec<AdsbMessage>) -> AdsbMessage {
        assert_eq!(messages.len(), 1, "{messages:?}");
        messages.into_iter().next().unwrap()
    }

    #[test]
    fn identification_round_trips_through_the_air() {
        let frame = squitter(0x3C_6444, me_identification("DLH123"));
        let msg = only(decode(AdsbParams::default(), std::slice::from_ref(&frame)));
        assert_eq!(msg.df, 17);
        assert_eq!(msg.icao, "3C6444");
        assert_eq!(msg.type_code, Some(4));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
        assert_eq!(msg.raw, hex_upper(&frame));
        assert_eq!(msg.altitude_ft, None);
        assert_eq!(msg.lat, None);
    }

    /// The identification squitter every Mode S text quotes: ICAO 4840D6, callsign KLM1023.
    #[test]
    fn published_identification_frame_decodes() {
        let msg = only(decode(
            AdsbParams::default(),
            &[published("8D4840D6202CC371C32CE0")],
        ));
        assert_eq!(msg.icao, "4840D6");
        assert_eq!(msg.callsign.as_deref(), Some("KLM1023"));
    }

    /// The published even/odd pair for ICAO 40621D at 38 000 ft. A global solution is
    /// reported at the position of the *later* frame, and the literature quotes the pair
    /// with the even frame last: 52.25720 N, 3.91937 E. The aircraft moved ~1 km between the
    /// two transmissions, so the odd-last solution is a different (and equally correct) point.
    #[test]
    fn published_position_pair_solves_globally() {
        let even = published("8D40621D58C382D690C8AC");
        let odd = published("8D40621D58C386435CC412");

        let msgs = decode(AdsbParams::default(), &[odd.clone(), even.clone()]);
        assert_eq!(msgs.len(), 2);
        let [first, last] = &msgs[..] else {
            unreachable!()
        };
        assert_eq!(first.icao, "40621D");
        assert_eq!(first.type_code, Some(11));
        assert_eq!(first.altitude_ft, Some(38_000));
        // A single frame with no reference position cannot be placed.
        assert_eq!(first.lat, None);
        let (lat, lon) = (last.lat.unwrap(), last.lon.unwrap());
        assert!((lat - 52.257_202).abs() < 1e-4, "lat {lat}");
        assert!((lon - 3.919_37).abs() < 1e-4, "lon {lon}");

        // Same pair the other way round: the odd frame's own position, ~1 km further east.
        let msgs = decode(AdsbParams::default(), &[even, odd]);
        let solved = msgs.last().unwrap();
        assert!((solved.lat.unwrap() - 52.265_78).abs() < 1e-4, "{solved:?}");
        assert!((solved.lon.unwrap() - 3.938_91).abs() < 1e-3, "{solved:?}");
    }

    #[test]
    fn airborne_position_pair_solves_to_the_encoded_point() {
        let icao = 0x3C_6444;
        let msgs = decode(
            AdsbParams::default(),
            &[
                squitter(icao, me_airborne_position(36_000, LAT, LON, false)),
                squitter(icao, me_airborne_position(36_000, LAT, LON, true)),
            ],
        );
        assert_eq!(msgs.len(), 2);
        let solved = msgs.last().unwrap();
        assert_eq!(solved.altitude_ft, Some(36_000));
        assert_eq!(solved.on_ground, Some(false));
        let (lat, lon) = (solved.lat.unwrap(), solved.lon.unwrap());
        assert!((lat - LAT).abs() < 0.01, "lat {lat}");
        assert!((lon - LON).abs() < 0.01, "lon {lon}");
    }

    #[test]
    fn a_single_frame_is_placed_against_a_reference_position() {
        for odd in [false, true] {
            let msg = only(decode(
                AdsbParams {
                    ref_lat: Some(LAT - 0.4),
                    ref_lon: Some(LON + 0.6),
                    ..AdsbParams::default()
                },
                &[squitter(
                    0x3C_6444,
                    me_airborne_position(9_000, LAT, LON, odd),
                )],
            ));
            let (lat, lon) = (msg.lat.unwrap(), msg.lon.unwrap());
            assert!((lat - LAT).abs() < 0.01, "odd {odd}: lat {lat}");
            assert!((lon - LON).abs() < 0.01, "odd {odd}: lon {lon}");
        }
    }

    /// Southern/western coordinates are where a `%`-based CPR gets the sign wrong.
    #[test]
    fn positions_south_and_west_of_the_meridian_solve() {
        let icao = 0xE8_0000;
        let (lat, lon) = (-33.868_2, -70.652_7);
        let msgs = decode(
            AdsbParams::default(),
            &[
                squitter(icao, me_airborne_position(12_000, lat, lon, false)),
                squitter(icao, me_airborne_position(12_000, lat, lon, true)),
            ],
        );
        let solved = msgs.last().unwrap();
        assert!((solved.lat.unwrap() - lat).abs() < 0.01, "{solved:?}");
        assert!((solved.lon.unwrap() - lon).abs() < 0.01, "{solved:?}");
    }

    #[test]
    fn surface_position_needs_a_reference_and_reports_on_ground() {
        let frame = squitter(0x3C_6444, me_surface_position(LAT, LON, false));
        let without = only(decode(AdsbParams::default(), std::slice::from_ref(&frame)));
        assert_eq!(without.on_ground, Some(true));
        assert_eq!(without.altitude_ft, None);
        assert_eq!(without.lat, None);

        let with = only(decode(
            AdsbParams {
                ref_lat: Some(LAT + 0.1),
                ref_lon: Some(LON - 0.1),
                ..AdsbParams::default()
            },
            &[frame],
        ));
        assert!((with.lat.unwrap() - LAT).abs() < 0.01, "{with:?}");
        assert!((with.lon.unwrap() - LON).abs() < 0.01, "{with:?}");
    }

    #[test]
    fn velocity_round_trips_speed_track_and_climb() {
        for (speed, track, climb) in [
            (420.0, 250.0, -1_408),
            (180.0, 5.0, 2_048),
            (500.0, 91.0, 0),
            (300.0, 359.5, -64),
        ] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(0x3C_6444, me_velocity(speed, track, climb))],
            ));
            assert_eq!(msg.type_code, Some(19));
            let got_speed = msg.ground_speed_kt.unwrap();
            let got_track = msg.track_deg.unwrap();
            assert!((got_speed - speed).abs() < 1.5, "speed {got_speed}");
            let error = (got_track - track + 540.0).rem_euclid(360.0) - 180.0;
            assert!(error.abs() < 0.5, "track {got_track}");
            assert_eq!(msg.vertical_rate_fpm, Some(climb));
        }
    }

    /// The Q bit switches the altitude encoding at 50 175 ft: 25 ft steps below, Gillham
    /// coded 100 ft steps above.
    #[test]
    fn altitude_decodes_on_both_sides_of_the_q_bit_boundary() {
        for altitude in [
            -1_000, 0, 725, 36_000, 50_175, 50_200, 51_000, 62_000, 80_000,
        ] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(
                    0x3C_6444,
                    me_airborne_position(altitude, LAT, LON, false),
                )],
            ));
            assert_eq!(msg.altitude_ft, Some(altitude), "altitude {altitude}");
        }
    }

    #[test]
    fn gnss_altitude_frames_use_the_same_ac12_encoding_as_barometric() {
        // The altitudes a GNSS-equipped airliner actually reports; a metre reading would
        // saturate its 12-bit field long before FL380.
        for alt_ft in [3_000, 38_000, 50_175] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(
                    0x3C_6444,
                    me_airborne_position_gnss(alt_ft, LAT, LON, false),
                )],
            ));
            assert_eq!(msg.type_code, Some(21));
            assert_eq!(msg.altitude_ft, Some(alt_ft), "{alt_ft} ft");
        }
    }

    /// AC12 == 0 is "altitude not available" for every airborne position frame, GNSS or not.
    #[test]
    fn absent_altitude_is_none_on_both_altitude_paths() {
        for tc in [11u64, 21] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(0x3C_6444, position_me_raw(tc, 0, LAT, LON, false))],
            ));
            assert_eq!(msg.altitude_ft, None, "tc {tc}");
        }
    }

    fn flip(frame: &mut [u8], bit: usize) {
        frame[bit / 8] ^= 0x80 >> (bit % 8);
    }

    #[test]
    fn crc_fix_repairs_a_single_bit_error() {
        let clean = squitter(0x3C_6444, me_identification("DLH123"));
        let mut damaged = clean.clone();
        flip(&mut damaged, 63);

        let msg = only(decode(AdsbParams::default(), &[damaged.clone()]));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
        assert_eq!(msg.raw, hex_upper(&clean));

        assert!(
            decode(
                AdsbParams {
                    crc_fix: false,
                    ..AdsbParams::default()
                },
                &[damaged]
            )
            .is_empty(),
            "crc_fix off must not repair anything"
        );
    }

    #[test]
    fn a_two_bit_error_is_dropped() {
        let mut damaged = squitter(0x3C_6444, me_identification("DLH123"));
        flip(&mut damaged, 40);
        flip(&mut damaged, 77);
        assert!(decode(AdsbParams::default(), &[damaged]).is_empty());
    }

    #[test]
    fn noise_alone_produces_no_frames() {
        let mut iq = vec![Complex::new(0.0f32, 0.0); 4_000_000];
        add_noise(&mut iq, 0x5EED_1234, 1.0);
        assert!(feed(&mut channel(AdsbParams::default()), &iq, &[65_536]).is_empty());
    }

    /// The accept threshold is derived from the pulses, so a frame 34 dB weaker than the one
    /// every other test uses must decode just as well — with noise under it.
    #[test]
    fn a_weak_frame_buried_in_noise_still_decodes() {
        let frame = squitter(0x3C_6444, me_identification("DLH123"));
        let mut iq = transmission(&[frame], GAP_US, 0.01, INPUT_RATE_HZ);
        add_noise(&mut iq, 0x00C0_FFEE, 0.001);
        let msg = only(feed(&mut channel(AdsbParams::default()), &iq, &[1_024]));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
    }

    #[test]
    fn ragged_blocks_decode_exactly_like_one_block() {
        let icao = 0x3C_6444;
        let frames = [
            squitter(icao, me_identification("DLH123")),
            squitter(icao, me_airborne_position(36_000, LAT, LON, false)),
            squitter(icao, me_velocity(420.0, 250.0, -1_408)),
            squitter(icao, me_airborne_position(36_000, LAT, LON, true)),
        ];
        let iq = transmission(&frames, GAP_US, LEVEL, INPUT_RATE_HZ);

        let whole = feed(&mut channel(AdsbParams::default()), &iq, &[iq.len()]);
        assert_eq!(whole.len(), 4);
        let ragged = feed(
            &mut channel(AdsbParams::default()),
            &iq,
            &[997, 1, 4_096, 65, 239, 7, 1_024],
        );
        assert_eq!(whole, ragged);
    }

    #[test]
    fn a_frame_split_across_two_calls_is_still_decoded() {
        let iq = transmission(
            &[squitter(0x3C_6444, me_identification("DLH123"))],
            GAP_US,
            LEVEL,
            INPUT_RATE_HZ,
        );
        // Cut inside the frame: the preamble is in the first block, most bits in the second.
        let cut = (GAP_US * 2.0) as usize + 40;
        let mut chan = channel(AdsbParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq[..cut], &mut out);
        assert!(out.events.is_empty());
        chan.process(&iq[cut..], &mut out);
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn the_cpr_cache_is_bounded() {
        let frames: Vec<Vec<u8>> = (0..CPR_CACHE_LEN as u32 * 3)
            .map(|n| squitter(0x40_0000 + n, me_airborne_position(30_000, LAT, LON, false)))
            .collect();
        let mut chan = channel(AdsbParams::default());
        let iq = transmission(&frames, GAP_US, LEVEL, INPUT_RATE_HZ);
        assert_eq!(feed(&mut chan, &iq, &[8_192]).len(), frames.len());
        assert_eq!(chan.cpr.len(), CPR_CACHE_LEN);
    }

    /// An even/odd pair that arrived minutes apart describes two different places; the
    /// aircraft has flown between them, so the pair must not be solved.
    #[test]
    fn a_stale_even_odd_pair_is_not_paired() {
        let mut chan = channel(AdsbParams::default());
        let even = published("8D40621D58C382D690C8AC");
        let odd = published("8D40621D58C386435CC412");
        assert!(chan.message(&even, 17, 0).lat.is_none());
        assert!(
            chan.message(&odd, 17, CPR_PAIR_MAX_AGE_SAMPLES + 1)
                .lat
                .is_none()
        );
        // Fresh again once a new even frame arrives.
        assert!(
            chan.message(&even, 17, CPR_PAIR_MAX_AGE_SAMPLES + 2)
                .lat
                .is_some()
        );
        assert_eq!(chan.cpr.first().map(|a| a.icao), Some(0x40_621D));
    }

    /// The NL table is 58 hand-typed constants; check every one of them against the closed
    /// form it tabulates (DO-260B Appendix A), away from the boundaries themselves.
    #[test]
    fn the_nl_table_matches_its_closed_form() {
        let closed_form = |lat: f64| -> i32 {
            let nz = 15.0;
            let a = 1.0 - (1.0 - (PI / (2.0 * nz)).cos()) / (PI * lat / 180.0).cos().powi(2);
            (TAU / a.acos()).floor() as i32
        };
        let mut lat = 0.0;
        while lat < 86.5 {
            let near_boundary = NL_BOUNDARIES.iter().any(|b| (b - lat).abs() < 1e-4);
            if !near_boundary {
                assert_eq!(cpr_nl(lat), closed_form(lat), "lat {lat}");
                assert_eq!(cpr_nl(-lat), closed_form(lat), "lat -{lat}");
            }
            lat += 0.013;
        }
        assert_eq!(cpr_nl(88.0), 1);
        assert_eq!(cpr_nl(-89.9), 1);
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AdsbParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = AdsbChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AdsbChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            adsb_params(AdsbParams::default()),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn an_incomplete_or_out_of_range_reference_is_rejected() {
        for bad in [
            AdsbParams {
                ref_lat: Some(91.0),
                ref_lon: Some(0.0),
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(0.0),
                ref_lon: Some(181.0),
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(0.0),
                ref_lon: None,
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(f64::NAN),
                ref_lon: Some(0.0),
                ..AdsbParams::default()
            },
        ] {
            let built = AdsbChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                adsb_params(bad.clone()),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "{bad:?} must be rejected"
            );
        }
    }
}
