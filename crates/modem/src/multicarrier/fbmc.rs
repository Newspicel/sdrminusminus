//! FBMC/OQAM — filter-bank multicarrier with offset QAM (MODEM-PLAN §3.1 `multicarrier/`, §7
//! phase 9).
//!
//! **No prefix, no rectangle, and no complex orthogonality.** FBMC shapes every subcarrier with a
//! long prototype filter — four symbol periods here, the PHYDYAS design's overlapping factor — so
//! its spectrum is the filter's and not a sinc's, and it spends nothing at all on a guard
//! interval. What it gives up in exchange is the thing that makes that possible: **a filter that
//! well localised cannot be orthogonal in the complex field**, by the Balian–Low theorem, so FBMC
//! is orthogonal only in the *real* field and the waveform carries the real and imaginary halves
//! of each point half a symbol period apart. That is the "offset QAM" in the name, and it is not a
//! detail — it is the whole structure:
//!
//! - Each subcarrier carries one **real** symbol every `M/2` samples instead of one complex symbol
//!   every `M`, so the rate is identical to OFDM-without-prefix.
//! - Each is multiplied by `j^{m+n}`, which puts a neighbour's leakage on the *imaginary* axis of
//!   the wanted symbol — where taking the real part discards it. The interference is not small;
//!   it is orthogonal, which is stronger.
//! - A receiver that took the magnitude, or that recovered the two halves together, would read
//!   that leakage as noise. Taking `Re` after de-rotating by the same `j^{m+n}` is the entry.
//!
//! **The acceptance is its constellation's own oracle.** The prototype is unit energy and the map
//! from points to samples is orthogonal in the real field, so under AWGN FBMC can be neither
//! better nor worse than the constellation it carries; every dB a committed curve sits from that
//! oracle is the frame's own tail overhead or a defect, and the entry is gated on which.

use std::f64::consts::TAU;

use num_complex::Complex;

/// The PHYDYAS prototype's frequency coefficients at overlapping factor 4 (Bellanger, *FBMC
/// physical layer: a primer*, PHYDYAS 2010 §2.1). `H0 = 1` is implicit.
const PHYDYAS_K4: [f64; 3] = [0.971_960, std::f64::consts::FRAC_1_SQRT_2, 0.235_147];

/// Largest transform an FBMC entry will build kernels for.
pub const MAX_SUBCARRIERS: usize = 1_024;

/// The waveform as data (§3.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FbmcParams {
    /// Subcarriers in the bank — the `M` that sets the symbol period.
    pub subcarriers: usize,
    /// Contiguous allocated subcarriers, symmetric about the carrier and skipping DC.
    pub allocated: usize,
}

impl FbmcParams {
    /// Overlapping factor. Fixed at 4: [`PHYDYAS_K4`] is the coefficient set that makes the
    /// prototype's real-field orthogonality hold, and a different factor is a different filter
    /// rather than a different value of this one.
    pub const OVERLAP: usize = 4;

    /// The reference configuration the catalog measures: a 64-subcarrier bank with 48 allocated
    /// — the same count `ofdm/`'s 802.11a/g-like row carries, so the two frameworks' curves are
    /// directly comparable.
    #[must_use]
    pub fn reference() -> Self {
        Self {
            subcarriers: 64,
            allocated: 48,
        }
    }

    /// Prototype length: `K·M`.
    #[must_use]
    pub fn prototype_len(&self) -> usize {
        Self::OVERLAP * self.subcarriers
    }

    /// Samples between consecutive OQAM slots — half a symbol period, which is what "offset"
    /// means.
    #[must_use]
    pub fn slot_stride(&self) -> usize {
        self.subcarriers / 2
    }

    /// Samples a frame of `points` complex points occupies. Two OQAM slots per point per
    /// subcarrier, plus the prototype's own tail.
    #[must_use]
    pub fn frame_samples(&self, points: usize) -> usize {
        let slots = 2 * points / self.allocated;
        (slots.max(1) - 1) * self.slot_stride() + self.prototype_len()
    }

    /// The PHYDYAS prototype, unit energy.
    ///
    /// # Panics
    /// If the bank is empty, past [`MAX_SUBCARRIERS`], or odd.
    #[must_use]
    pub fn prototype(&self) -> Vec<f64> {
        assert!(
            (2..=MAX_SUBCARRIERS).contains(&self.subcarriers) && self.subcarriers.is_multiple_of(2),
            "an FBMC bank runs an even 2..={MAX_SUBCARRIERS} subcarriers"
        );
        let len = self.prototype_len();
        let mut p: Vec<f64> = (0..len)
            .map(|n| {
                let mut acc = 1.0;
                for (k, &h) in PHYDYAS_K4.iter().enumerate() {
                    let order = (k + 1) as f64;
                    let sign = if (k + 1) % 2 == 0 { 1.0 } else { -1.0 };
                    acc += 2.0 * sign * h * (TAU * order * n as f64 / len as f64).cos();
                }
                acc
            })
            .collect();
        let scale = p.iter().map(|v| v * v).sum::<f64>().sqrt().recip();
        for v in &mut p {
            *v *= scale;
        }
        p
    }

