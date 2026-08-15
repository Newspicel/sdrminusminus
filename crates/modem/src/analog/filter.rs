use std::f64::consts::PI;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FirC, design_lowpass};

pub const MAX_TAPS: usize = 1_023;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandFilter {
    pub low: f64,
    pub high: f64,
    pub taps: usize,
}

impl BandFilter {
    #[must_use]
    pub fn symmetric(half_width: f64, taps: usize) -> Self {
        Self {
            low: -half_width,
            high: half_width,
            taps,
        }
    }

    #[must_use]
    pub fn build(&self) -> Band {
        const EDGE: f64 = 0.4999;
        const GUARD: f64 = 1e-6;
        let low = self.low.clamp(-EDGE, EDGE - GUARD);
        let high = self.high.clamp((low + GUARD).min(EDGE), EDGE);
        let taps = self.taps.clamp(3, MAX_TAPS) | 1;
        let half_width = 0.5 * (high - low);
        let centre = 0.5 * (high + low);
        let prototype = design_lowpass(taps, half_width.min(0.4999));
        if centre.abs() < 1e-12 {
            Band::Real(Decimator::new(&prototype, 1))
        } else {
            Band::Complex(FirC::from_lowpass(&prototype, centre))
        }
    }

    #[must_use]
    pub fn delay(&self) -> usize {
        (self.taps.clamp(3, MAX_TAPS) | 1) / 2
    }
}

pub enum Band {
    Real(Decimator),
    Complex(FirC),
}

impl Band {
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        match self {
            Self::Real(f) => f.process(iq, out),
            Self::Complex(f) => f.process(iq, out),
        }
    }
}

pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    #[must_use]
    pub fn new(samples: usize) -> Self {
        Self {
            buf: vec![0.0; samples],
            pos: 0,
        }
    }

    pub fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if self.buf.is_empty() {
            out.extend_from_slice(x);
            return;
        }
        for &v in x {
            out.push(self.buf[self.pos]);
            self.buf[self.pos] = v;
            self.pos = (self.pos + 1) % self.buf.len();
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

#[must_use]
pub fn design_from_response(
    taps: usize,
    oversample: usize,
    response: impl Fn(f64) -> Complex<f64>,
) -> Vec<Complex<f32>> {
    assert!(
        (3..=MAX_TAPS).contains(&taps),
        "band filters run 3..={MAX_TAPS} taps, got {taps}"
    );
    let taps = taps | 1;
    let centre = (taps - 1) as f64 / 2.0;
    let grid = (taps * oversample.max(1)).next_power_of_two();
    let window = blackman(taps);
    (0..taps)
        .map(|k| {
            let offset = k as f64 - centre;
            let mut acc = Complex::new(0.0, 0.0);
            for g in 0..grid {
                let f = (g as f64 + 0.5) / grid as f64 - 0.5;
                acc += response(f) * Complex::from_polar(1.0, 2.0 * PI * f * offset);
            }
            let h = acc / grid as f64 * window[k];
            Complex::new(h.re as f32, h.im as f32)
        })
        .collect()
}

fn blackman(taps: usize) -> Vec<f64> {
    let n = (taps - 1) as f64;
    (0..taps)
        .map(|k| {
            let x = 2.0 * PI * k as f64 / n;
            0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
        })
        .collect()
}

#[must_use]
pub fn design_hilbert(taps: usize) -> Vec<f32> {
    assert!(
        (3..=MAX_TAPS).contains(&taps) && !taps.is_multiple_of(2),
        "a Hilbert transformer needs an odd 3..={MAX_TAPS} taps, got {taps}"
    );
    let centre = (taps - 1) as isize / 2;
    let window = blackman(taps);
    (0..taps)
        .map(|k| {
            let n = k as isize - centre;
            if n % 2 == 0 {
                0.0
            } else {
                (2.0 / (PI * n as f64) * window[k]) as f32
            }
        })
        .collect()
}

#[must_use]
pub fn design_vestigial(taps: usize, vestige: f64, upper: f64) -> Vec<Complex<f32>> {
    assert!(
        vestige > 0.0 && vestige < upper && upper < 0.5,
        "need 0 < vestige < upper < ½, got vestige {vestige}, upper {upper}"
    );
    design_from_response(taps, 16, |f| {
        let gain = if f < -vestige || f > upper {
            0.0
        } else if f <= vestige {
            0.5 * (1.0 + (PI * f / (2.0 * vestige)).sin())
        } else {
            1.0
        };
        Complex::new(gain, 0.0)
    })
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;

    fn response(taps: &[Complex<f32>], f: f64) -> Complex<f64> {
        taps.iter()
            .enumerate()
            .map(|(k, &t)| {
                Complex::new(f64::from(t.re), f64::from(t.im))
                    * Complex::from_polar(1.0, -2.0 * PI * f * k as f64)
            })
            .sum()
    }

    fn real_response(taps: &[f32], f: f64) -> Complex<f64> {
        let complex: Vec<Complex<f32>> = taps.iter().map(|&t| Complex::new(t, 0.0)).collect();
        response(&complex, f)
    }

    #[test]
    fn a_symmetric_band_is_a_real_filter_and_an_offset_one_is_not() {
        assert!(matches!(
            BandFilter::symmetric(0.1, 65).build(),
            Band::Real(_)
        ));
        assert!(matches!(
            BandFilter {
                low: 0.01,
                high: 0.1,
                taps: 65
            }
            .build(),
            Band::Complex(_)
        ));
    }

    #[test]
    fn a_band_at_the_nyquist_edge_still_builds() {
        for (low, high) in [(0.4999, 0.5), (0.6, 0.7), (-0.7, -0.6), (0.5, 0.4)] {
            let mut band = BandFilter {
                low,
                high,
                taps: 33,
            }
            .build();
            let mut out = Vec::new();
            band.process(&[Complex::new(1.0, 0.0); 64], &mut out);
            assert_eq!(out.len(), 64, "band {low}..{high} produced no output");
            assert!(
                out.iter().all(|s| s.re.is_finite() && s.im.is_finite()),
                "band {low}..{high} produced a non-finite sample"
            );
        }
    }

    #[test]
    fn an_offset_band_passes_its_own_side_only() {
        let mut band = BandFilter {
            low: 0.02,
            high: 0.12,
            taps: 129,
        }
        .build();
        let level = |f: f64| {
            let x: Vec<Complex<f32>> = (0..4096)
                .map(|n| Complex::from_polar(1.0, (2.0 * PI * f * n as f64) as f32))
                .collect();
            let mut out = Vec::new();
            let mut band = BandFilter {
                low: 0.02,
                high: 0.12,
                taps: 129,
            }
            .build();
            band.process(&x, &mut out);
            let tail = &out[512..];
            (tail.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / tail.len() as f64).sqrt()
        };
        let mut sink = Vec::new();
        band.process(&[Complex::new(0.0, 0.0); 8], &mut sink);
        assert!((level(0.07) - 1.0).abs() < 0.02, "passband {}", level(0.07));
        assert!(level(-0.07) < 0.01, "image {}", level(-0.07));
        assert!(level(0.25) < 0.01, "stopband {}", level(0.25));
    }

    #[test]
    fn the_hilbert_response_is_a_quarter_turn_each_way() {
        let taps = design_hilbert(129);
        for &f in &[0.05, 0.1, 0.2, 0.4] {
            let h = real_response(&taps, f);
            assert!(
                (h.norm() - 1.0).abs() < 0.02,
                "magnitude at {f}: {}",
                h.norm()
            );
            let delay = Complex::from_polar(1.0, 2.0 * PI * f * ((taps.len() - 1) / 2) as f64);
            let turn = (h * delay).arg();
            assert!(
                (turn + std::f64::consts::FRAC_PI_2).abs() < 0.02,
                "phase at {f}: {turn}"
            );
            let mirror = real_response(&taps, -f) * delay.conj();
            assert!(
                (mirror.arg() - std::f64::consts::FRAC_PI_2).abs() < 0.02,
                "phase at -{f}: {}",
                mirror.arg()
            );
        }
        for (k, &t) in taps.iter().enumerate() {
            if (k as isize - 64) % 2 == 0 {
                assert_eq!(t, 0.0, "tap {k}");
            }
        }
    }

    #[test]
    fn the_vestigial_skirts_add_to_a_constant() {
        let taps = design_vestigial(257, 0.01, 0.1);
        let delay = |f: f64| Complex::from_polar(1.0, 2.0 * PI * f * 128.0);
        let at = |f: f64| (response(&taps, f) * delay(f)).re;
        assert!((at(0.0) - 0.5).abs() < 0.01, "carrier {}", at(0.0));
        for &f in &[0.002, 0.004, 0.006, 0.008] {
            let sum = at(f) + at(-f);
            assert!((sum - 1.0).abs() < 0.02, "skirts at ±{f} sum to {sum}");
        }
        assert!((at(0.05) - 1.0).abs() < 0.02, "upper sideband {}", at(0.05));
        assert!(at(-0.05).abs() < 0.02, "carved sideband {}", at(-0.05));
        assert!(at(0.2).abs() < 0.02, "stopband {}", at(0.2));
    }

    #[test]
    fn the_designer_reproduces_a_plain_lowpass() {
        let designed = design_from_response(65, 16, |f| {
            Complex::new(f64::from(u8::from(f.abs() <= 0.1)), 0.0)
        });
        let reference = design_lowpass(65, 0.1);
        for &f in &[0.0, 0.05, 0.09, 0.2, 0.4] {
            let a = response(&designed, f).norm();
            let b = real_response(&reference, f).norm();
            assert!((a - b).abs() < 0.03, "at {f}: designed {a}, reference {b}");
        }
    }
}
