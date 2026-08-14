use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_modem::{
    cpm::{CpmMod, CpmParams, Mapping},
    pulse::{self, Norm},
};

use super::{shift, silence};

/// Every burst ends with enough silence for a decoder to close its repeat-collapse window.
const TAIL_S: f64 = 0.7;
/// Lead-in silence, so an adaptive slicer has a noise floor before the first edge.
const LEAD_IN_S: f64 = 0.05;
/// Carrier offset within the channel. Deliberately not zero: these transmitters are never on
/// frequency, and a generator that pretended otherwise would not test the wide channel.
const CARRIER_OFFSET_HZ: f64 = 30_000.0;

/// A pulse-width-coded transmission of the PT2262 / EV1527 family.
#[derive(Clone, Debug)]
pub struct Pwm {
    pub bits: Vec<bool>,
    /// The base clock period, in µs.
    pub short_us: u32,
    /// Length of the long half of a bit cell, in base periods (3 for the usual remote).
    pub long_multiple: u32,
    /// The sync gap that separates repeats, in base periods.
    pub sync_gap_multiple: u32,
    pub repeats: u32,
}

/// Alternating pulse/gap durations in µs for one frame, ending with the sync pulse and its
/// long gap — the order a receiver actually sees.
#[must_use]
pub fn pwm_timings(frame: &Pwm) -> Vec<u32> {
    let short = frame.short_us;
    let long = short * frame.long_multiple;
    let mut out = Vec::with_capacity(frame.bits.len() * 2 + 2);
    for &bit in &frame.bits {
        if bit {
            out.extend_from_slice(&[long, short]);
        } else {
            out.extend_from_slice(&[short, long]);
        }
    }
    out.push(short);
    out.push(short * frame.sync_gap_multiple);
    out
}

/// Half-cell levels for a Manchester-coded payload: a 1 is high then low.
#[must_use]
pub fn manchester_cells(bits: &[bool]) -> Vec<bool> {
    bits.iter().flat_map(|&b| [b, !b]).collect()
}

/// Alternating pulse/gap durations in µs for a Manchester frame at `half_cell_us`.
#[must_use]
pub fn manchester_timings(bits: &[bool], half_cell_us: u32) -> Vec<u32> {
    let cells = manchester_cells(bits);
    let mut out = Vec::new();
    let mut level = true;
    let mut run = 0u32;
    for &cell in &cells {
        if cell == level {
            run += half_cell_us;
        } else {
            out.push(run);
            level = cell;
            run = half_cell_us;
        }
    }
    out.push(run);
    // A frame that starts low has no opening pulse; the receiver would drop the leading gap,
    // so the generator does too and the two agree on where the frame begins.
    if !cells.first().copied().unwrap_or(true) {
        out.remove(0);
    }
    out
}

/// Key an on/off envelope from alternating pulse/gap durations in µs, pulse first.
#[must_use]
pub fn envelope(timings_us: &[u32], rate: f64) -> Vec<f32> {
    let samples = |us: u32| (f64::from(us) * 1e-6 * rate).round() as usize;
    let mut out = vec![0.0; samples((LEAD_IN_S * 1e6) as u32)];
    let mut high = true;
    for &us in timings_us {
        out.extend(std::iter::repeat_n(
            if high { 1.0 } else { 0.0 },
            samples(us),
        ));
        high = !high;
    }
    out.extend(std::iter::repeat_n(0.0, samples((TAIL_S * 1e6) as u32)));
    out
}

/// On-off keying of the channel carrier by `timings_us`.
#[must_use]
pub fn keyed(timings_us: &[u32], rate: f64) -> Vec<Complex<f32>> {
    let step = TAU * CARRIER_OFFSET_HZ / rate;
    envelope(timings_us, rate)
        .iter()
        .enumerate()
        .map(|(k, &env)| Complex::from_polar(env, (step * k as f64) as f32))
        .collect()
}