    /// Transform bin of the `index`-th allocated subcarrier: symmetric about the carrier, DC
    /// never allocated — the same layout `ufmc` uses, so the two entries' spectra are comparable.
    #[must_use]
    pub fn bin(&self, index: usize) -> usize {
        let half = (self.allocated / 2) as isize;
        let offset = index as isize - half;
        let signed = if offset < 0 { offset } else { offset + 1 };
        signed.rem_euclid(self.subcarriers as isize) as usize
    }

    /// `p[u]·e^{j2πm(u − D)/M}` for every allocated subcarrier, `D` the prototype's own delay —
    /// the per-subcarrier kernel both ends convolve with, built once so the transmitter and the
    /// receiver cannot disagree about a phase reference.
    #[must_use]
    fn kernels(&self) -> Vec<Vec<Complex<f32>>> {
        let p = self.prototype();
        let len = self.prototype_len();
        // The PHYDYAS prototype's own centre, which is `KM/2` and *not* `(KM−1)/2`: the design
        // puts `p[0] = 0` and is symmetric about the half-length, so referencing the carrier
        // half a sample away leaves a residual the real projection cannot discard — measured as
        // the difference between a bank that round-trips and one that does not.
        let delay = (len / 2) as f64;
        (0..self.allocated)
            .map(|index| {
                let m = self.bin(index) as f64;
                (0..len)
                    .map(|u| {
                        let phase = TAU * m * (u as f64 - delay) / self.subcarriers as f64;
                        let z = Complex::from_polar(p[u], phase);
                        Complex::new(z.re as f32, z.im as f32)
                    })
                    .collect()
            })
            .collect()
    }

