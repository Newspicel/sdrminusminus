//! AX.25 reference modulator (PLAN §14): a UI frame down to Bell 202 AFSK1200 or 9600 baud
//! G3RUH baseband IQ.
//!
//! The physical constants come from the decoder module itself, so a modulator and its
//! demodulator cannot drift apart.

use num_complex::Complex;
use sdrmm_dsp::{Scrambler, crc16_x25};

use super::{afsk_audio, fm_modulate, fsk};
use crate::aprs::{
    ADDRESS_LEN, AFSK_BAUD, AFSK_MARK_HZ, AFSK_SPACE_HZ, CONTROL_UI, DEVIATION_HZ, G3RUH_BAUD,
    PID_NO_LAYER3,
};

/// Frame delimiter and idle pattern (AX.25 2.2 §3.6).
const FLAG: u8 = 0x7E;
/// TXDELAY: flags keyed before the first frame, long enough for the receiver's clock recovery
/// to lock and (at 9600 baud) for the descrambler to fill.
const PREAMBLE_FLAGS: usize = 24;
/// Flags keyed after the last frame; one of them closes it.
const TRAILING_FLAGS: usize = 3;
/// Reserved bits of an SSID octet, transmitted as ones (AX.25 2.2 §3.12.2).
const SSID_RESERVED: u8 = 0x60;

/// Transmitted frame check sequence. Every real signal uses [`Fcs::Valid`]; [`Fcs::Corrupt`]
/// lets a test exercise the receiver's FCS rejection without depending on where a flipped
/// channel bit happens to land.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fcs {
    #[default]
    Valid,
    Corrupt,
}

/// Build a UI frame's bytes (addresses, control, PID, info) without the FCS.
///
/// A path entry ending in `*` is transmitted with its "has been repeated" bit set.
#[must_use]
pub fn ui_frame(source: &str, destination: &str, path: &[&str], info: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(ADDRESS_LEN * (2 + path.len()) + 2 + info.len());
    // AX.25 2.2 §6.1.2: a command frame carries C=1 in the destination SSID octet and C=0 in
    // the source's, which is what every APRS transmitter sends.
    push_address(&mut out, destination, true, false);
    push_address(&mut out, source, false, path.is_empty());
    for (i, hop) in path.iter().enumerate() {
        let repeated = hop.ends_with('*');
        push_address(
            &mut out,
            hop.trim_end_matches('*'),
            repeated,
            i + 1 == path.len(),
        );
    }
    out.push(CONTROL_UI);
    out.push(PID_NO_LAYER3);
    out.extend_from_slice(info.as_bytes());
    out
}

/// Wrap bytes into an HDLC burst (flags, FCS, stuffing, NRZI) and modulate it as Bell 202
/// AFSK through an FM transmitter.
#[must_use]
pub fn afsk1200(frame: &[u8], rate: f64) -> Vec<Complex<f32>> {
    afsk1200_frames(&[frame], Fcs::Valid, rate)
}

/// [`afsk1200`] over several frames in one burst: adjacent frames share a single flag, as a
/// transmitter draining its queue sends them.
#[must_use]
pub fn afsk1200_frames(frames: &[&[u8]], fcs: Fcs, rate: f64) -> Vec<Complex<f32>> {
    let levels = burst_levels(frames, fcs);
    let audio = afsk_audio(&levels, AFSK_BAUD, AFSK_MARK_HZ, AFSK_SPACE_HZ, rate);
    fm_modulate(&audio, DEVIATION_HZ, rate)
}

/// Wrap bytes into an HDLC burst and modulate it as 9600 baud G3RUH: the NRZI line is
/// scrambled and keyed straight onto the carrier as two-level FSK.
#[must_use]
pub fn g3ruh9600(frame: &[u8], rate: f64) -> Vec<Complex<f32>> {
    g3ruh9600_frames(&[frame], Fcs::Valid, rate)
}

/// [`g3ruh9600`] over several frames in one burst.
#[must_use]
pub fn g3ruh9600_frames(frames: &[&[u8]], fcs: Fcs, rate: f64) -> Vec<Complex<f32>> {
    let mut scrambler = Scrambler::g3ruh();
    let scrambled: Vec<bool> = burst_levels(frames, fcs)
        .into_iter()
        .map(|level| scrambler.push(level))
        .collect();
    fsk(&scrambled, G3RUH_BAUD, DEVIATION_HZ, rate)
}

