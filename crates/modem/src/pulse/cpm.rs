//! The finite-support pulse family: CPM frequency pulses (rect/LREC, LRC, Gaussian) and the
//! closed-support amplitude pulses (half-sine, the Gaussian premod filter). Naming and
//! formulas follow Anderson, Aulin & Sundberg, *Digital Phase Modulation* — the source of the
//! LREC/LRC vocabulary and of the phase-pulse convention `q(∞) = ½` that [`phase_pulse`]
//! implements.
//!
//! One pulse, two readings: whether a tap vector here acts as a *frequency* pulse (scaling
//! instantaneous frequency inside a CPM modulator) or an *amplitude* pulse (shaping a linear
//! waveform) is the consumer's choice, and [`Norm`] is how the consumer says it — Area for the
//! frequency reading (the integral fixes the phase step), Energy for the amplitude reading
//! (the integral of h² fixes the symbol energy). That is why every constructor takes [`Norm`]
//! rather than baking one in.

use std::f64::consts::PI;

use sdrmm_dsp::design_gaussian;

use super::{Norm, normalise, renorm_designed};

/// Samples a shape given on the normalised support `u ∈ (0, 1)` at interval midpoints,
/// `u = (k + ½)/n` with `n = round(L·sps)` taps: exactly even-symmetric for symmetric shapes
/// at any tap count, and no zero endpoint taps spent on shapes that vanish at the edges.
fn midpoint_taps(sps: f64, l: usize, shape: impl Fn(f64) -> f64) -> Vec<f64> {
    assert!(sps > 1.0, "need more than one sample per symbol");
    assert!(l >= 1, "pulse must cover at least one symbol");
    let n = (l as f64 * sps).round() as usize;
    assert!(
        n >= 2,
        "pulse needs at least two samples across its support"
    );
    (0..n).map(|k| shape((k as f64 + 0.5) / n as f64)).collect()
}

/// Rectangular pulse over one symbol — 1REC: the frequency pulse of plain CPFSK and of MSK
/// (h = ½), and the shape behind every unfiltered FSK mode in the catalog (POCSAG, RTTY,
/// navtex). Under [`Norm::Energy`] it is the integrate-and-dump matched filter.
///
/// Implemented as [`lrec`] with L = 1 — the identity `LREC(1) = rect` holds by construction,
/// not by parallel code.
#[must_use]
pub fn rect(sps: f64, norm: Norm) -> Vec<f32> {
    lrec(sps, 1, norm)
}

/// LREC(L): the rectangular partial-response CPM frequency pulse, `g(t) = const` over
/// `[0, L·T]` (Anderson/Aulin/Sundberg). L = 1 is CPFSK/MSK; L > 1 trades bandwidth for a
/// controlled-ISI phase trellis the MLSE tier resolves.
#[must_use]
pub fn lrec(sps: f64, l: usize, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, l, |_| 1.0), norm)
}

/// LRC(L): the raised-cosine *frequency* pulse `g(t) ∝ 1 − cos(2πt/(L·T))` over `[0, L·T]`
/// (Anderson/Aulin/Sundberg) — a namesake of, and not to be confused with, the Nyquist
/// raised-cosine *amplitude* pulse in `nyquist.rs`: LRC shapes a CPM phase trajectory, has no
/// zero-ISI property, and its smooth edges buy spectral roll-off. L = 1 appears as
/// full-response 1RC in the CPM literature; L ≥ 2 is the classic partial-response smoothing.
#[must_use]
pub fn lrc(sps: f64, l: usize, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, l, |u| 1.0 - (2.0 * PI * u).cos()), norm)
}

/// Half-sine over one symbol, `h(t) = sin(πt/T)` on `[0, T]` — MSK's pulse seen from either
/// side: as the amplitude pulse of MSK's linear OQPSK representation (Pasupathy, "Minimum
/// Shift Keying: A Spectrally Efficient Modulation", IEEE Comm. Mag. 1979) and as the chip
/// shape of IEEE 802.15.4 O-QPSK. [`Norm::Energy`] for those linear uses; [`Norm::Area`]
/// reads it as a full-response CPM frequency pulse (1HCS in parts of the literature).
#[must_use]
pub fn half_sine(sps: f64, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, 1, |u| (PI * u).sin()), norm)
}

