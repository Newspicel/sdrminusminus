use num_complex::Complex;

use super::transform::Dft;

pub const MAX_GRID: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtfsGrid {
    pub delay: usize,
    pub doppler: usize,
}

impl OtfsGrid {
    #[must_use]
    pub fn new(delay: usize, doppler: usize) -> Self {
        Self { delay, doppler }
    }

    #[must_use]
    pub fn points(&self) -> usize {
        self.delay * self.doppler
    }
}

#[derive(Clone)]
pub struct OtfsPrecoder {
    grid: OtfsGrid,
    doppler_dft: Dft,
    delay_dft: Dft,
    column: Vec<Complex<f32>>,
    plane: Vec<Complex<f32>>,
}

impl OtfsPrecoder {
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

    pub fn spread(&mut self, dd: &[Complex<f32>], out: &mut [Complex<f32>]) {
        let (m, n) = (self.grid.delay, self.grid.doppler);
        debug_assert_eq!(dd.len(), m * n);
        debug_assert_eq!(out.len(), m * n);
        for delay in 0..m {
            let column = &mut self.column[..n];
            column.copy_from_slice(&dd[delay * n..(delay + 1) * n]);
            self.doppler_dft.forward(column);
            for (symbol, &v) in column.iter().enumerate() {
                self.plane[symbol * m + delay] = v;
            }
        }
        for symbol in 0..n {
            let row = &mut self.plane[symbol * m..(symbol + 1) * m];
            self.delay_dft.inverse(row);
        }
        out.copy_from_slice(&self.plane);
    }

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

    #[test]
    fn without_the_precoder_a_symbol_sits_on_one_bin() {
        let grid = grid();
        let mut precoder = OtfsPrecoder::new(grid);
        let mut tf = vec![Complex::new(0.0f32, 0.0); grid.points()];
        tf[5 * grid.delay + 11] = Complex::new(1.0, 0.0);
        let mut dd = vec![Complex::new(0.0, 0.0); grid.points()];
        precoder.despread(&tf, &mut dd);
        let want = (grid.points() as f32).sqrt().recip();
        for (k, v) in dd.iter().enumerate() {
            assert!((v.norm() - want).abs() < 1e-5, "bin {k}: {}", v.norm());
        }
    }
}
