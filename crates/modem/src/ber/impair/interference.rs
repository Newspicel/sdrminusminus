//! Interference: an independent transmitter added at a stated carrier-to-interference ratio.
//! Self-contained on purpose — the interferer must not be built from the modulator under test, or
//! an interference limits row would move whenever the entry it measures does.
use num_complex::Complex;
use sdrmm_dsp::fir::design_rrc;

use super::{Impairment, mean_power};
use crate::ber::rng::Rng;

/// Pulse-shaping span of the interferer's RRC, symbols each side — enough that its spectrum
/// is the filter's, not the truncation's.
const SPAN: usize = 8;

/// What the interfering transmitter radiates.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// RRC-shaped QPSK at `sps` samples per symbol and roll-off `alpha`.
    Qpsk { sps: usize, alpha: f64 },
    /// An unmodulated carrier whose offset is drawn uniformly over `±half_band_cycles` per
    /// realisation, with a random phase — see the module docs for why the draw is the definition.
    Narrowband { half_band_cycles: f64 },
}

/// An interferer at `ci_db` below the waveform's measured power (C/I, so larger is cleaner),
/// offset by `offset_cycles` per sample: 0 for co-channel, non-zero for adjacent-channel. The
/// interferer is scaled against its own *measured* power over the waveform, so the stated C/I is
/// exact rather than nominal.
#[derive(Clone, Copy, Debug)]
pub struct Interferer {
    ci_db: f64,
    offset_cycles: f64,
    source: Source,
}

impl Interferer {
    #[must_use]
    pub fn cochannel(ci_db: f64, sps: usize, alpha: f64) -> Self {
        Self {
            ci_db,
            offset_cycles: 0.0,
            source: Source::Qpsk { sps, alpha },
        }
    }

    #[must_use]
    pub fn adjacent(ci_db: f64, offset_cycles: f64, sps: usize, alpha: f64) -> Self {
        Self {
            ci_db,
            offset_cycles,
            source: Source::Qpsk { sps, alpha },
        }
    }

    /// An unmodulated carrier at a frequency drawn uniformly over `±half_band_cycles` per
    /// realisation — the narrowband jammer a spread-spectrum entry's processing gain is measured
    /// against. The band is the *caller's* statement of where such a jammer could sit; a spread
    /// entry passes half its own chip rate, since outside that the receive filter has already
    /// removed the jammer and any rejection measured there is filtering rather than spreading.
    #[must_use]
    pub fn narrowband(ci_db: f64, half_band_cycles: f64) -> Self {
        Self {
            ci_db,
            offset_cycles: 0.0,
            source: Source::Narrowband { half_band_cycles },
        }
    }

    /// This interferer at a *fixed* offset instead of a drawn one — what a partial-band row
    /// needs, where the point is a jammer parked on one channel rather than an average over
    /// where it might sit.
    #[must_use]
    pub fn parked(ci_db: f64, offset_cycles: f64) -> Self {
        Self {
            ci_db,
            offset_cycles,
            source: Source::Narrowband {
                half_band_cycles: 0.0,
            },
        }
    }
}

impl Impairment for Interferer {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        let carrier = mean_power(x);
        if carrier <= 0.0 {
            return;
        }
        let (mut interferer, offset) = match self.source {
            Source::Qpsk { sps, alpha } => (rrc_qpsk(rng, x.len(), sps, alpha), self.offset_cycles),
            Source::Narrowband { half_band_cycles } => {
                let phase = std::f64::consts::TAU * rng.uniform();
                let (sin, cos) = phase.sin_cos();
                (
                    vec![Complex::new(cos as f32, sin as f32); x.len()],
                    self.offset_cycles + half_band_cycles * (2.0 * rng.uniform() - 1.0),
                )
            }
        };
        let mut acc = 0.0f64;
        for s in &mut interferer {
            let phase = std::f64::consts::TAU * acc;
            let (sin, cos) = phase.sin_cos();
            let re = f64::from(s.re);
            let im = f64::from(s.im);
            s.re = (re * cos - im * sin) as f32;
            s.im = (re * sin + im * cos) as f32;
            acc += offset;
            acc -= acc.floor();
        }
        let own = mean_power(&interferer);
        if own <= 0.0 {
            return;
        }
        let target = carrier / 10f64.powf(self.ci_db / 10.0);
        let scale = (target / own).sqrt() as f32;
        for (s, i) in x.iter_mut().zip(&interferer) {
            s.re += i.re * scale;
            s.im += i.im * scale;
        }
    }
}