fn push_address(out: &mut Vec<u8>, call: &str, command: bool, last: bool) {
    let (base, ssid) = match call.split_once('-') {
        Some((base, ssid)) => (base, ssid.parse::<u8>().unwrap_or(0)),
        None => (call, 0),
    };
    let mut chars = [b' '; ADDRESS_LEN - 1];
    for (slot, byte) in chars.iter_mut().zip(base.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    for c in chars {
        // Callsign characters occupy bits 1..7; bit 0 is the address extension bit.
        out.push((c & 0x7F) << 1);
    }
    out.push(u8::from(command) << 7 | SSID_RESERVED | (ssid & 0x0F) << 1 | u8::from(last));
}

/// The burst as NRZI line levels: preamble flags, then each frame's bytes and FCS with zero
/// stuffing applied, frames separated by one shared flag, then trailing flags.
fn burst_levels(frames: &[&[u8]], fcs: Fcs) -> Vec<bool> {
    let mut bits = Vec::new();
    for _ in 0..PREAMBLE_FLAGS {
        push_flag(&mut bits);
    }
    for frame in frames {
        let checksum = fcs_bytes(frame, fcs);
        let mut ones = 0u8;
        for &byte in frame.iter().chain(checksum.iter()) {
            for i in 0..8 {
                push_stuffed(&mut bits, byte >> i & 1 == 1, &mut ones);
            }
        }
        push_flag(&mut bits);
    }
    for _ in 1..TRAILING_FLAGS {
        push_flag(&mut bits);
    }
    nrzi(&bits)
}

/// AX.25 2.2 §4.4.6: the FCS is CRC-16/X.25 over the frame, transmitted little-endian.
fn fcs_bytes(frame: &[u8], fcs: Fcs) -> [u8; 2] {
    let error = match fcs {
        Fcs::Valid => 0,
        Fcs::Corrupt => 1,
    };
    (crc16_x25(frame) ^ error).to_le_bytes()
}

/// Flags are transmitted verbatim — the one bit pattern the stuffing rule must not touch.
fn push_flag(bits: &mut Vec<bool>) {
    for i in 0..8 {
        bits.push(FLAG >> i & 1 == 1);
    }
}

fn push_stuffed(bits: &mut Vec<bool>, bit: bool, ones: &mut u8) {
    bits.push(bit);
    *ones = if bit { *ones + 1 } else { 0 };
    if *ones == 5 {
        bits.push(false);
        *ones = 0;
    }
}

/// NRZI: a 0 bit toggles the line, a 1 holds it (AX.25 2.2 §3.5).
fn nrzi(bits: &[bool]) -> Vec<bool> {
    let mut level = false;
    bits.iter()
        .map(|&bit| {
            if !bit {
                level = !level;
            }
            level
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::hdlc_fcs_ok;

    use super::*;

    #[test]
    fn ui_frame_lays_out_the_ax25_address_field() {
        let frame = ui_frame("DL1ABC-9", "APRS", &["WIDE1-1*"], "!test");
        // Destination, source, one digipeater, control, PID, then the information field.
        assert_eq!(frame.len(), 3 * ADDRESS_LEN + 2 + 5);
        assert_eq!(&frame[..6], b"APRS  ".map(|c| c << 1));
        // Only the last address entry sets the extension bit.
        assert_eq!(frame[6] & 1, 0);
        assert_eq!(frame[13] & 1, 0);
        assert_eq!(frame[20] & 1, 1);
        // Source SSID 9, no repeated flag; digipeater SSID 1 with the repeated flag.
        assert_eq!(frame[13] >> 1 & 0x0F, 9);
        assert_eq!(frame[13] & 0x80, 0);
        assert_eq!(frame[20] >> 1 & 0x0F, 1);
        assert_eq!(frame[20] & 0x80, 0x80);
        assert_eq!(&frame[21..], b"\x03\xf0!test");
    }

    #[test]
    fn corrupting_the_fcs_is_the_only_difference_between_the_two_bursts() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "!x");
        assert!(hdlc_fcs_ok(
            &[frame.clone(), fcs_bytes(&frame, Fcs::Valid).to_vec()].concat()
        ));
        assert!(!hdlc_fcs_ok(
            &[frame.clone(), fcs_bytes(&frame, Fcs::Corrupt).to_vec()].concat()
        ));
    }

    #[test]
    fn stuffing_breaks_every_run_of_five_ones() {
        let mut bits = Vec::new();
        let mut ones = 0;
        for _ in 0..4 {
            push_stuffed(&mut bits, true, &mut ones);
            push_stuffed(&mut bits, true, &mut ones);
            push_stuffed(&mut bits, true, &mut ones);
        }
        let longest = bits
            .chunk_by(|a, b| a == b)
            .filter(|run| run[0])
            .map(<[bool]>::len)
            .max()
            .unwrap();
        assert_eq!(longest, 5);
    }

    #[test]
    fn the_burst_carries_the_requested_number_of_symbols() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "!x");
        let levels = burst_levels(&[&frame], Fcs::Valid);
        let iq = afsk1200(&frame, 48_000.0);
        assert_eq!(iq.len(), levels.len() * 40);
        for s in &iq {
            assert!((s.norm() - 1.0).abs() < 1e-3, "magnitude {}", s.norm());
        }
        assert_eq!(g3ruh9600(&frame, 48_000.0).len(), levels.len() * 5);
    }
}
