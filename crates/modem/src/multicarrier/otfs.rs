//! OTFS — orthogonal time–frequency space.
//!
//! **OTFS is a precoder, not a carrier**, and saying so is most of understanding it. The points do
//! not ride subcarriers; they ride a **delay–Doppler grid**, and a two-dimensional transform — the
//! inverse symplectic finite Fourier transform — spreads that grid across the time–frequency grid
//! an ordinary [`ofdm`](crate::ofdm) frame already carries. That is why this module holds a
//! precoder and no modulator: the waveform on the wire is OFDM's, and everything OTFS is worth
//! happens in the map into it.
//!
//! **What it is worth, in this crate's own terms.** Phase 6 measured that an uncoded one-tap
//! equaliser *loses a nulled subcarrier outright* — the channel divides, the null divides by
//! nothing, and every bit on that subcarrier is gone. Every symbol of an OTFS frame occupies
//! *every* subcarrier of that frame, in equal measure, so a null costs the whole frame a small
//! amount instead of costing one subcarrier everything. That is diversity, in the uncoded domain,
//! and it is measurable with nothing but the impairments already in the harness — which is what
//! the entry measures.
//!
//! **Under AWGN it is exactly transparent, and that is the acceptance.** The transform is unitary
//! in both directions (each carries `1/√N`), so a flat channel plus white noise cannot see it at
//! all: an OTFS curve *is* its constellation's curve, and every dB it sits from one is the OFDM
//! frame's own overhead or a defect.
//!
//! The channel's delay–Doppler view — where OTFS's other claims (a sparse, slowly-varying channel
//! matrix; message passing over it) live — needs a doubly-selective channel model the harness does
//! not have, and is out of scope here (§1.1). What is in scope is the transform, its unitarity,
//! its spreading, and what the spreading buys against a static null.

use num_complex::Complex;

use super::transform::Dft;

/// Largest grid a precoder will plan transforms for.
pub const MAX_GRID: usize = 4_096;

/// The delay–Doppler grid: `delay × doppler` points per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtfsGrid {
    /// Delay bins — one per subcarrier the carrier frame allocates.
    pub delay: usize,
    /// Doppler bins — one per symbol in the frame.
    pub doppler: usize,
}

impl OtfsGrid {
    #[must_use]
    pub fn new(delay: usize, doppler: usize) -> Self {
        Self { delay, doppler }
    }

    /// Points one frame carries.
    #[must_use]
    pub fn points(&self) -> usize {
        self.delay * self.doppler
    }
}

/// The transform, both directions.
///
/// Layouts, stated once because a two-dimensional transform is where an index convention goes
/// wrong silently: the **delay–Doppler** grid is delay-major (`m·doppler + n`), and the
/// **time–frequency** grid is symbol-major (`k·delay + l`) — which is exactly the order
/// [`OfdmMod::frame`](crate::ofdm::OfdmMod::frame) reads its points in, so a spread frame hands
/// straight to the carrier with no re-indexing between them.
#[derive(Clone)]
pub struct OtfsPrecoder {
    grid: OtfsGrid,
    doppler_dft: Dft,
    delay_dft: Dft,
    column: Vec<Complex<f32>>,
    plane: Vec<Complex<f32>>,
}

impl OtfsPrecoder {
    /// # Panics
    /// If either axis is empty or past [`MAX_GRID`].
    #[must_use]
    pub fn new(grid: OtfsGrid) -> Self {
        assert!(
            (1..=MAX_GRID).contains(&grid.delay) && (1..=MAX_GRID).contains(&grid.doppler),
            "an OTFS grid runs 1..={MAX_GRID} on each axis"
        );
        Self {
            doppler_dft: Dft::new(grid.doppler),
            delay_dft: Dft::new(grid.delay),
            column: vec![Complex::new(0.0, 0.0); grid.doppler.max(grid.delay)],
            plane: vec![Complex::new(0.0, 0.0); grid.points()],
            grid,
        }
    }

    #[must_use]
    pub fn grid(&self) -> OtfsGrid {
        self.grid
    }

    /// Delay–Doppler grid to time–frequency grid: transform along Doppler, then along delay.
    ///
    /// `out` receives exactly `grid.points()` values, symbol-major.
    pub fn spread(&mut self, dd: &[Complex<f32>], out: &mut [Complex<f32>]) {
        let (m, n) = (self.grid.delay, self.grid.doppler);
        debug_assert_eq!(dd.len(), m * n);
        debug_assert_eq!(out.len(), m * n);
        // Doppler axis: one N-point transform per delay bin.
        for delay in 0..m {
            let column = &mut self.column[..n];
            column.copy_from_slice(&dd[delay * n..(delay + 1) * n]);
            self.doppler_dft.forward(column);
            for (symbol, &v) in column.iter().enumerate() {
                self.plane[symbol * m + delay] = v;
            }
        }
        // Delay axis: one M-point inverse transform per symbol, in place on the plane.
        for symbol in 0..n {
            let row = &mut self.plane[symbol * m..(symbol + 1) * m];
            self.delay_dft.inverse(row);
        }
        out.copy_from_slice(&self.plane);
    }