/// `len` samples of steady-state RRC-shaped QPSK at `sps` samples per symbol — the shared
/// band-limited test waveform of this module (the timing calibrations borrow it: it has the
/// smooth, aperiodic autocorrelation a sub-sample delay measurement needs). Filter warm-up is
/// generated and discarded so the returned stretch has full power from its first sample.
pub(crate) fn rrc_qpsk(rng: &mut Rng, len: usize, sps: usize, alpha: f64) -> Vec<Complex<f32>> {
    let taps = design_rrc(sps as f64, alpha, SPAN);
    let lead = SPAN * sps;
    let total = len + lead + taps.len();
    let mut impulses = vec![Complex::new(0.0f32, 0.0); total];
    let mut k = 0;
    while k < total {
        let bits = rng.next_u64() & 3;
        let level = std::f32::consts::FRAC_1_SQRT_2;
        impulses[k] = Complex::new(
            if bits & 1 == 0 { level } else { -level },
            if bits & 2 == 0 { level } else { -level },
        );
        k += sps;
    }
    (lead..lead + len)
        .map(|n| {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (j, &h) in taps.iter().enumerate() {
                if let Some(s) = n.checked_sub(j).and_then(|i| impulses.get(i)) {
                    re += f64::from(h) * f64::from(s.re);
                    im += f64::from(h) * f64::from(s.im);
                }
            }
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use num_complex::Complex;

    use super::Interferer;
    use crate::ber::{
        impair::{Impairment, mean_power, testutil::tone},
        rng::Rng,
    };

    fn added(y: &[Complex<f32>], x: &[Complex<f32>]) -> Vec<Complex<f32>> {
        y.iter().zip(x).map(|(a, b)| a - b).collect()
    }

    /// Applied == measured: the power added on top of a unit carrier sits at the stated C/I.
    #[test]
    fn cochannel_power_matches_stated_ci() {
        let ci_db = 17.0;
        let x = tone(0.05, 100_000);
        let mut y = x.clone();
        Interferer::cochannel(ci_db, 4, 0.35).apply(&mut y, &mut Rng::new(0xcc1));
        let measured = 10.0 * (mean_power(&x) / mean_power(&added(&y, &x))).log10();
        assert!(
            (measured - ci_db).abs() < 0.2,
            "applied C/I {ci_db} dB, measured {measured} dB"
        );
    }

    /// Applied == measured for the adjacent case: same power gate, and the added signal's
    /// power-weighted mean frequency sits at the stated offset (the RRC spectrum is symmetric
    /// about its carrier, so the mean frequency *is* the offset).
    #[test]
    fn adjacent_offset_and_power_read_back() {
        let ci_db = 12.0;
        let offset = 0.2;
        let x = tone(0.0, 100_000);
        let mut y = x.clone();
        Interferer::adjacent(ci_db, offset, 4, 0.35).apply(&mut y, &mut Rng::new(0xad1));
        let i = added(&y, &x);
        let measured_ci = 10.0 * (mean_power(&x) / mean_power(&i)).log10();
        assert!(
            (measured_ci - ci_db).abs() < 0.2,
            "measured C/I {measured_ci}"
        );
        let mut acc = Complex::new(0.0f64, 0.0);
        for w in i.windows(2) {
            let a = Complex::new(f64::from(w[1].re), f64::from(w[1].im));
            let b = Complex::new(f64::from(w[0].re), f64::from(w[0].im));
            acc += a * b.conj();
        }
        let measured_offset = acc.arg() / TAU;
        assert!(
            (measured_offset - offset).abs() < 0.005,
            "applied offset {offset}, measured {measured_offset}"
        );
    }

    /// Applied == measured for the narrowband kind, on both axes it claims: the added power sits
    /// at the stated C/I, and what was added is a *carrier* — a single frequency, which reads back
    /// as a constant envelope rather than as a bandwidth.
    #[test]
    fn narrowband_power_and_constant_envelope_read_back() {
        let ci_db = 9.0;
        let x = tone(0.0, 60_000);
        let mut y = x.clone();
        Interferer::narrowband(ci_db, 0.1).apply(&mut y, &mut Rng::new(0xcc01));
        let i = added(&y, &x);
        let measured = 10.0 * (mean_power(&x) / mean_power(&i)).log10();
        assert!(
            (measured - ci_db).abs() < 0.01,
            "measured C/I {measured} dB"
        );
        let rms = mean_power(&i).sqrt();
        let worst = i
            .iter()
            .map(|s| (f64::from(s.norm()) / rms - 1.0).abs())
            .fold(0.0f64, f64::max);
        assert!(worst < 1e-3, "envelope varies by {worst} of its RMS");
    }

    /// The property that makes the narrowband kind a *processing-gain* instrument rather than a
    /// tone at an arbitrary place: the offset is redrawn per realisation, uniformly over the
    /// stated band, and a parked interferer is not. Measured as the spread of the recovered
    /// frequencies over many seeds.
    #[test]
    fn the_narrowband_offset_is_drawn_over_its_stated_band() {
        let half_band = 0.1;
        let recovered = |interferer: Interferer, seed: u64| {
            let x = tone(0.0, 20_000);
            let mut y = x.clone();
            interferer.apply(&mut y, &mut Rng::new(seed));
            let i = added(&y, &x);
            let mut acc = Complex::new(0.0f64, 0.0);
            for w in i.windows(2) {
                let a = Complex::new(f64::from(w[1].re), f64::from(w[1].im));
                let b = Complex::new(f64::from(w[0].re), f64::from(w[0].im));
                acc += a * b.conj();
            }
            acc.arg() / TAU
        };
        let drawn: Vec<f64> = (0..64)
            .map(|s| recovered(Interferer::narrowband(6.0, half_band), 0xcc00 + s))
            .collect();
        assert!(
            drawn.iter().all(|f| f.abs() <= half_band + 1e-6),
            "an offset landed outside the stated band"
        );
        // Uniform over ±0.1 has RMS 0.1/√3 ≈ 0.058; anything much smaller would mean the draw is
        // not happening, and the whole measurement would silently become a single-tone one.
        let rms = (drawn.iter().map(|f| f * f).sum::<f64>() / drawn.len() as f64).sqrt();
        assert!((0.03..0.09).contains(&rms), "offset RMS {rms}");
        for seed in 0..8u64 {
            let at = recovered(Interferer::parked(6.0, 0.07), seed);
            assert!((at - 0.07).abs() < 1e-3, "parked offset read {at}");
        }
    }
}