/// An OOK transmission of `frame`, repeated as a remote repeats it.
#[must_use]
pub fn pwm(frame: &Pwm, rate: f64) -> Vec<Complex<f32>> {
    let one = pwm_timings(frame);
    let mut timings = Vec::new();
    for _ in 0..frame.repeats.max(1) {
        timings.extend_from_slice(&one);
    }
    keyed(&timings, rate)
}

/// A Manchester transmission repeated with a silent gap between copies.
#[must_use]
pub fn manchester(bits: &[bool], half_cell_us: u32, repeats: u32, rate: f64) -> Vec<Complex<f32>> {
    let one = manchester_timings(bits, half_cell_us);
    let mut timings = Vec::new();
    for i in 0..repeats.max(1) {
        if i > 0 {
            timings.push(20_000);
        }
        timings.extend_from_slice(&one);
    }
    keyed(&timings, rate)
}

#[must_use]
pub fn pwm_fsk(frame: &Pwm, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let one = pwm_timings(frame);
    let mut symbols = Vec::new();
    for _ in 0..frame.repeats.max(1) {
        let mut high = true;
        for &us in &one {
            symbols.extend(std::iter::repeat_n(
                u8::from(high),
                (us / frame.short_us) as usize,
            ));
            high = !high;
        }
    }
    let base_baud = 1e6 / f64::from(frame.short_us);
    let sps = rate / base_baud;
    let mut modulator = CpmMod::new(CpmParams::from_deviation(
        Mapping::new(vec![-1.0, 1.0]),
        deviation_hz,
        base_baud,
        pulse::rect(sps, Norm::Area),
        sps,
    ));
    let mut burst = Vec::new();
    modulator.modulate(&symbols, &mut burst);
    // No flush: a transmitter keying down truncates its last tone, and the frame's own
    // trailing sync gap is what the decoder times.
    shift(&mut burst, CARRIER_OFFSET_HZ, rate);
    let mut iq = silence((LEAD_IN_S * rate) as usize);
    iq.extend(burst);
    iq.extend(silence((TAIL_S * rate) as usize));
    iq
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Pwm {
        Pwm {
            bits: vec![true, false, true, true],
            short_us: 320,
            long_multiple: 3,
            sync_gap_multiple: 31,
            repeats: 2,
        }
    }

    #[test]
    fn a_bit_is_a_long_short_pair_the_right_way_round() {
        let timings = pwm_timings(&frame());
        assert_eq!(&timings[..8], &[960, 320, 320, 960, 960, 320, 960, 320]);
        assert_eq!(
            &timings[8..],
            &[320, 9_920],
            "the sync pulse and its gap close the frame"
        );
    }

    #[test]
    fn manchester_runs_are_one_or_two_half_cells() {
        let timings = manchester_timings(&[true, true, false, false, true], 250);
        assert!(timings.iter().all(|&d| d == 250 || d == 500), "{timings:?}");
        assert_eq!(timings, [250, 250, 250, 500, 250, 250, 500, 250]);
    }

    #[test]
    fn ook_keys_the_carrier_fully_off_between_pulses() {
        let iq = keyed(&[500, 500], 250_000.0);
        let lead = (LEAD_IN_S * 250_000.0) as usize;
        let pulse = iq[lead + 10].norm();
        let gap = iq[lead + (500.0 * 1e-6 * 250_000.0) as usize + 10].norm();
        assert!((pulse - 1.0).abs() < 1e-3, "pulse {pulse}");
        assert!(gap < 1e-6, "gap {gap}");
    }

    #[test]
    fn fsk_holds_the_carrier_through_the_frame_and_drops_it_after() {
        let iq = pwm_fsk(&frame(), 40_000.0, 250_000.0);
        let lead = (LEAD_IN_S * 250_000.0) as usize;
        assert!((iq[lead + 100].norm() - 1.0).abs() < 1e-3);
        assert!(iq[iq.len() - 100].norm() < 1e-6);
    }
}
