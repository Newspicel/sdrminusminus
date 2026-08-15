use std::f64::consts::TAU;

use num_complex::Complex;

use super::{modulator::long_training_time, params::OfdmParams};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acquisition {
    pub long_start: usize,
    pub data_start: usize,
    pub cfo: f64,
    pub metric: f64,
}

#[derive(Clone, Debug)]
pub struct PreambleSync {
    fft: usize,
    period: usize,
    window: usize,
    short_samples: usize,
    long_guard: usize,
    long_repeats: usize,
    ltf: Vec<Complex<f32>>,
}

impl PreambleSync {
    #[must_use]
    pub fn new(params: &OfdmParams) -> Self {
        let preamble = params.preamble();
        let period = preamble.short_period(params.fft());
        assert!(
            preamble.short_repeats >= 3,
            "a repetition metric needs at least three short-training periods, got {}",
            preamble.short_repeats
        );
        Self {
            fft: params.fft(),
            period,
            window: (preamble.short_repeats - 2) * period,
            short_samples: preamble.short_repeats * period,
            long_guard: preamble.long_guard,
            long_repeats: preamble.long_repeats,
            ltf: long_training_time(params),
        }
    }

    #[must_use]
    pub fn span(&self) -> usize {
        self.short_samples + self.long_guard + self.long_repeats * self.fft
    }

    #[must_use]
    pub fn detect(&self, x: &[Complex<f32>], search: usize) -> Option<(usize, f64, f64)> {
        let needed = self.window + self.period;
        if x.len() < needed {
            return None;
        }
        let last = search.min(x.len() - needed);
        let mut p = Complex::new(0.0f64, 0.0);
        let mut r = 0.0f64;
        for n in 0..self.window {
            p += conj_product(x[n], x[n + self.period]);
            r += f64::from(x[n + self.period].norm_sqr());
        }
        let mut best = (0usize, p, f64::NEG_INFINITY);
        for d in 0..=last {
            if d > 0 {
                let out = d - 1;
                let into = d - 1 + self.window;
                p -= conj_product(x[out], x[out + self.period]);
                p += conj_product(x[into], x[into + self.period]);
                r -= f64::from(x[out + self.period].norm_sqr());
                r += f64::from(x[into + self.period].norm_sqr());
            }
            let metric = if r > 0.0 { p.norm_sqr() / (r * r) } else { 0.0 };
            if metric > best.2 {
                best = (d, p, metric);
            }
        }
        let (offset, correlation, metric) = best;
        Some((
            offset,
            correlation.arg() / (TAU * self.period as f64),
            metric,
        ))
    }

    #[must_use]
    pub fn acquire(&self, x: &[Complex<f32>], search: usize) -> Option<Acquisition> {
        let (plateau, coarse, metric) = self.detect(x, search)?;
        let long_start = self.locate_long(x, plateau, coarse)?;
        let cfo = self.fine_cfo(x, long_start, coarse);
        Some(Acquisition {
            long_start,
            data_start: long_start + self.long_repeats * self.fft,
            cfo,
            metric,
        })
    }

    fn locate_long(&self, x: &[Complex<f32>], from: usize, cfo: f64) -> Option<usize> {
        let span = self.short_samples + self.long_guard + self.period;
        let last = (from + span).min(x.len().checked_sub(2 * self.fft)?);
        let mut best = (from, f64::NEG_INFINITY);
        for t in from..=last {
            let score =
                self.correlate(x, t, cfo).norm() + self.correlate(x, t + self.fft, cfo).norm();
            if score > best.1 {
                best = (t, score);
            }
        }
        Some(best.0)
    }

    fn correlate(&self, x: &[Complex<f32>], t: usize, cfo: f64) -> Complex<f64> {
        let mut acc = Complex::new(0.0f64, 0.0);
        let (mut rot, step) = rotor(cfo, t);
        for (n, &tap) in self.ltf.iter().enumerate() {
            let Some(&sample) = x.get(t + n) else { break };
            let y = Complex::new(f64::from(sample.re), f64::from(sample.im)) * rot;
            acc += y * Complex::new(f64::from(tap.re), -f64::from(tap.im));
            rot *= step;
        }
        acc
    }

    fn fine_cfo(&self, x: &[Complex<f32>], long_start: usize, coarse: f64) -> f64 {
        let mut acc = Complex::new(0.0f64, 0.0);
        for n in 0..self.fft {
            let (Some(&a), Some(&b)) = (x.get(long_start + n), x.get(long_start + self.fft + n))
            else {
                break;
            };
            acc += conj_product(a, b);
        }
        let fft = self.fft as f64;
        let measured = acc.arg() / (TAU * fft);
        measured + ((coarse - measured) * fft).round() / fft
    }
}

