//! GFDM — generalised frequency division multiplexing ( §3.1 `multicarrier/`, §7 phase
//! 9).
//!
//! **A block of `K` subcarriers by `M` subsymbols, pulse-shaped circularly.** Where OFDM sends one
//! symbol per subcarrier per transform and pays a cyclic prefix for each, GFDM sends `M` of them
//! and pays one prefix for the block — and shapes each with a circularly-shifted prototype pulse,
//! so the spectrum falls away at the band edges instead of sitting under a rectangle's sinc
//! skirts. Both of those are the point of the waveform, and both cost the same thing:
//!
//! **GFDM is not orthogonal, and that is the entry.** A pulse narrow enough in frequency to buy
//! the out-of-band roll-off is wider than one subsymbol in time, so subsymbols overlap and
//! subcarriers overlap, by construction. The transmitter is a dense `N × N` matrix `A` with
//! `N = K·M`, and the two receivers are the two ways of dealing with a matrix that is not unitary:
//!
//! - [`GfdmDetector::ZeroForcing`] — `A⁻¹`. Removes the self-interference exactly and amplifies
//!   the noise by however badly `A` is conditioned. Tier 1, because it is the one whose curve can
//!   be read against a closed form at all.
//! - [`GfdmDetector::Matched`] — `Aᴴ`. Costs nothing, amplifies nothing, and leaves the
//!   self-interference in place as an error floor no amount of Eb/N0 removes.
//!
//! The two crossing over is the waveform's whole trade, and the entry commits both curves so the
//! crossing is a number rather than a claim.

use num_complex::Complex;

use super::transform::{invert, matvec};

/// Largest block a receiver will build a dense matrix for. `N = K·M` and the inverse is `N³` at
/// construction and `N²` per block, so this is a guard against a parameterisation that would
/// quietly turn a receiver into a linear-algebra benchmark.
pub const MAX_BLOCK: usize = 512;

/// The waveform as data (§3.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GfdmParams {
    /// Subcarriers per block.
    pub subcarriers: usize,
    /// Subsymbols per block — the `M` that distinguishes GFDM from OFDM, which is this at 1.
    pub subsymbols: usize,
    /// Root-raised-cosine roll-off of the circular prototype pulse. At 0 the pulse is a sinc
    /// over the block and the matrix is worst-conditioned; toward 1 it is well-conditioned and
    /// spectrally wide, which is the trade the roll-off *is*.
    pub rolloff: f64,
    /// Cyclic prefix in samples, prepended to the whole block rather than to each subsymbol —
    /// the saving that motivates the waveform.
    pub cp: usize,
}

impl GfdmParams {
    /// The 802.11-adjacent reference configuration the catalog measures: 16 subcarriers, 5
    /// subsymbols, roll-off 0.5.
    #[must_use]
    pub fn new(subcarriers: usize, subsymbols: usize, rolloff: f64) -> Self {
        Self {
            subcarriers,
            subsymbols,
            rolloff,
            cp: 0,
        }
    }

    /// Samples in one block before the prefix — `K·M`, and the number of points it carries.
    #[must_use]
    pub fn block(&self) -> usize {
        self.subcarriers * self.subsymbols
    }

    /// Samples one block occupies on the wire.
    #[must_use]
    pub fn samples(&self) -> usize {
        self.block() + self.cp
    }

    /// The circular prototype pulse, unit energy, centred at sample 0 so that the `m`-th
    /// subsymbol's copy is a plain rotation by `m·K`.
    ///
    /// # Panics
    /// If the block is empty or past [`MAX_BLOCK`].
    #[must_use]
    pub fn prototype(&self) -> Vec<f64> {
        let n = self.block();
        assert!(
            (1..=MAX_BLOCK).contains(&n),
            "a GFDM block runs 1..={MAX_BLOCK} samples, got {n}"
        );
        let k = self.subcarriers as f64;
        let mut g: Vec<f64> = (0..n)
            .map(|i| {
                // Circular distance from sample 0, in subsymbol periods.
                let half = n as f64 / 2.0;
                let offset = (i as f64 + half).rem_euclid(n as f64) - half;
                rrc(offset / k, self.rolloff)
            })
            .collect();
        let energy: f64 = g.iter().map(|v| v * v).sum();
        let scale = energy.sqrt().recip();
        for v in &mut g {
            *v *= scale;
        }
        g
    }

