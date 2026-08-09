//! AIS reference modulator (PLAN §14): ITU-R M.1371 bursts as 9600 bit/s GMSK at complex
//! baseband, built from the spec rather than from the decoder's constants so a wrong constant
//! cannot cancel out between the two.

use num_complex::Complex;
use sdrmm_dsp::{RealDecimator, crc16_x25, design_gaussian, pack_lsb};

use super::fm_modulate;

/// ITU-R M.1371 Annex 2 §2.2: 9600 bit/s, ±2400 Hz deviation (modulation index 0.5), BT 0.4.
const BAUD: f64 = 9_600.0;
const DEVIATION_HZ: f64 = 2_400.0;
const BT: f64 = 0.4;
/// Gaussian pulse truncation, in symbol periods either side.
const SHAPING_SPAN: usize = 4;

const FLAG: [bool; 8] = [false, true, true, true, true, true, true, false];
/// ITU-R M.1371 Annex 2 §3.3.7.2 training sequence: 24 bits of alternating line level. Zero
/// data bits produce exactly that, because NRZI toggles the line on every zero.
const TRAINING_BITS: usize = 24;
/// The spec's trailing buffer field, which also gives the shaping filter's group delay the
/// room it needs to finish the closing flag.
const BUFFER_BITS: usize = 24;

/// Degrees to the 1/10 000 minute units AIS transmits positions in.
const COORD_UNITS_PER_DEGREE: f64 = 600_000.0;

/// The subset of a type 1/2/3 or type 18 position report a test cares about; every other
/// field is transmitted as its "not available" code.
#[derive(Clone, Copy, Debug)]
pub struct PositionReport {
    pub mmsi: u32,
    pub lat: f64,
    pub lon: f64,
    pub sog_kt: f64,
    pub cog_deg: f64,
    pub heading_deg: u16,
    pub nav_status: u8,
}

/// Payload bits for a type 1 position report.
#[must_use]
pub fn position_payload(report: &PositionReport) -> Vec<bool> {
    let mut b = Bits::default();
    b.field(1, 6);
    b.field(0, 2);
    b.field(u64::from(report.mmsi), 30);
    b.field(u64::from(report.nav_status & 0xF), 4);
    // Rate of turn: −128 is "not available".
    b.signed(-128, 8);
    b.field(sog_code(report.sog_kt), 10);
    b.field(0, 1);
    b.signed(coord_code(report.lon), 28);
    b.signed(coord_code(report.lat), 27);
    b.field(cog_code(report.cog_deg), 12);
    b.field(u64::from(report.heading_deg.min(511)), 9);
    // Time stamp 60 = "not available".
    b.field(60, 6);
    b.field(0, 2);
    b.field(0, 3);
    b.field(0, 1);
    b.field(0, 19);
    debug_assert_eq!(b.0.len(), 168);
    b.0
}

/// Payload bits for a type 18 class B position report.
#[must_use]
pub fn class_b_payload(report: &PositionReport) -> Vec<bool> {
    let mut b = Bits::default();
    b.field(18, 6);
    b.field(0, 2);
    b.field(u64::from(report.mmsi), 30);
    b.field(0, 8);
    b.field(sog_code(report.sog_kt), 10);
    b.field(0, 1);
    b.signed(coord_code(report.lon), 28);
    b.signed(coord_code(report.lat), 27);
    b.field(cog_code(report.cog_deg), 12);
    b.field(u64::from(report.heading_deg.min(511)), 9);
    b.field(60, 6);
    b.field(0, 2);
    // CS unit, display, DSC, band, message 22, assigned mode and RAIM flags.
    b.field(0, 7);
    b.field(0, 20);
    debug_assert_eq!(b.0.len(), 168);
    b.0
}

/// Payload bits for a type 5 static and voyage related data report.
#[must_use]
pub fn static_payload(mmsi: u32, name: &str, call_sign: &str, destination: &str) -> Vec<bool> {
    let mut b = Bits::default();
    b.field(5, 6);
    b.field(0, 2);
    b.field(u64::from(mmsi), 30);
    b.field(0, 2);
    b.field(0, 30);
    b.text(call_sign, 7);
    b.text(name, 20);
    b.field(0, 8);
    // Dimensions to bow / stern / port / starboard.
    b.field(0, 9);
    b.field(0, 9);
    b.field(0, 6);
    b.field(0, 6);
    b.field(0, 4);
    // ETA month / day / hour / minute, all "not available".
    b.field(0, 4);
    b.field(0, 5);
    b.field(24, 5);
    b.field(60, 6);
    b.field(0, 8);
    b.text(destination, 20);
    b.field(0, 1);
    b.field(0, 1);
    debug_assert_eq!(b.0.len(), 424);
    b.0
}