fn conj_product(a: Complex<f32>, b: Complex<f32>) -> Complex<f64> {
    let a = Complex::new(f64::from(a.re), -f64::from(a.im));
    let b = Complex::new(f64::from(b.re), f64::from(b.im));
    a * b
}

pub(super) fn rotor(cfo: f64, n: usize) -> (Complex<f64>, Complex<f64>) {
    let phase = -TAU * cfo;
    let start = phase * n as f64;
    let (s0, c0) = start.sin_cos();
    let (s1, c1) = phase.sin_cos();
    (Complex::new(c0, s0), Complex::new(c1, s1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{
            impair::{Cfo, ChannelSpec, Impairment},
            rng::Rng,
        },
        ofdm::modulator::OfdmMod,
    };

    fn points(n: usize, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    if state & 1 == 0 { 1.0 } else { -1.0 },
                    if state & 2 == 0 { 1.0 } else { -1.0 },
                ) * std::f32::consts::FRAC_1_SQRT_2
            })
            .collect()
    }

    fn burst(lead: usize, symbols: usize, cfo_cycles: f64) -> Vec<Complex<f32>> {
        let params = OfdmParams::wifi_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        m.frame(
            &points(params.data_subcarriers() * symbols, 0x0f4),
            &mut wave,
        );
        if cfo_cycles != 0.0 {
            Cfo::from_cycles_per_sample(cfo_cycles).apply(&mut wave, &mut Rng::new(0));
        }
        wave
    }

    fn sync() -> PreambleSync {
        PreambleSync::new(&OfdmParams::wifi_like())
    }

    #[test]
    fn the_sliding_metric_equals_the_direct_sum() {
        let s = sync();
        let x = burst(37, 4, 0.0);
        let (offset, cfo, metric) = s.detect(&x, 80).unwrap();
        let direct = |d: usize| {
            let mut p = Complex::new(0.0f64, 0.0);
            let mut r = 0.0f64;
            for n in 0..s.window {
                p += conj_product(x[d + n], x[d + n + s.period]);
                r += f64::from(x[d + n + s.period].norm_sqr());
            }
            (p, p.norm_sqr() / (r * r))
        };
        let (p, m) = direct(offset);
        assert!((m - metric).abs() < 1e-9, "metric {metric} vs direct {m}");
        assert!((p.arg() / (TAU * s.period as f64) - cfo).abs() < 1e-12);
        for d in 0..=80usize {
            assert!(
                direct(d).1 <= metric + 1e-9,
                "candidate {d} beats the argmax"
            );
        }
    }

    #[test]
    fn a_clean_burst_is_located_exactly_at_every_lead() {
        let s = sync();
        for lead in [0usize, 1, 7, 40, 111] {
            let x = burst(lead, 4, 0.0);
            let a = s.acquire(&x, 160).unwrap();
            assert_eq!(a.long_start, lead + 160 + 32, "lead {lead}");
            assert_eq!(a.data_start, lead + 320, "lead {lead}");
            assert!(a.metric > 0.99, "lead {lead}: metric {}", a.metric);
            assert!(a.cfo.abs() < 1e-6, "lead {lead}: cfo {}", a.cfo);
        }
    }

    #[test]
    fn the_carrier_offset_is_recovered_across_the_coarse_range() {
        let s = sync();
        for applied in [-0.030, -0.012, -0.001, 0.0, 0.001, 0.012, 0.030] {
            let x = burst(23, 4, applied);
            let a = s.acquire(&x, 160).unwrap();
            assert_eq!(a.long_start, 23 + 192, "cfo {applied}");
            assert!(
                (a.cfo - applied).abs() < 1e-5,
                "applied {applied}, recovered {}",
                a.cfo
            );
        }
    }

    #[test]
    fn the_frame_is_placed_to_the_sample_under_noise() {
        let s = sync();
        let mut placed = 0;
        for trial in 0..20u64 {
            let mut x = burst(29, 8, 0.004);
            ChannelSpec::default()
                .awgn(crate::ber::impair::Awgn::with_sigma(0.12))
                .build()
                .apply(&mut x, &mut Rng::new(0x0fd0 + trial));
            let a = s.acquire(&x, 160).unwrap();
            if a.long_start == 29 + 192 && (a.cfo - 0.004).abs() < 5e-4 {
                placed += 1;
            }
        }
        assert_eq!(placed, 20, "{placed} of 20 frames placed exactly");
    }

    #[test]
    fn the_second_repeat_never_wins_the_correlation() {
        let s = sync();
        let x = burst(11, 4, 0.0);
        let t0 = 11 + 192;
        let single = |t: usize| s.correlate(&x, t, 0.0).norm();
        assert!(
            (single(t0) - single(t0 + 64)).abs() < 0.05 * single(t0),
            "the two repeats correlate equally, which is why one peak is ambiguous"
        );
        let pair = |t: usize| single(t) + single(t + 64);
        assert!(pair(t0) > 1.5 * pair(t0 + 64));
    }
}