    /// The modulation matrix `A`, row-major, `N` rows of `N` columns. Column `k·M + m` is
    /// subcarrier `k`'s `m`-th subsymbol: the prototype rotated by `m·K` samples and mixed to
    /// subcarrier `k`.
    #[must_use]
    pub fn matrix(&self) -> Vec<Complex<f64>> {
        let (n, k_count, m_count) = (self.block(), self.subcarriers, self.subsymbols);
        let g = self.prototype();
        let mut a = vec![Complex::new(0.0, 0.0); n * n];
        for k in 0..k_count {
            for m in 0..m_count {
                let column = k * m_count + m;
                for sample in 0..n {
                    let shifted = (sample + n - m * k_count) % n;
                    let phase = std::f64::consts::TAU * k as f64 * sample as f64 / k_count as f64;
                    a[sample * n + column] = Complex::from_polar(g[shifted], phase);
                }
            }
        }
        a
    }
}

/// Root-raised-cosine amplitude at `t` symbol periods, roll-off `alpha`. Written here rather than
/// taken from `pulse/` because that module designs *sampled* filters of a stated span, and this
/// prototype is sampled circularly over a block whose length the waveform fixes.
fn rrc(t: f64, alpha: f64) -> f64 {
    use std::f64::consts::PI;
    if alpha <= 0.0 {
        return if t.abs() < 1e-12 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        };
    }
    if t.abs() < 1e-12 {
        return 1.0 + alpha * (4.0 / PI - 1.0);
    }
    let singular = 1.0 / (4.0 * alpha);
    if (t.abs() - singular).abs() < 1e-9 {
        let a = (1.0 + 2.0 / PI) * (PI / (4.0 * alpha)).sin();
        let b = (1.0 - 2.0 / PI) * (PI / (4.0 * alpha)).cos();
        return alpha / 2f64.sqrt() * (a + b);
    }
    let numerator =
        (PI * t * (1.0 - alpha)).sin() + 4.0 * alpha * t * (PI * t * (1.0 + alpha)).cos();
    numerator / (PI * t * (1.0 - (4.0 * alpha * t).powi(2)))
}

/// The transmitter: one dense product per block.
#[derive(Clone)]
pub struct GfdmMod {
    params: GfdmParams,
    matrix: Vec<Complex<f32>>,
    block: Vec<Complex<f32>>,
}