/// Payload bits for the two halves of a type 24 static data report: part A carries the name,
/// part B the call sign. A class B transmitter sends them as a pair.
#[must_use]
pub fn static_data_payloads(mmsi: u32, name: &str, call_sign: &str) -> (Vec<bool>, Vec<bool>) {
    let mut a = Bits::default();
    a.field(24, 6);
    a.field(0, 2);
    a.field(u64::from(mmsi), 30);
    a.field(0, 2);
    a.text(name, 20);
    a.field(0, 8);
    debug_assert_eq!(a.0.len(), 168);

    let mut b = Bits::default();
    b.field(24, 6);
    b.field(0, 2);
    b.field(u64::from(mmsi), 30);
    b.field(1, 2);
    b.field(0, 8);
    // Vendor id, unit model code and serial number.
    b.field(0, 18);
    b.field(0, 4);
    b.field(0, 20);
    b.text(call_sign, 7);
    // Dimensions to bow / stern / port / starboard, then spare.
    b.field(0, 9);
    b.field(0, 9);
    b.field(0, 6);
    b.field(0, 6);
    b.field(0, 6);
    debug_assert_eq!(b.0.len(), 168);

    (a.0, b.0)
}

/// Wrap payload bits into a full HDLC burst (preamble, flags, CRC, stuffing, NRZI) and
/// GMSK-modulate to complex baseband IQ at `rate`.
///
/// # Panics
/// If `payload` is not a whole number of octets, or `rate` gives under two samples per bit.
#[must_use]
pub fn burst(payload: &[bool], rate: f64) -> Vec<Complex<f32>> {
    modulate(&with_fcs(payload), rate)
}

/// [`burst`], with frame bit `flip` inverted after the FCS has been computed over the clean
/// payload — a burst that frames perfectly and fails its CRC by exactly one bit.
///
/// # Panics
/// If `flip` is past the end of the payload and its FCS, or as [`burst`] panics.
#[must_use]
pub fn corrupted_burst(payload: &[bool], flip: usize, rate: f64) -> Vec<Complex<f32>> {
    let mut framed = with_fcs(payload);
    assert!(
        flip < framed.len(),
        "bit {flip} is past the {}-bit frame",
        framed.len()
    );
    framed[flip] = !framed[flip];
    modulate(&framed, rate)
}

fn modulate(framed: &[bool], rate: f64) -> Vec<Complex<f32>> {
    assert!(
        framed.len().is_multiple_of(8),
        "an HDLC frame carries whole octets"
    );
    let sps = rate / BAUD;
    assert!(
        sps >= 2.0,
        "need at least two samples per bit at {BAUD} baud"
    );

    let mut bits = vec![false; TRAINING_BITS];
    bits.extend(FLAG);
    bits.extend(stuff(framed));
    bits.extend(FLAG);
    bits.extend(std::iter::repeat_n(false, BUFFER_BITS));

    let nrz = upsample(&nrzi_encode(&bits), sps);
    let mut shaped = Vec::with_capacity(nrz.len());
    RealDecimator::new(&design_gaussian(sps, BT, SHAPING_SPAN), 1).process(&nrz, &mut shaped);
    fm_modulate(&shaped, DEVIATION_HZ, rate)
}

/// Bit writer for the big-endian fields an AIS message is defined in.
#[derive(Default)]
struct Bits(Vec<bool>);

impl Bits {
    fn field(&mut self, value: u64, len: u32) {
        for k in (0..len).rev() {
            self.0.push(value >> k & 1 == 1);
        }
    }

    fn signed(&mut self, value: i64, len: u32) {
        let mask = (1u64 << len) - 1;
        self.field(value as u64 & mask, len);
    }

    /// Six-bit ASCII, padded to `chars` with `@` (the alphabet's NUL).
    fn text(&mut self, s: &str, chars: usize) {
        let mut written = 0;
        for c in s.chars().take(chars) {
            self.field(u64::from(six_bit(c)), 6);
            written += 1;
        }
        for _ in written..chars {
            self.field(0, 6);
        }
    }
}