    /// The OQAM phase of slot `n` on the `index`-th allocated subcarrier: `j^{m+n}` times the
    /// `(−1)^{mn}` that referencing the carrier at the pulse's own origin introduces. Together
    /// they are what puts a neighbour's leakage on the axis `Re` discards.
    fn oqam_phase(&self, index: usize, slot: usize) -> Complex<f32> {
        let m = self.bin(index);
        let quarter = (m + slot) % 4;
        let base = [
            Complex::new(1.0f32, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(-1.0, 0.0),
            Complex::new(0.0, -1.0),
        ][quarter];
        if (m * slot).is_multiple_of(2) {
            base
        } else {
            -base
        }
    }
}

/// The transmitter.
#[derive(Clone)]
pub struct FbmcMod {
    params: FbmcParams,
    kernels: Vec<Vec<Complex<f32>>>,
}

impl FbmcMod {
    #[must_use]
    pub fn new(params: FbmcParams) -> Self {
        Self {
            kernels: params.kernels(),
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &FbmcParams {
        &self.params
    }

    /// Appends one frame to `out`: `points` read symbol-major, each contributing its real part to
    /// one OQAM slot and its imaginary part to the next.
    pub fn modulate(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let allocated = self.params.allocated;
        let stride = self.params.slot_stride();
        let start = out.len();
        out.resize(
            start + self.params.frame_samples(points.len()),
            Complex::new(0.0, 0.0),
        );
        for (symbol, chunk) in points.chunks_exact(allocated).enumerate() {
            for (index, &point) in chunk.iter().enumerate() {
                let kernel = &self.kernels[index];
                for (half, value) in [point.re, point.im].into_iter().enumerate() {
                    let slot = 2 * symbol + half;
                    let phase = self.params.oqam_phase(index, slot) * value;
                    let base = start + slot * stride;
                    for (u, &tap) in kernel.iter().enumerate() {
                        out[base + u] += phase * tap;
                    }
                }
            }
        }
    }
}

/// The receiver: the same kernels, de-rotated, and the real part.
#[derive(Clone)]
pub struct FbmcDemod {
    params: FbmcParams,
    kernels: Vec<Vec<Complex<f32>>>,
}

impl FbmcDemod {
    #[must_use]
    pub fn new(params: FbmcParams) -> Self {
        Self {
            kernels: params.kernels(),
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &FbmcParams {
        &self.params
    }

    /// Appends `symbols · allocated` points to `out`.
    pub fn demodulate(&mut self, x: &[Complex<f32>], symbols: usize, out: &mut Vec<Complex<f32>>) {
        let (allocated, stride) = (self.params.allocated, self.params.slot_stride());
        let len = self.params.prototype_len();
        for symbol in 0..symbols {
            for index in 0..allocated {
                let kernel = &self.kernels[index];
                let mut halves = [0.0f32; 2];
                for (half, slot_value) in halves.iter_mut().enumerate() {
                    let slot = 2 * symbol + half;
                    let base = slot * stride;
                    if base + len > x.len() {
                        continue;
                    }
                    let mut acc = Complex::new(0.0f32, 0.0);
                    for (u, &tap) in kernel.iter().enumerate() {
                        acc += x[base + u] * tap.conj();
                    }
                    *slot_value = (acc * self.params.oqam_phase(index, slot).conj()).re;
                }
                out.push(Complex::new(halves[0], halves[1]));
            }
        }
    }
}

/// The prototype's own out-of-band roll-off, in dB, at `offset` subcarrier spacings from its
/// centre — what FBMC's filter buys, computed from the taps rather than measured through a frame.
#[must_use]
pub fn prototype_response_db(params: &FbmcParams, offset: f64) -> f64 {
    let p = params.prototype();
    let m = params.subcarriers as f64;
    let at = |f: f64| {
        p.iter()
            .enumerate()
            .map(|(n, &tap)| Complex::from_polar(tap, -TAU * f * n as f64 / m))
            .sum::<Complex<f64>>()
            .norm_sqr()
    };
    10.0 * (at(offset) / at(0.0)).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(count: usize) -> Vec<Complex<f32>> {
        let mut state = 0x0_fb3cu32;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let i = if state & 1 == 0 { 1.0 } else { -1.0 };
                let q = if state & 2 == 0 { 1.0 } else { -1.0 };
                Complex::new(i, q) / 2f32.sqrt()
            })
            .collect()
    }

    /// The prototype is unit energy, symmetric, and rolls off the way a filter designed in the
    /// frequency domain should — the three things the rest of the entry rests on.
    #[test]
    fn the_phydyas_prototype_is_unit_energy_and_symmetric() {
        let params = FbmcParams::reference();
        let p = params.prototype();
        assert_eq!(p.len(), 256);
        let energy: f64 = p.iter().map(|v| v * v).sum();
        assert!((energy - 1.0).abs() < 1e-12, "energy {energy}");
        // Symmetric about its own centre (the n = 0 tap is the design's zero and stands alone).
        for n in 1..p.len() / 2 {
            assert!(
                (p[n] - p[p.len() - n]).abs() < 1e-12,
                "asymmetric at tap {n}"
            );
        }
        // Its stopband is what the waveform is for: an adjacent-but-one subcarrier is deep down.
        let one_away = prototype_response_db(&params, 1.0);
        let two_away = prototype_response_db(&params, 2.0);
        assert!(one_away < -20.0, "one spacing away: {one_away} dB");
        assert!(two_away < -35.0, "two spacings away: {two_away} dB");
    }

    /// Real-field orthogonality, which is the entry: a noiseless frame comes back point for
    /// point, even though the subcarriers overlap heavily and the slots overlap by four symbol
    /// periods.
    #[test]
    fn a_noiseless_frame_round_trips() {
        let params = FbmcParams::reference();
        let symbols = 16;
        let sent = points(symbols * params.allocated);
        let mut wave = Vec::new();
        FbmcMod::new(params).modulate(&sent, &mut wave);
        assert_eq!(wave.len(), params.frame_samples(sent.len()));
        let mut got = Vec::new();
        FbmcDemod::new(params).demodulate(&wave, symbols, &mut got);
        assert_eq!(got.len(), sent.len());
        // Interior symbols only: the first and last slots are missing half their overlap, which
        // is the ramp every FBMC frame pays and the reason a real one sends guard symbols.
        let interior = params.allocated * 4..sent.len() - params.allocated * 4;
        let worst = got[interior.clone()]
            .iter()
            .zip(&sent[interior])
            .map(|(a, b)| f64::from((a - b).norm()))
            .fold(0.0f64, f64::max);
        assert!(worst < 0.02, "worst interior error {worst}");
    }

    /// The waveform carries the energy its points carry, which is what makes the entry's Eb/N0
    /// the same quantity as the linear engine's.
    #[test]
    fn the_frame_carries_the_energy_of_its_points() {
        let params = FbmcParams::reference();
        let symbols = 32;
        let sent = points(symbols * params.allocated);
        let mut wave = Vec::new();
        FbmcMod::new(params).modulate(&sent, &mut wave);
        let wave_energy: f64 = wave.iter().map(|v| f64::from(v.norm_sqr())).sum();
        let point_energy: f64 = sent.iter().map(|v| f64::from(v.norm_sqr())).sum();
        assert!(
            (wave_energy / point_energy - 1.0).abs() < 0.05,
            "waveform {wave_energy}, points {point_energy}"
        );
    }

    /// The offset is what makes it work, so removing it must break it: sending both halves of a
    /// point in the same slot destroys the real-field orthogonality the entry depends on.
    #[test]
    fn without_the_half_symbol_offset_the_bank_is_not_orthogonal() {
        let params = FbmcParams::reference();
        let kernels = params.kernels();
        // Two neighbouring subcarriers, same slot: the complex inner product is not zero — that
        // is why a complex-orthogonal reading fails — but its *real part* is.
        let inner: Complex<f64> = kernels[10]
            .iter()
            .zip(&kernels[11])
            .map(|(a, b)| {
                Complex::new(f64::from(a.re), f64::from(a.im))
                    * Complex::new(f64::from(b.re), f64::from(b.im)).conj()
            })
            .sum();
        let phase = params.oqam_phase(10, 0) * params.oqam_phase(11, 0).conj();
        let projected = inner * Complex::new(f64::from(phase.re), f64::from(phase.im));
        assert!(inner.norm() > 0.1, "neighbours do not overlap: {inner}");
        assert!(
            projected.re.abs() < 1e-3,
            "the real part is not orthogonal: {}",
            projected.re
        );
    }
}