/// Gaussian premodulation filter — the *amplitude* lowpass a GFSK transmitter runs its NRZ
/// data through: Bluetooth BR (BT = 0.5), AIS per ITU-R M.1371 (BT = 0.4), D-STAR (BT = 0.5).
///
/// Wraps `sdrmm_dsp::design_gaussian` (§1 minimal duplication): under [`Norm::Area`] the taps
/// are its output bit for bit. Inherits its span convention — `span` is the *total* length in
/// symbols, `span·sps` taps rounded up to odd — unlike the each-side spans in `nyquist.rs`;
/// the asymmetry is dsp's and is kept so the wrap stays exact.
#[must_use]
pub fn gaussian(sps: f64, bt: f64, span: usize, norm: Norm) -> Vec<f32> {
    renorm_designed(design_gaussian(sps, bt, span), norm)
}

/// The GMSK frequency pulse: a one-symbol rect convolved with the Gaussian premod filter
/// (Murota & Hirade, "GMSK Modulation for Digital Mobile Radio Telephony", IEEE Trans. Comm.
/// 1981 — their closed Q-function form *is* this convolution). GSM uses BT = 0.3
/// (3GPP TS 45.004), AIS BT = 0.4. Built literally as that convolution over the wrapped
/// [`gaussian`] design, so the Gaussian math exists once (§1).
///
/// Under [`Norm::Area`] this is the CPM engine's phase-3 contract: [`phase_pulse`] of these
/// taps reaches q = ½, so a symbol advances the carrier phase by exactly π·h. `span` is the
/// *total* length of g in symbols (the CPM L; ≥ 2 because the rect alone takes one symbol
/// and the Gaussian smoothing the rest). Lower BT ⇒ narrower spectrum ⇒ a *longer, flatter*
/// g(t): more of each symbol's phase step leaks into its neighbours, which is the ISI the
/// MLSE tier exists to absorb.
#[must_use]
pub fn gaussian_freq(sps: f64, bt: f64, span: usize, norm: Norm) -> Vec<f32> {
    assert!(
        span >= 2,
        "total span must exceed the rect's own symbol: {span}"
    );
    let smoothing = design_gaussian(sps, bt, span - 1);
    let n_rect = sps.round() as usize;
    assert!(n_rect >= 2, "need at least two samples per symbol");
    let mut g = vec![0.0f64; smoothing.len() + n_rect - 1];
    for (i, &s) in smoothing.iter().enumerate() {
        for slot in &mut g[i..i + n_rect] {
            *slot += f64::from(s);
        }
    }
    normalise(g, norm)
}

