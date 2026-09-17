use std::f64::consts::TAU;

use num_complex::Complex;

const TRACK_RANGE_HZ: f64 = 7.6;
const TRACK_POLE: f64 = 0.52;

/// Coherent MSK bit detector for a real-valued subcarrier. A bit repeats the previous one while the
/// upper tone is sent and flips on the lower tone; the absolute polarity is ambiguous.
pub struct MskDetector {
    sample_rate: f64,
    centre_hz: f64,
    bit_advance: f64,
    phase: f64,
    clock: f64,
    quarter: u8,
    offset_hz: f64,
    ring: Vec<Complex<f32>>,
    oldest: usize,
    taps: Vec<f32>,
}

impl MskDetector {
    #[must_use]
    pub fn new(sample_rate: f64, centre_hz: f64, baud: f64) -> Self {
        assert!(
            sample_rate.is_finite() && sample_rate > 0.0,
            "sample rate must be positive"
        );
        assert!(
            baud > 0.0 && centre_hz > 0.0 && centre_hz + baud / 4.0 < sample_rate / 2.0,
            "both MSK tones must lie inside the Nyquist band"
        );
        let len = (sample_rate / (baud / 2.0)) as usize + 1;
        let taps = (0..len)
            .map(|j| {
                let t = (j as f64 - (len as f64 - 1.0) / 2.0) / sample_rate;
                (TAU * baud / 4.0 * t).cos().max(0.0) as f32
            })
            .collect();
        Self {
            sample_rate,
            centre_hz,
            bit_advance: TAU * centre_hz / baud,
            phase: 0.0,
            clock: 0.0,
            quarter: 0,
            offset_hz: 0.0,
            ring: vec![Complex::new(0.0, 0.0); len],
            oldest: 0,
            taps,
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.clock = 0.0;
        self.quarter = 0;
        self.offset_hz = 0.0;
        self.ring.fill(Complex::new(0.0, 0.0));
        self.oldest = 0;
    }

    pub fn push(&mut self, sample: f32) -> Option<bool> {
        let step = TAU * (self.centre_hz + self.offset_hz) / self.sample_rate;
        self.phase = (self.phase + step).rem_euclid(TAU);
        let (sin, cos) = self.phase.sin_cos();
        self.ring[self.oldest] = Complex::new(sample * cos as f32, -sample * sin as f32);
        self.oldest = (self.oldest + 1) % self.ring.len();
        self.clock += step;
        if self.clock < self.bit_advance - step / 2.0 {
            return None;
        }
        self.clock -= self.bit_advance;
        Some(self.decide(self.matched()))
    }

    fn matched(&self) -> Complex<f32> {
        let mut v = Complex::new(0.0f32, 0.0);
        for (j, &tap) in self.taps.iter().enumerate() {
            v += tap * self.ring[(j + self.oldest) % self.ring.len()];
        }
        v / (v.norm() + 1e-8)
    }

    fn decide(&mut self, v: Complex<f32>) -> bool {
        let (axis, error) = if self.quarter & 1 == 1 {
            (v.im, if v.im >= 0.0 { -v.re } else { v.re })
        } else {
            (v.re, if v.re >= 0.0 { v.im } else { -v.im })
        };
        let bit = if self.quarter & 2 == 2 { -axis } else { axis } > 0.0;
        self.quarter = self.quarter.wrapping_add(1);
        self.offset_hz =
            TRACK_POLE * self.offset_hz + (1.0 - TRACK_POLE) * TRACK_RANGE_HZ * f64::from(error);
        bit
    }

    #[must_use]
    pub fn offset_hz(&self) -> f64 {
        self.offset_hz
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::rng::Rng;

    use super::*;

    const RATE: f64 = 48_000.0;
    const BAUD: f64 = 2_400.0;
    const CENTRE_HZ: f64 = 1_800.0;
    const PREKEY_BITS: usize = 80;

    fn random_bits(seed: u64, n: usize) -> Vec<bool> {
        let mut rng = Rng::new(seed);
        let mut bits = vec![false; PREKEY_BITS];
        bits.extend((0..n).map(|_| rng.next_u64() & 1 == 1));
        bits
    }

    fn upper_tone_flags(bits: &[bool]) -> Vec<bool> {
        let mut last = false;
        bits.iter()
            .map(|&b| {
                let hold = b == last;
                last = b;
                hold
            })
            .collect()
    }

    fn audio(bits: &[bool], clock_ppm: f64, amplitude: f32) -> Vec<f32> {
        let scale = 1.0 + clock_ppm * 1e-6;
        let sps = RATE / (BAUD * scale);
        let flags = upper_tone_flags(bits);
        let mut phase = Rng::new(bits.len() as u64).uniform() * TAU;
        (0..(bits.len() as f64 * sps) as usize)
            .map(|k| {
                let idx = ((k as f64 / sps) as usize).min(flags.len() - 1);
                let tone = if flags[idx] {
                    CENTRE_HZ + BAUD / 4.0
                } else {
                    CENTRE_HZ - BAUD / 4.0
                };
                phase = (phase + TAU * tone * scale / RATE).rem_euclid(TAU);
                amplitude * phase.sin() as f32
            })
            .collect()
    }

    fn detect(samples: &[f32]) -> Vec<bool> {
        let mut det = MskDetector::new(RATE, CENTRE_HZ, BAUD);
        samples.iter().filter_map(|&s| det.push(s)).collect()
    }

    fn errors_after_settling(sent: &[bool], got: &[bool]) -> usize {
        let data = &sent[PREKEY_BITS..];
        (0..4)
            .filter_map(|lag| got.get(PREKEY_BITS + lag..))
            .filter(|got| got.len() + 8 >= data.len())
            .flat_map(|got| {
                [false, true].map(|flip| {
                    data.iter()
                        .zip(got)
                        .filter(|&(&s, &g)| s != (g != flip))
                        .count()
                })
            })
            .min()
            .unwrap_or(data.len())
    }

    #[test]
    fn emits_one_bit_per_symbol() {
        let bits = random_bits(1, 300);
        let got = detect(&audio(&bits, 0.0, 1.0));
        assert!(got.len().abs_diff(bits.len()) <= 1, "{} bits", got.len());
    }

    #[test]
    fn recovers_a_random_bit_stream_up_to_polarity() {
        let bits = random_bits(2, 500);
        let got = detect(&audio(&bits, 0.0, 0.3));
        assert_eq!(errors_after_settling(&bits, &got), 0);
    }

    #[test]
    fn tracks_a_transmitter_clock_error() {
        let bits = random_bits(3, 1_500);
        for clock_ppm in [-500.0, 200.0, 500.0] {
            let got = detect(&audio(&bits, clock_ppm, 1.0));
            assert_eq!(
                errors_after_settling(&bits, &got),
                0,
                "{clock_ppm} ppm clock error"
            );
        }
    }

    #[test]
    fn survives_additive_noise() {
        let bits = random_bits(4, 2_000);
        let mut rng = Rng::new(99);
        let noisy: Vec<f32> = audio(&bits, 0.0, 1.0)
            .iter()
            .map(|&s| s + 0.35 * rng.normal() as f32)
            .collect();
        let errors = errors_after_settling(&bits, &detect(&noisy));
        assert!(
            errors * 100 < bits.len(),
            "{errors} errors in {} bits",
            bits.len()
        );
    }

    #[test]
    fn silence_yields_no_tracking_drift() {
        let mut det = MskDetector::new(RATE, CENTRE_HZ, BAUD);
        for _ in 0..48_000 {
            det.push(0.0);
        }
        assert_eq!(det.offset_hz(), 0.0);
    }

    #[test]
    fn reset_forgets_the_tracked_offset() {
        let bits = random_bits(5, 400);
        let mut det = MskDetector::new(RATE, CENTRE_HZ, BAUD);
        for s in audio(&bits, 500.0, 1.0) {
            det.push(s);
        }
        assert_ne!(det.offset_hz(), 0.0);
        det.reset();
        assert_eq!(det.offset_hz(), 0.0);
    }

    #[test]
    #[should_panic(expected = "Nyquist")]
    fn tones_outside_the_band_are_rejected() {
        let _ = MskDetector::new(4_000.0, 1_800.0, 2_400.0);
    }
}
