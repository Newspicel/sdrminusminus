use num_complex::Complex;

use crate::{decim::Decimator, fir::design_lowpass};

pub const DEFAULT_TAPS: usize = 47;

/// Turns the real samples a quadrature-sampling receiver delivers into the complex baseband the
/// rest of the chain expects, at half the input rate.
///
/// The band of interest sits at a quarter of the input rate. Translating by that quarter costs no
/// multiplies, the rotation cycles through 1, -j, -1, j, and leaves the wanted signal at DC and
/// its mirror at the edge, where a half-band low-pass removes it on the way down by two.
#[derive(Clone, Debug)]
pub struct RealToIq {
    decimator: Decimator,
    rotated: Vec<Complex<f32>>,
    phase: u8,
}

impl RealToIq {
    /// # Panics
    /// Panics unless `taps` is at least 3 and congruent to 3 modulo 4, which is what puts a zero
    /// at every even offset from the centre tap and makes the filter a half-band.
    #[must_use]
    pub fn new(taps: usize) -> Self {
        assert!(taps >= 3, "need at least 3 taps");
        assert!(
            taps % 4 == 3,
            "a half-band filter has 4k+3 taps, so that every even offset from its centre is zero"
        );
        Self {
            decimator: Decimator::new(&design_lowpass(taps, 0.25), 2),
            rotated: Vec::new(),
            phase: 0,
        }
    }

    pub fn reset(&mut self) {
        self.decimator.reset();
        self.rotated.clear();
        self.phase = 0;
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<Complex<f32>>) {
        self.rotated.clear();
        self.rotated.reserve(input.len());
        for &sample in input {
            self.rotated.push(match self.phase {
                0 => Complex::new(sample, 0.0),
                1 => Complex::new(0.0, -sample),
                2 => Complex::new(-sample, 0.0),
                _ => Complex::new(0.0, sample),
            });
            self.phase = (self.phase + 1) & 3;
        }
        self.decimator.process(&self.rotated, out);
    }
}

impl Default for RealToIq {
    fn default() -> Self {
        Self::new(DEFAULT_TAPS)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use rustfft::FftPlanner;

    use super::*;
    use crate::testutil::tone_peak_and_snr;

    fn real_cosine(freq_norm: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (TAU * freq_norm * n as f64).cos() as f32)
            .collect()
    }

    fn converted(freq_norm: f64, len: usize) -> Vec<Complex<f32>> {
        let mut converter = RealToIq::default();
        let mut out = Vec::new();
        converter.process(&real_cosine(freq_norm, len), &mut out);
        out
    }

    #[test]
    fn a_tone_at_a_quarter_of_the_rate_lands_on_dc() {
        let out = converted(0.25, 8192);
        let settled = &out[128..];
        let mean = settled.iter().sum::<Complex<f32>>() / settled.len() as f32;
        assert!(
            mean.norm() > 0.45,
            "a tone at the centre must survive as a steady vector, got {}",
            mean.norm()
        );
    }

    const WINDOW: usize = 4096;

    fn settled(freq_norm: f64) -> Vec<Complex<f32>> {
        converted(freq_norm, 2 * WINDOW + 1024)[512..512 + WINDOW].to_vec()
    }

    fn bin_power(samples: &[Complex<f32>], bin: i64) -> f64 {
        let n = samples.len();
        let mut buf = samples.to_vec();
        FftPlanner::new().plan_fft_forward(n).process(&mut buf);
        f64::from(buf[bin.rem_euclid(n as i64) as usize].norm_sqr())
    }

    /// Offsets are chosen to land on a whole output bin, so that what the assertions measure is
    /// the converter rather than the leakage of an unwindowed tone between two bins.
    fn offset_for_bin(bin: i64) -> f64 {
        bin as f64 / (2.0 * WINDOW as f64)
    }

    #[test]
    fn an_offset_tone_lands_at_twice_its_offset_and_keeps_its_sign() {
        for bin in [160_i64, -160] {
            let offset = offset_for_bin(bin);
            let out = settled(0.25 + offset);
            let (peak, snr) = tone_peak_and_snr(&out);
            let expected = bin.rem_euclid(WINDOW as i64) as usize;
            assert!(
                peak.abs_diff(expected) <= 1,
                "an offset of {offset} should appear at bin {expected}, found {peak}"
            );
            assert!(
                snr > 45.0,
                "an offset of {offset} kept only {snr} dB above everything else"
            );
        }
    }

    /// The whole real band below Nyquist is kept, not just a window around the centre: input
    /// frequency `f` arrives at `2(f - 1/4)`, which sweeps the full output band as `f` sweeps
    /// from 0 to a half.
    #[test]
    fn the_whole_real_band_maps_onto_the_output_band() {
        for bin in [-1600_i64, -800, 800, 1600] {
            let offset = offset_for_bin(bin);
            let out = settled(0.25 + offset);
            let (peak, _) = tone_peak_and_snr(&out);
            let expected = bin.rem_euclid(WINDOW as i64) as usize;
            assert!(
                peak.abs_diff(expected) <= 1,
                "an offset of {offset} should appear at bin {expected}, found {peak}"
            );
        }
    }

    /// A real input carries a tone at `+f` and at `-f` alike. Only one of the two may survive as
    /// a complex output, or every signal arrives with a mirror image folded over it.
    #[test]
    fn the_unwanted_half_of_a_real_tone_is_rejected() {
        let wanted = 240_i64;
        let out = settled(0.25 + offset_for_bin(wanted));
        let rejection =
            10.0 * (bin_power(&out, wanted) / bin_power(&out, -wanted).max(1e-30)).log10();
        assert!(
            rejection > 50.0,
            "the mirror of the wanted tone is only {rejection} dB down"
        );
    }

    #[test]
    fn the_output_runs_at_half_the_input_rate() {
        let mut converter = RealToIq::default();
        let mut out = Vec::new();
        converter.process(&real_cosine(0.25, 4096), &mut out);
        assert_eq!(out.len(), 2048);
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let input = real_cosine(0.27, 10_000);
        let mut whole = RealToIq::default();
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = RealToIq::default();
        let mut got = Vec::new();
        let mut block = Vec::new();
        let mut at = 0;
        for len in [1usize, 7, 64, 3, 129, 1024, 17].iter().cycle() {
            if at >= input.len() {
                break;
            }
            let end = (at + len).min(input.len());
            ragged.process(&input[at..end], &mut block);
            got.extend_from_slice(&block);
            at = end;
        }
        assert_eq!(expected, got);
    }

    #[test]
    fn a_reset_converter_repeats_its_first_output() {
        let input = real_cosine(0.3, 2048);
        let mut converter = RealToIq::default();
        let mut first = Vec::new();
        converter.process(&input, &mut first);
        converter.reset();
        let mut again = Vec::new();
        converter.process(&input, &mut again);
        assert_eq!(first, again);
    }

    #[test]
    #[should_panic(expected = "half-band filter has 4k+3 taps")]
    fn a_tap_count_that_is_not_half_band_is_refused() {
        let _ = RealToIq::new(49);
    }
}