impl GfdmMod {
    /// # Panics
    /// If the prefix is longer than the block it prefixes — the block is what it is copied from,
    /// so a longer one has nothing to take, and `modulate` would underflow rather than say so.
    #[must_use]
    pub fn new(params: GfdmParams) -> Self {
        assert!(
            params.cp <= params.block(),
            "a cyclic prefix is a copy of the block's tail; {} samples of a {}-sample block is \
             not one",
            params.cp,
            params.block()
        );
        let matrix = params
            .matrix()
            .into_iter()
            .map(|v| Complex::new(v.re as f32, v.im as f32))
            .collect();
        Self {
            matrix,
            block: vec![Complex::new(0.0, 0.0); params.block()],
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &GfdmParams {
        &self.params
    }

    /// Appends `points.len() / block` blocks to `out`, each with its cyclic prefix. Points are
    /// read subcarrier-major within a block, which is the column order [`GfdmParams::matrix`]
    /// builds.
    pub fn modulate(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let n = self.params.block();
        for chunk in points.chunks_exact(n) {
            matvec(&self.matrix, n, n, chunk, &mut self.block);
            out.extend_from_slice(&self.block[n - self.params.cp..]);
            out.extend_from_slice(&self.block);
        }
    }
}

/// The two receivers (§5 item 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GfdmDetector {
    /// `A⁻¹` — exact, noise-amplifying. Tier 1.
    ZeroForcing,
    /// `Aᴴ` — free, self-interfering. Tier 2.
    Matched,
}

/// The receiver.
#[derive(Clone)]
pub struct GfdmDemod {
    params: GfdmParams,
    matrix: Vec<Complex<f32>>,
    /// Noise amplification per point: the row norms of the receive matrix, which for the
    /// zero-forcing tier is what its inverse costs and for the matched one is exactly 1.
    amplification: Vec<f32>,
    block: Vec<Complex<f32>>,
}

impl GfdmDemod {
    /// # Panics
    /// If the zero-forcing tier is asked for and the modulation matrix is singular — a prototype
    /// with no inverse is a parameterisation error, not a runtime condition.
    #[must_use]
    pub fn new(params: GfdmParams, detector: GfdmDetector) -> Self {
        let n = params.block();
        let mut a = params.matrix();
        let receive: Vec<Complex<f64>> = match detector {
            GfdmDetector::ZeroForcing => {
                assert!(
                    invert(&mut a, n).is_some(),
                    "the GFDM prototype at roll-off {} over a {}×{} block has no inverse",
                    params.rolloff,
                    params.subcarriers,
                    params.subsymbols
                );
                a
            }
            // Aᴴ: conjugate transpose, so row r of the receiver is column r of the transmitter.
            GfdmDetector::Matched => (0..n * n).map(|i| a[(i % n) * n + i / n].conj()).collect(),
        };
        let amplification = receive
            .chunks_exact(n)
            .map(|row| row.iter().map(|v| v.norm_sqr()).sum::<f64>() as f32)
            .collect();
        Self {
            matrix: receive
                .into_iter()
                .map(|v| Complex::new(v.re as f32, v.im as f32))
                .collect(),
            amplification,
            block: vec![Complex::new(0.0, 0.0); n],
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &GfdmParams {
        &self.params
    }

    /// Per-point noise amplification — the zero-forcing tier's whole cost, readable without
    /// sweeping anything, and exactly 1 on the matched tier.
    #[must_use]
    pub fn amplification(&self) -> &[f32] {
        &self.amplification
    }

    /// Appends one point per transmitted point to `out`, dropping each block's prefix.
    pub fn demodulate(&mut self, x: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let n = self.params.block();
        for chunk in x.chunks_exact(self.params.samples()) {
            matvec(
                &self.matrix,
                n,
                n,
                &chunk[self.params.cp..],
                &mut self.block,
            );
            out.extend_from_slice(&self.block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> GfdmParams {
        GfdmParams::new(16, 5, 0.5)
    }

    fn points(count: usize) -> Vec<Complex<f32>> {
        let mut state = 0x9f_d0u32;
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

    /// The prototype is unit energy and its energy is where a pulse's should be — concentrated
    /// about sample 0, since that is what makes the `m`-th subsymbol a plain rotation.
    #[test]
    fn the_prototype_is_unit_energy_and_centred_on_zero() {
        let params = reference();
        let g = params.prototype();
        assert_eq!(g.len(), 80);
        let energy: f64 = g.iter().map(|v| v * v).sum();
        assert!((energy - 1.0).abs() < 1e-12, "energy {energy}");
        let peak = g
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert_eq!(peak, 0, "the pulse peaks away from sample 0");
    }

    /// Every column of the modulation matrix carries the same energy, which is what makes a
    /// per-point Eb/N0 mean one thing across the block.
    #[test]
    fn every_column_of_the_matrix_carries_unit_energy() {
        let params = reference();
        let n = params.block();
        let a = params.matrix();
        for column in 0..n {
            let energy: f64 = (0..n).map(|row| a[row * n + column].norm_sqr()).sum();
            assert!((energy - 1.0).abs() < 1e-9, "column {column}: {energy}");
        }
    }

    /// The zero-forcing tier is exact: without noise the points come back as they went in, which
    /// is the property a non-orthogonal waveform has to earn rather than inherit.
    #[test]
    fn the_zero_forcing_tier_round_trips_exactly() {
        let params = reference();
        let sent = points(params.block() * 4);
        let mut wave = Vec::new();
        GfdmMod::new(params).modulate(&sent, &mut wave);
        let mut got = Vec::new();
        GfdmDemod::new(params, GfdmDetector::ZeroForcing).demodulate(&wave, &mut got);
        assert_eq!(got.len(), sent.len());
        for (k, (a, b)) in got.iter().zip(&sent).enumerate() {
            assert!((a - b).norm() < 2e-4, "point {k}: {a} vs {b}");
        }
    }

    /// And the matched tier is not, which is the *other* half of the same fact. Its residual is
    /// the self-interference a non-orthogonal transmitter creates, and it is what becomes an
    /// error floor no Eb/N0 removes.
    #[test]
    fn the_matched_tier_leaves_measurable_self_interference() {
        let params = reference();
        let sent = points(params.block() * 8);
        let mut wave = Vec::new();
        GfdmMod::new(params).modulate(&sent, &mut wave);
        let mut got = Vec::new();
        GfdmDemod::new(params, GfdmDetector::Matched).demodulate(&wave, &mut got);
        let residual: f64 = got
            .iter()
            .zip(&sent)
            .map(|(a, b)| f64::from((a - b).norm_sqr()))
            .sum::<f64>()
            / sent.len() as f64;
        assert!(
            (0.01..1.0).contains(&residual),
            "matched-tier residual {residual}"
        );
    }

    /// The zero-forcing tier's cost, readable without sweeping anything: the inverse's row norms
    /// are the noise it amplifies, and the matched tier's are exactly one.
    #[test]
    fn the_zero_forcing_tier_states_its_own_noise_amplification() {
        let params = reference();
        let zf = GfdmDemod::new(params, GfdmDetector::ZeroForcing);
        let mf = GfdmDemod::new(params, GfdmDetector::Matched);
        for (k, &a) in mf.amplification().iter().enumerate() {
            assert!((a - 1.0).abs() < 1e-4, "matched row {k}: {a}");
        }
        let worst = zf.amplification().iter().fold(0.0f32, |acc, &v| acc.max(v));
        assert!(worst > 1.0, "the inverse amplifies nothing: {worst}");
        assert!(
            worst < 4.0,
            "roll-off 0.5 should stay well conditioned: {worst}"
        );
    }

    /// **The roll-off is the conditioning, and it runs the way the frequency axis runs, not the
    /// time axis.** A larger roll-off localises the pulse in *time* — which is what a reader
    /// expects to help, since GFDM's subsymbols overlap in time — but widens it in *frequency*,
    /// and the measurement says the subcarrier overlap is what dominates: at roll-off 0.9 the
    /// inverse amplifies noise by 1.86 where at 0.1 it amplifies by 1.01. So the pulse that costs
    /// the receiver least is the one whose spectrum is tightest, and the out-of-band roll-off the
    /// waveform exists for is bought at the zero-forcing tier's expense.
    #[test]
    fn a_spectrally_wider_prototype_costs_the_inverse_more() {
        let worst = |rolloff: f64| {
            GfdmDemod::new(GfdmParams::new(16, 5, rolloff), GfdmDetector::ZeroForcing)
                .amplification()
                .iter()
                .fold(0.0f32, |acc, &v| acc.max(v))
        };
        let tight = worst(0.1);
        let wide = worst(0.9);
        assert!(
            wide > tight,
            "roll-off 0.1 amplifies {tight}, roll-off 0.9 amplifies {wide}"
        );
        assert!((tight - 1.01).abs() < 0.1 && (wide - 1.86).abs() < 0.2);
    }

    /// The prefix is the block's, not the subsymbol's — the saving the waveform exists for.
    #[test]
    fn the_cyclic_prefix_is_the_blocks_own() {
        let mut params = reference();
        params.cp = 8;
        let sent = points(params.block());
        let mut wave = Vec::new();
        GfdmMod::new(params).modulate(&sent, &mut wave);
        assert_eq!(wave.len(), params.block() + 8);
        for k in 0..8 {
            let head = wave[k];
            let tail = wave[wave.len() - 8 + k];
            assert!((head - tail).norm() < 1e-6, "prefix sample {k}");
        }
        let mut got = Vec::new();
        GfdmDemod::new(params, GfdmDetector::ZeroForcing).demodulate(&wave, &mut got);
        for (a, b) in got.iter().zip(&sent) {
            assert!((a - b).norm() < 2e-4);
        }
    }

    /// `cp` is a public field nothing else validates, and a prefix longer than the block it
    /// copies underflows a `usize` in `modulate` rather than naming the parameter.
    #[test]
    #[should_panic(expected = "a cyclic prefix is a copy of the block's tail")]
    fn a_prefix_longer_than_its_block_is_rejected_at_construction() {
        let mut params = reference();
        params.cp = params.block() + 1;
        let _ = GfdmMod::new(params);
    }
}