/// Inverse of the AIS six-bit alphabet: 0–31 are `@`–`_`, 32–63 are space–`?`.
fn six_bit(c: char) -> u8 {
    match c.to_ascii_uppercase() as u32 {
        v @ 64..=95 => (v - 64) as u8,
        v @ 32..=63 => v as u8,
        _ => 0,
    }
}

fn coord_code(deg: f64) -> i64 {
    (deg * COORD_UNITS_PER_DEGREE).round() as i64
}

/// 0.1 kt units; 1023 is "not available".
fn sog_code(kt: f64) -> u64 {
    ((kt * 10.0).round() as i64).clamp(0, 1023) as u64
}

/// 0.1° units; 3600 is "not available".
fn cog_code(deg: f64) -> u64 {
    ((deg * 10.0).round() as i64).clamp(0, 3600) as u64
}

/// Append the HDLC frame check sequence: CRC-16/X-25 over the wire bits packed LSB-first into
/// octets, sent low octet first with each octet's low bit first.
fn with_fcs(payload: &[bool]) -> Vec<bool> {
    let crc = crc16_x25(&pack_lsb(payload));
    let mut out = payload.to_vec();
    for k in 0..16 {
        out.push(crc >> k & 1 == 1);
    }
    out
}

fn stuff(bits: &[bool]) -> Vec<bool> {
    let mut out = Vec::with_capacity(bits.len() + bits.len() / 5);
    let mut ones = 0;
    for &bit in bits {
        out.push(bit);
        ones = if bit { ones + 1 } else { 0 };
        if ones == 5 {
            out.push(false);
            ones = 0;
        }
    }
    out
}

/// NRZI line encode: a 0 bit toggles the line, a 1 holds it (the inverse of
/// [`sdrmm_dsp::NrziDecoder`], which starts from the same low idle level).
fn nrzi_encode(bits: &[bool]) -> Vec<bool> {
    let mut level = false;
    bits.iter()
        .map(|&b| {
            if !b {
                level = !level;
            }
            level
        })
        .collect()
}

fn upsample(line: &[bool], sps: f64) -> Vec<f32> {
    (0..(line.len() as f64 * sps) as usize)
        .map(|k| {
            let i = ((k as f64 / sps) as usize).min(line.len() - 1);
            if line[i] { 1.0 } else { -1.0 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::hdlc_fcs_ok;

    use super::*;

    fn report() -> PositionReport {
        PositionReport {
            mmsi: 244_670_316,
            lat: 52.372_5,
            lon: 4.893_2,
            sog_kt: 12.4,
            cog_deg: 187.3,
            heading_deg: 188,
            nav_status: 0,
        }
    }

    #[test]
    fn payload_lengths_match_the_message_definitions() {
        assert_eq!(position_payload(&report()).len(), 168);
        assert_eq!(class_b_payload(&report()).len(), 168);
        assert_eq!(static_payload(1, "A", "B", "C").len(), 424);
        let (a, b) = static_data_payloads(1, "A", "B");
        assert_eq!((a.len(), b.len()), (168, 168));
    }

    #[test]
    fn fcs_closes_the_frame_the_deframer_will_check() {
        let framed = with_fcs(&position_payload(&report()));
        assert!(hdlc_fcs_ok(&pack_lsb(&framed)));
    }

    #[test]
    fn stuffing_breaks_every_run_of_five_ones() {
        let stuffed = stuff(&[true; 17]);
        assert_eq!(
            stuffed,
            [
                true, true, true, true, true, false, true, true, true, true, true, false, true,
                true, true, true, true, false, true, true
            ]
        );
    }

    #[test]
    fn burst_is_constant_envelope_and_the_expected_length() {
        let payload = position_payload(&report());
        let iq = burst(&payload, 48_000.0);
        // 24 training + 8 flag + 184 stuffed data + 8 flag + 24 buffer bits, at 5 samples
        // per bit; the exact stuffed count depends on the payload, so bound it instead.
        assert!(
            (1_240..1_400).contains(&iq.len()),
            "burst length {}",
            iq.len()
        );
        for (k, s) in iq.iter().enumerate() {
            assert!(
                (s.norm() - 1.0).abs() < 1e-3,
                "sample {k} magnitude {}",
                s.norm()
            );
        }
    }
}
