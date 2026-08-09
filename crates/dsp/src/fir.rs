//! Windowed-sinc FIR design and the shared streaming-FIR core (PLAN §7). Blackman window
//! throughout: ~74 dB stopband and a 5.5/N transition width — the tradeoff every decimation
//! stage in this crate is sized against.

use std::{
    f64::consts::PI,
    ops::{Add, Mul},
};

use num_complex::Complex;

/// Design a linear-phase lowpass. `cutoff` is the −6 dB point normalized to the sample rate
/// (`0 < cutoff < 0.5`). Normalized to unity DC gain (in f64, then rounded once to f32).
#[must_use]
pub fn design_lowpass(taps: usize, cutoff: f64) -> Vec<f32> {
    assert!(taps >= 3, "need at least 3 taps");
    assert!(cutoff > 0.0 && cutoff < 0.5, "cutoff must be in (0, 0.5)");
    let center = (taps - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..taps)
        .map(|k| 2.0 * cutoff * sinc(2.0 * cutoff * (k as f64 - center)) * blackman(k, taps))
        .collect();
    let sum: f64 = h.iter().sum();
    for v in &mut h {
        *v /= sum;
    }
    h.into_iter().map(|v| v as f32).collect()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn blackman(k: usize, n: usize) -> f64 {
    let x = 2.0 * PI * k as f64 / (n - 1) as f64;
    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

/// Sample types the streaming FIR cores operate on.
pub(crate) trait Sample: Copy + Add<Output = Self> {
    fn zero() -> Self;
}

impl Sample for f32 {
    fn zero() -> Self {
        0.0
    }
}

impl Sample for Complex<f32> {
    fn zero() -> Self {
        Complex::new(0.0, 0.0)
    }
}

/// Streaming FIR with an integer output stride. History carries across calls, so arbitrary
/// block splits yield bit-identical output; no per-call allocation once buffers reach steady
/// capacity. `factor > 1` evaluates the filter only at the retained instants — the polyphase
/// decimator identity (never compute what decimation would discard).
#[derive(Clone, Debug)]
pub(crate) struct StreamFir<T, C> {
    /// Stored newest-sample-first so each output is a forward dot product over the window.
    rev_taps: Vec<C>,
    factor: usize,
    buf: Vec<T>,
}

impl<T, C> StreamFir<T, C>
where
    T: Sample + Mul<C, Output = T>,
    C: Copy,
{
    pub(crate) fn new(taps: &[C], factor: usize) -> Self {
        assert!(!taps.is_empty(), "taps must not be empty");
        assert!(factor >= 1, "factor must be >= 1");
        // With factor > taps the post-emit stride can pass the buffer end and the drain
        // panics mid-stream on block-size-dependent input; the combination is meaningless
        // for an anti-alias filter anyway, so fail at construction like the other
        // precondition violations.
        assert!(factor <= taps.len(), "factor must not exceed the tap count");
        let mut rev_taps = taps.to_vec();
        rev_taps.reverse();
        Self {
            rev_taps,
            factor,
            // Zero pre-history: the long-run output count then tracks input/factor exactly
            // instead of running a constant filter-length deficit behind it.
            buf: vec![T::zero(); taps.len() - 1],
        }
    }

    pub(crate) fn process(&mut self, input: &[T], out: &mut Vec<T>) {
        out.clear();
        self.buf.extend_from_slice(input);
        let k = self.rev_taps.len();
        let mut pos = 0;
        while pos + k <= self.buf.len() {
            let mut acc = T::zero();
            for (&x, &c) in self.buf[pos..pos + k].iter().zip(&self.rev_taps) {
                acc = acc + x * c;
            }
            out.push(acc);
            pos += self.factor;
        }
        self.buf.drain(..pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_db(h: &[f32], freq: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (k, &v) in h.iter().enumerate() {
            let w = -2.0 * PI * freq * k as f64;
            re += f64::from(v) * w.cos();
            im += f64::from(v) * w.sin();
        }
        10.0 * (re * re + im * im).log10()
    }

    #[test]
    fn dc_gain_is_unity() {
        let h = design_lowpass(129, 0.1);
        let sum: f64 = h.iter().map(|&v| f64::from(v)).sum();
        assert!((sum - 1.0).abs() < 1e-6, "dc gain {sum}");
    }

    #[test]
    fn passband_ripple_under_half_db() {
        let h = design_lowpass(129, 0.1);
        // The Blackman transition half-width is 2.75/129 ≈ 0.021; 0.075 stays clear of it.
        for i in 0..=150 {
            let f = 0.075 * i as f64 / 150.0;
            let db = response_db(&h, f);
            assert!(db.abs() < 0.5, "ripple {db} dB at f={f}");
        }
    }

    #[test]
    fn stopband_below_minus_50_db_beyond_1_5x_cutoff() {
        let h = design_lowpass(129, 0.1);
        for i in 0..=350 {
            let f = 0.15 + (0.5 - 0.15) * i as f64 / 350.0;
            let db = response_db(&h, f);
            assert!(db < -50.0, "stopband leak {db} dB at f={f}");
        }
    }
}
