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

/// Design a linear-phase bandpass by modulating a lowpass prototype to `center`.
/// `low`/`high` are the −6 dB edges normalized to the sample rate (`0 < low < high < 0.5`).
/// Passband gain is unity only while the band clears DC and Nyquist by the prototype's
/// transition width — closer in, the negative-frequency image adds and the gain walks toward 2.
#[must_use]
pub fn design_bandpass(taps: usize, low: f64, high: f64) -> Vec<f32> {
    assert!(
        low > 0.0 && low < high && high < 0.5,
        "band edges must satisfy 0 < low < high < 0.5"
    );
    let prototype = design_lowpass(taps, (high - low) / 2.0);
    let (band_center, center) = ((low + high) / 2.0, (taps - 1) as f64 / 2.0);
    prototype
        .iter()
        .enumerate()
        .map(|(k, &v)| {
            // The cosine splits the prototype into ±band_center images at half amplitude each;
            // doubling restores unity in the (positive-frequency) passband.
            let m = (2.0 * PI * band_center * (k as f64 - center)).cos();
            (2.0 * f64::from(v) * m) as f32
        })
        .collect()
}

/// Gaussian pulse-shaping / matched filter for GMSK (AIS): `sps` samples per symbol, `bt` the
/// bandwidth-time product (0.4 for AIS/GMSK), `span` symbol periods. The tap count is
/// `span·sps` rounded up to odd so the pulse has a true center tap. Normalized to unity DC gain.
#[must_use]
pub fn design_gaussian(sps: f64, bt: f64, span: usize) -> Vec<f32> {
    assert!(sps > 1.0, "need more than one sample per symbol");
    assert!(bt > 0.0, "bandwidth-time product must be positive");
    assert!(span >= 1, "span must cover at least one symbol");
    let mut taps = (span as f64 * sps).round() as usize;
    if taps.is_multiple_of(2) {
        taps += 1;
    }
    assert!(taps >= 3, "need at least 3 taps");
    // Gaussian σ in symbol periods for a −3 dB bandwidth of `bt/T` (ITU-R M.1371 shaping).
    let sigma = (2.0f64.ln()).sqrt() / (2.0 * PI * bt);
    let center = (taps - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..taps)
        .map(|k| {
            let t = (k as f64 - center) / sps;
            (-t * t / (2.0 * sigma * sigma)).exp()
        })
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
    use crate::testutil::{real_tone, rms_r};

    /// Steady-state amplitude gain of `h` at `freq`, measured on a real tone (unit-amplitude
    /// sine, so RMS 1/√2) with the filter's transient skipped.
    fn tone_gain(h: &[f32], freq: f64) -> f32 {
        let mut fir = StreamFir::<f32, f32>::new(h, 1);
        let mut out = Vec::new();
        fir.process(&real_tone(freq, 16_384), &mut out);
        rms_r(&out[h.len()..]) * std::f32::consts::SQRT_2
    }

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

    #[test]
    fn bandpass_passes_its_band_and_rejects_outside() {
        // 255 taps → Blackman transition half-width 2.75/255 ≈ 0.011, so the passband is flat
        // over 0.031..0.049 and the stopband is reached below 0.009 / above 0.071.
        let h = design_bandpass(255, 0.02, 0.06);
        for &f in &[0.031, 0.04, 0.049] {
            let gain = tone_gain(&h, f);
            assert!((0.9..1.1).contains(&gain), "passband gain {gain} at f={f}");
        }
        for &f in &[0.002, 0.008, 0.12, 0.45] {
            let gain = tone_gain(&h, f);
            assert!(gain < 0.01, "stopband gain {gain} at f={f}");
        }
    }

    #[test]
    fn bandpass_is_symmetric() {
        let h = design_bandpass(129, 0.05, 0.15);
        for k in 0..h.len() / 2 {
            let mirrored = h[h.len() - 1 - k];
            assert!((h[k] - mirrored).abs() < 1e-7, "asymmetric at tap {k}");
        }
    }

    #[test]
    fn gaussian_length_rounds_up_to_odd() {
        assert_eq!(design_gaussian(10.0, 0.4, 4).len(), 41);
        assert_eq!(design_gaussian(8.0, 0.4, 3).len(), 25);
        assert_eq!(design_gaussian(5.0, 0.5, 3).len(), 15);
    }

    #[test]
    fn gaussian_is_unit_gain_symmetric_and_unimodal() {
        let h = design_gaussian(10.0, 0.4, 4);
        let sum: f64 = h.iter().map(|&v| f64::from(v)).sum();
        assert!((sum - 1.0).abs() < 1e-6, "dc gain {sum}");

        let mid = h.len() / 2;
        for k in 0..mid {
            assert!(
                (h[k] - h[h.len() - 1 - k]).abs() < 1e-9,
                "asymmetric at tap {k}"
            );
            assert!(
                h[k] < h[k + 1],
                "not increasing toward the center at tap {k}"
            );
        }
        assert!(h[mid] > 0.0, "center tap must be the peak");
    }
}
