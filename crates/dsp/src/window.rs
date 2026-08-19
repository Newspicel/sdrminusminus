use std::f32::consts::PI;

#[must_use]
pub fn hann(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos()))
        .collect()
}

/// Four-term Blackman–Harris. Its main lobe is wider than Hann's, which is the price for
/// sidelobes near -92 dB — the difference between reading a weak bearing next to a strong signal
/// and reading the strong signal's skirt.
#[must_use]
pub fn blackman_harris(n: usize) -> Vec<f32> {
    const A: [f32; 4] = [0.358_75, 0.488_29, 0.141_28, 0.011_68];
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }
    (0..n)
        .map(|i| {
            let w = 2.0 * PI * i as f32 / n as f32;
            A[0] - A[1] * w.cos() + A[2] * (2.0 * w).cos() - A[3] * (3.0 * w).cos()
        })
        .collect()
}

#[must_use]
pub fn coherent_gain(window: &[f32]) -> f32 {
    window.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_are_zero_and_center_is_one() {
        let w = hann(8);
        assert!(w[0].abs() < 1e-6);
        assert!((w[4] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hann_coherent_gain_is_half_n() {
        let w = hann(1024);
        assert!((coherent_gain(&w) - 512.0).abs() < 1e-2);
    }

    #[test]
    fn blackman_harris_tapers_to_almost_nothing_and_peaks_in_the_middle() {
        let w = blackman_harris(64);
        assert!(w[0].abs() < 1e-4, "{}", w[0]);
        assert!((w[32] - 1.0).abs() < 1e-3, "{}", w[32]);
        assert!(w.windows(2).take(32).all(|pair| pair[1] >= pair[0]));
    }

    #[test]
    fn blackman_harris_sidelobes_sit_far_below_hann() {
        let sidelobe = |window: &[f32]| -> f32 {
            let n = window.len();
            let bin = |k: f32| -> f32 {
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (i, w) in window.iter().enumerate() {
                    let phase = -2.0 * PI * k * i as f32 / n as f32;
                    re += w * phase.cos();
                    im += w * phase.sin();
                }
                re.hypot(im)
            };
            let peak = bin(0.0);
            let mut worst = 0.0f32;
            let mut k = 6.0;
            while k < n as f32 / 2.0 {
                worst = worst.max(bin(k));
                k += 0.5;
            }
            20.0 * (worst / peak).log10()
        };
        let hann_db = sidelobe(&hann(256));
        let bh_db = sidelobe(&blackman_harris(256));
        assert!(bh_db < hann_db - 30.0, "hann {hann_db} dB, bh {bh_db} dB");
    }
}