/// The CPM phase pulse `q(t) = ∫₀ᵗ g(τ) dτ` in the Aulin/Sundberg normalisation `q(∞) = ½`:
/// `q[n] = ½·Σ_{k≤n} g[k]`, so a unit-area frequency pulse reaches exactly ½ and the phase
/// step per symbol is `2πh·q(∞) = πh`. The ½ lives here, once, rather than in every
/// frequency-pulse table — handing the CPM engine a pulse that is *not* unit-area produces a
/// proportionally wrong modulation index, which is why the Area tests pin every pulse above.
#[must_use]
pub fn phase_pulse(freq: &[f32]) -> Vec<f32> {
    let mut acc = 0.0f64;
    freq.iter()
        .map(|&g| {
            acc += f64::from(g);
            (0.5 * acc) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §7 phase-2 cross-check. Exact equality, not tolerance: both sides are the same code
    /// path, and this pins the identity through any future refactor.
    #[test]
    fn lrec_of_one_symbol_is_rect() {
        for sps in [4.0, 8.0, 12.5] {
            for norm in [Norm::Energy, Norm::Area] {
                assert_eq!(lrec(sps, 1, norm), rect(sps, norm), "sps={sps} {norm:?}");
            }
        }
    }

    /// §7 phase-2 cross-check: half-sine against exact closed forms. On the midpoint grid the
    /// Lagrange identities are exact, not asymptotic: Σ sin²(π(k+½)/n) = n/2 and
    /// Σ sin(π(k+½)/n) = 1/sin(π/(2n)) — so each normalisation's scale factor is checked
    /// pointwise in closed form.
    #[test]
    fn half_sine_energy_and_area_match_the_closed_forms() {
        for n in [8usize, 5] {
            let energy = half_sine(n as f64, Norm::Energy);
            let area = half_sine(n as f64, Norm::Area);
            assert_eq!(energy.len(), n);
            let energy_scale = (n as f64 / 2.0).sqrt().recip();
            let area_scale = (PI / (2.0 * n as f64)).sin();
            for k in 0..n {
                let raw = (PI * (k as f64 + 0.5) / n as f64).sin();
                let e = f64::from(energy[k]);
                let a = f64::from(area[k]);
                assert!((e - raw * energy_scale).abs() < 1e-6, "n={n} tap {k}: {e}");
                assert!((a - raw * area_scale).abs() < 1e-6, "n={n} tap {k}: {a}");
            }
        }
    }

    /// The wrap is a wrap — same bits as `design_gaussian`, mirroring the RRC test.
    #[test]
    fn gaussian_under_area_norm_is_bit_identical_to_design_gaussian() {
        for (sps, bt, span) in [(8.0, 0.5, 3), (10.0, 0.4, 4), (5.0, 0.3, 3)] {
            assert_eq!(
                gaussian(sps, bt, span, Norm::Area),
                design_gaussian(sps, bt, span),
                "sps={sps} bt={bt} span={span}"
            );
        }
    }

    /// §7 phase-2 cross-check: the documented narrowing. Lower BT means a narrower spectrum
    /// and therefore a longer, flatter g(t) — lower peak, less of the pulse inside its own
    /// symbol. Both orderings asserted at equal length so the shapes are truly comparable.
    #[test]
    fn gaussian_freq_at_bt_half_is_narrower_in_time_than_bt_point_three() {
        let sps = 10usize;
        let sharp = gaussian_freq(sps as f64, 0.5, 4, Norm::Area);
        let smooth = gaussian_freq(sps as f64, 0.3, 4, Norm::Area);
        assert_eq!(sharp.len(), smooth.len());
        assert_eq!(sharp.len(), 4 * sps);
        let peak = |taps: &[f32]| taps.iter().fold(0.0f32, |m, &x| m.max(x));
        assert!(peak(&sharp) > peak(&smooth), "peak ordering");
        // Fraction of the (unit) area inside the central symbol period.
        let central = |taps: &[f32]| {
            let lo = taps.len() / 2 - sps / 2;
            taps[lo..lo + sps]
                .iter()
                .map(|&x| f64::from(x))
                .sum::<f64>()
        };
        assert!(central(&sharp) > central(&smooth), "concentration ordering");
    }

    /// §7 phase-2 acceptance: every frequency pulse under Area normalisation drives the phase
    /// pulse to q = ½, by cumulative sum — and monotonically, since none of these shapes has
    /// negative lobes to swing the instantaneous frequency the wrong way mid-symbol.
    #[test]
    fn area_normalised_frequency_pulses_reach_q_of_one_half() {
        let pulses: Vec<(&str, Vec<f32>)> = vec![
            ("rect", rect(8.0, Norm::Area)),
            ("half_sine", half_sine(8.0, Norm::Area)),
            ("lrec(2)", lrec(8.0, 2, Norm::Area)),
            ("lrec(3)", lrec(8.0, 3, Norm::Area)),
            ("lrc(2)", lrc(8.0, 2, Norm::Area)),
            ("lrc(3)", lrc(8.0, 3, Norm::Area)),
            (
                "gaussian_freq BT=0.3",
                gaussian_freq(8.0, 0.3, 4, Norm::Area),
            ),
            (
                "gaussian_freq BT=0.5",
                gaussian_freq(8.0, 0.5, 3, Norm::Area),
            ),
        ];
        for (name, g) in pulses {
            let q = phase_pulse(&g);
            let last = f64::from(*q.last().unwrap());
            assert!((last - 0.5).abs() < 1e-5, "{name}: q(∞) = {last}");
            for (k, w) in q.windows(2).enumerate() {
                assert!(w[1] >= w[0], "{name}: q not monotone at {k}");
            }
            assert!(q[0] >= 0.0, "{name}: q starts negative");
        }
    }
}
