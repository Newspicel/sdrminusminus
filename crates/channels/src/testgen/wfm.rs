//! FM broadcast stereo reference modulator (PLAN §14): the pilot-tone multiplex a stereo
//! station transmits, per ITU-R BS.450 / 47 CFR §73.322.
//!
//! `MPX = 0.45·[(L+R)/2 + (L−R)/2·sub] + 0.09·pilot`, the deviation shares of a typical
//! broadcast multiplex — the same audio/pilot split [`super::rds`] uses, so one composite could
//! carry both. The pilot is written as a cosine to match that module; the subcarrier is then
//! `−sin(2ωt)`, which is what the standard's rule — pilot zero crossings coincide with
//! positive-going subcarrier zero crossings — comes to for a cosine pilot. Getting that sign
//! wrong swaps the channels rather than breaking them, which is exactly why it is generated
//! here from the rule rather than copied from the demodulator.
//!
//! No pre-emphasis: a receiver's de-emphasis then shows up as the plain analog roll-off at the
//! test tone's frequency, which is what the WFM tests measure against.

use std::f64::consts::TAU;

use num_complex::Complex;

use super::fm_modulate;

/// Stereo pilot; the difference subcarrier is its second harmonic.
const PILOT_HZ: f64 = 19_000.0;
/// Peak deviation of a fully modulated broadcast carrier; [`composite`] returns ±1 for it.
const DEVIATION_HZ: f64 = 75_000.0;
const AUDIO_LEVEL: f64 = 0.45;
const PILOT_LEVEL: f64 = 0.09;

/// The stereo multiplex for `left`/`right` (both in ±1, same length) as a real signal in ±1.
/// `pilot` false makes it a mono station: no pilot, no difference subcarrier, so a stereo
/// receiver must fall back to the sum signal on both channels.
#[must_use]
pub fn composite(left: &[f32], right: &[f32], pilot: bool, rate: f64) -> Vec<f32> {
    assert_eq!(
        left.len(),
        right.len(),
        "left and right must be the same length"
    );
    left.iter()
        .zip(right)
        .enumerate()
        .map(|(n, (&l, &r))| {
            let (l, r) = (f64::from(l), f64::from(r));
            let phase = TAU * PILOT_HZ * n as f64 / rate;
            let sum = AUDIO_LEVEL * (l + r) / 2.0;
            if !pilot {
                return sum as f32;
            }
            let difference = AUDIO_LEVEL * (l - r) / 2.0 * -(2.0 * phase).sin();
            (sum + difference + PILOT_LEVEL * phase.cos()) as f32
        })
        .collect()
}

/// The same multiplex frequency-modulated onto a carrier as complex baseband IQ — what a WFM
/// channel receives off the air.
#[must_use]
pub fn transmission(left: &[f32], right: &[f32], pilot: bool, rate: f64) -> Vec<Complex<f32>> {
    fm_modulate(&composite(left, right, pilot, rate), DEVIATION_HZ, rate)
}

#[cfg(test)]
mod tests {
    use super::{super::tone_audio, *};

    const RATE: f64 = 240_000.0;

    fn program(len: usize) -> (Vec<f32>, Vec<f32>) {
        (
            tone_audio(1_000.0, 1.0, RATE, len),
            tone_audio(3_000.0, 1.0, RATE, len),
        )
    }

    /// Correlating against a reference recovers its share only if that component is present at
    /// that phase: the pilot at its nominal level, and the difference signal on `−sin(2ωt)`
    /// rather than the `+sin` or `cos` a sign or quadrature slip would put it on.
    fn correlate(mpx: &[f32], reference: impl Fn(f64) -> f64) -> f64 {
        mpx.iter()
            .enumerate()
            .map(|(n, &s)| f64::from(s) * reference(TAU * PILOT_HZ * n as f64 / RATE))
            .sum::<f64>()
            / mpx.len() as f64
    }

    #[test]
    fn composite_carries_the_pilot_and_the_difference_signal_in_the_standard_phase() {
        // One channel silent, so (L−R)/2 is a plain 1 kHz tone rather than a two-tone mix.
        let len = 24_000;
        let (left, _) = program(len);
        let silent = vec![0.0f32; len];
        let mpx = composite(&left, &silent, true, RATE);

        for (n, &s) in mpx.iter().enumerate() {
            assert!((-1.0..=1.0).contains(&s), "sample {n} out of range: {s}");
        }
        assert!(
            (correlate(&mpx, f64::cos) - PILOT_LEVEL / 2.0).abs() < 0.005,
            "pilot level {}",
            correlate(&mpx, f64::cos)
        );
        // The difference signal rides on −sin(2ωt); correlating the multiplex against that
        // subcarrier leaves the 1 kHz tone, which averages to zero — so demodulate first and
        // measure what lands at baseband.
        let demodulated: Vec<f32> = mpx
            .iter()
            .enumerate()
            .map(|(n, &s)| {
                let phase = TAU * PILOT_HZ * n as f64 / RATE;
                (f64::from(s) * -2.0 * (2.0 * phase).sin()) as f32
            })
            .collect();
        let recovered: f64 = demodulated
            .iter()
            .zip(&left)
            .map(|(&d, &l)| f64::from(d) * f64::from(l))
            .sum::<f64>()
            / demodulated.len() as f64;
        // (L−R)/2 = L/2 at 0.45 deviation share, correlated against L (mean square 1/2).
        assert!(
            (recovered - AUDIO_LEVEL / 4.0).abs() < 0.01,
            "difference level {recovered}"
        );
    }

    #[test]
    fn a_mono_station_carries_neither_pilot_nor_subcarrier() {
        let len = 24_000;
        let (left, right) = program(len);
        let mpx = composite(&left, &right, false, RATE);
        assert!(correlate(&mpx, f64::cos).abs() < 0.002, "pilot present");
        assert!(
            correlate(&mpx, |p| -(2.0 * p).sin()).abs() < 0.002,
            "subcarrier present"
        );
    }

    #[test]
    fn transmission_is_a_unit_magnitude_carrier() {
        let (left, right) = program(2_400);
        let iq = transmission(&left, &right, true, RATE);
        assert_eq!(iq.len(), 2_400);
        for (n, s) in iq.iter().enumerate() {
            assert!(
                (s.norm() - 1.0).abs() < 1e-3,
                "sample {n} magnitude {}",
                s.norm()
            );
        }
    }
}