    /// The exact adjoint of [`Self::spread`] — time–frequency grid back to delay–Doppler.
    pub fn despread(&mut self, tf: &[Complex<f32>], out: &mut [Complex<f32>]) {
        let (m, n) = (self.grid.delay, self.grid.doppler);
        debug_assert_eq!(tf.len(), m * n);
        debug_assert_eq!(out.len(), m * n);
        self.plane.copy_from_slice(tf);
        for symbol in 0..n {
            let row = &mut self.plane[symbol * m..(symbol + 1) * m];
            self.delay_dft.forward(row);
        }
        for delay in 0..m {
            let column = &mut self.column[..n];
            for (symbol, slot) in column.iter_mut().enumerate() {
                *slot = self.plane[symbol * m + delay];
            }
            self.doppler_dft.inverse(column);
            out[delay * n..(delay + 1) * n].copy_from_slice(column);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> OtfsGrid {
        OtfsGrid::new(48, 16)
    }

    fn points(count: usize) -> Vec<Complex<f32>> {
        let mut state = 0x0_07f5u32;
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

    /// Unitarity, both halves: the round trip is the identity and the energy never moves — which
    /// is what makes an OTFS curve comparable with its constellation's own.
    #[test]
    fn the_precoder_is_unitary() {
        let mut precoder = OtfsPrecoder::new(grid());
        let dd = points(grid().points());
        let mut tf = vec![Complex::new(0.0, 0.0); dd.len()];
        let mut back = vec![Complex::new(0.0, 0.0); dd.len()];
        precoder.spread(&dd, &mut tf);
        let energy = |x: &[Complex<f32>]| x.iter().map(|v| f64::from(v.norm_sqr())).sum::<f64>();
        assert!(
            (energy(&tf) / energy(&dd) - 1.0).abs() < 1e-4,
            "energy moved through the spread"
        );
        precoder.despread(&tf, &mut back);
        for (k, (a, b)) in back.iter().zip(&dd).enumerate() {
            assert!((a - b).norm() < 1e-4, "point {k}: {a} vs {b}");
        }
    }

    /// **The entry, as a property**: one delay–Doppler symbol occupies every time–frequency bin
    /// equally. That is why a nulled subcarrier costs the whole frame a little instead of costing
    /// one symbol everything, and it is checkable without a channel.
    #[test]
    fn one_symbol_spreads_evenly_over_the_whole_time_frequency_grid() {
        let grid = grid();
        let mut precoder = OtfsPrecoder::new(grid);
        let mut dd = vec![Complex::new(0.0f32, 0.0); grid.points()];
        dd[7 * grid.doppler + 3] = Complex::new(1.0, 0.0);
        let mut tf = vec![Complex::new(0.0, 0.0); grid.points()];
        precoder.spread(&dd, &mut tf);
        let want = (grid.points() as f32).sqrt().recip();
        for (k, v) in tf.iter().enumerate() {
            assert!(
                (v.norm() - want).abs() < 1e-5,
                "bin {k} holds {} of an expected {want}",
                v.norm()
            );
        }
    }

    /// And the converse, which is what a *carrier* does instead: without the precoder a symbol
    /// sits on exactly one bin, so anything that kills that bin kills the symbol.
    #[test]
    fn without_the_precoder_a_symbol_sits_on_one_bin() {
        let grid = grid();
        let mut precoder = OtfsPrecoder::new(grid);
        let mut tf = vec![Complex::new(0.0f32, 0.0); grid.points()];
        tf[5 * grid.delay + 11] = Complex::new(1.0, 0.0);
        let mut dd = vec![Complex::new(0.0, 0.0); grid.points()];
        precoder.despread(&tf, &mut dd);
        let want = (grid.points() as f32).sqrt().recip();
        // Read the other way, the same identity: a lone time–frequency bin is spread evenly over
        // the delay–Doppler grid, which is the statement "every symbol rides every subcarrier"
        // seen from the receiver's side.
        for (k, v) in dd.iter().enumerate() {
            assert!((v.norm() - want).abs() < 1e-5, "bin {k}: {}", v.norm());
        }
    }
}
