use num_complex::Complex;

use crate::fft::FftPair;

/// A range–Doppler power surface, row-major with one row per Doppler hypothesis.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surface {
    pub ranges: usize,
    pub dopplers: usize,
    pub doppler_step_hz: f32,
    pub range_step_s: f32,
    pub power: Vec<f32>,
}

impl Surface {
    #[must_use]
    pub fn at(&self, doppler: usize, range: usize) -> f32 {
        self.power
            .get(doppler * self.ranges + range)
            .copied()
            .unwrap_or(0.0)
    }

    /// The Doppler shift a row stands for. Rows run from the most negative shift upwards, so a
    /// stationary echo sits in the middle.
    #[must_use]
    pub fn doppler_hz(&self, row: usize) -> f32 {
        (row as f32 - (self.dopplers as f32 - 1.0) / 2.0) * self.doppler_step_hz
    }
}

/// The cross-ambiguity function: how well the surveillance lane matches the reference lane at
/// every delay and every Doppler shift at once.
///
/// One transform of the reference serves the whole surface; each Doppler hypothesis costs one
/// mix, one forward transform and one inverse.
pub struct Caf {
    fft: FftPair,
    cpi: usize,
    ranges: usize,
    dopplers: usize,
    sample_rate: f64,
    reference: Vec<Complex<f32>>,
    observed: Vec<Complex<f32>>,
    work: Vec<Complex<f32>>,
}

impl Caf {
    #[must_use]
    pub fn new(cpi: usize, ranges: usize, dopplers: usize, sample_rate: f64) -> Self {
        let ranges = ranges.clamp(1, cpi.max(1));
        let size = (cpi + ranges).next_power_of_two().max(2);
        Self {
            fft: FftPair::new(size),
            cpi,
            ranges,
            dopplers: dopplers.max(1),
            sample_rate,
            reference: vec![Complex::default(); size],
            observed: vec![Complex::default(); size],
            work: vec![Complex::default(); size],
        }
    }

    #[must_use]
    pub const fn cpi(&self) -> usize {
        self.cpi
    }

    /// One Doppler bin is one over the integration time, which is the finest shift a coherent
    /// stretch of this length can tell apart.
    #[must_use]
    pub fn doppler_step_hz(&self) -> f32 {
        (self.sample_rate / self.cpi.max(1) as f64) as f32
    }

    pub fn compute(
        &mut self,
        reference: &[Complex<f32>],
        surveillance: &[Complex<f32>],
        out: &mut Surface,
    ) {
        let size = self.fft.len();
        let count = self.cpi.min(reference.len()).min(surveillance.len());
        out.ranges = self.ranges;
        out.dopplers = self.dopplers;
        out.doppler_step_hz = self.doppler_step_hz();
        out.range_step_s = (1.0 / self.sample_rate) as f32;
        out.power.clear();
        out.power.resize(self.ranges * self.dopplers, 0.0);
        if count == 0 {
            return;
        }
        self.reference.clear();
        self.reference.extend(reference[..count].iter().copied());
        self.reference.resize(size, Complex::default());
        self.fft.forward(&mut self.reference);

        self.observed.clear();
        self.observed.extend(surveillance[..count].iter().copied());
        self.observed.resize(size, Complex::default());
        self.fft.forward(&mut self.observed);

        let step = self.doppler_step_hz() as f64;
        let centre = (self.dopplers as f64 - 1.0) / 2.0;
        let bins_per_row = size as f64 / self.cpi.max(1) as f64;
        let mask = size - 1;
        for row in 0..self.dopplers {
            let shift_hz = (row as f64 - centre) * step;
            let w = -std::f64::consts::TAU * shift_hz / self.sample_rate;
            let shift_bins = -(row as f64 - centre) * bins_per_row;
            let whole = shift_bins.round();
            if size.is_power_of_two() && (shift_bins - whole).abs() < 1e-9 {
                let offset = (whole as i64).rem_euclid(size as i64) as usize;
                for bin in 0..size {
                    self.work[bin] =
                        self.observed[(bin + size - offset) & mask] * self.reference[bin].conj();
                }
            } else {
                self.work.clear();
                self.work.extend(
                    surveillance[..count]
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            value * Complex::from_polar(1.0, (w * index as f64) as f32)
                        }),
                );
                self.work.resize(size, Complex::default());
                self.fft.forward(&mut self.work);
                for (bin, reference) in self.work.iter_mut().zip(&self.reference) {
                    *bin *= reference.conj();
                }
            }
            self.fft.inverse_scaled(&mut self.work);
            let base = row * self.ranges;
            for range in 0..self.ranges {
                out.power[base + range] = self.work[range].norm_sqr();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_spectral_shortcut_agrees_with_a_direct_correlation() {
        const RATE: f64 = 48_000.0;
        let (cpi, ranges, dopplers) = (256usize, 8usize, 5usize);
        let mut state = 0x2B1Du32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let reference: Vec<Complex<f32>> = (0..cpi).map(|_| Complex::new(next(), next())).collect();
        let surveillance: Vec<Complex<f32>> =
            (0..cpi).map(|_| Complex::new(next(), next())).collect();

        let mut caf = Caf::new(cpi, ranges, dopplers, RATE);
        let mut surface = Surface::default();
        caf.compute(&reference, &surveillance, &mut surface);

        let step = f64::from(surface.doppler_step_hz);
        let centre = (dopplers as f64 - 1.0) / 2.0;
        for row in 0..dopplers {
            let w = -std::f64::consts::TAU * (row as f64 - centre) * step / RATE;
            let mixed: Vec<Complex<f32>> = surveillance
                .iter()
                .enumerate()
                .map(|(index, value)| value * Complex::from_polar(1.0, (w * index as f64) as f32))
                .collect();
            for lag in 0..ranges {
                let mut acc = Complex::default();
                for (index, carrier) in reference.iter().enumerate() {
                    if let Some(value) = mixed.get(index + lag) {
                        acc += value * carrier.conj();
                    }
                }
                let direct: f32 = acc.norm_sqr();
                let taken = surface.at(row, lag);
                assert!(
                    (direct - taken).abs() <= 1e-3 * direct.max(1.0),
                    "row {row} lag {lag}: direct {direct}, shortcut {taken}"
                );
            }
        }
    }

    use std::f32::consts::TAU;

    use super::*;

    const RATE: f64 = 1_000_000.0;

    fn noise(len: usize, seed: u64) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                let mut next = || {
                    state ^= state >> 12;
                    state ^= state << 25;
                    state ^= state >> 27;
                    (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32
                        - 1.0
                };
                Complex::new(next(), next())
            })
            .collect()
    }

    fn peak(surface: &Surface) -> (usize, usize) {
        let mut best = (0usize, 0usize, 0.0f32);
        for doppler in 0..surface.dopplers {
            for range in 0..surface.ranges {
                let value = surface.at(doppler, range);
                if value > best.2 {
                    best = (doppler, range, value);
                }
            }
        }
        (best.0, best.1)
    }

    #[test]
    fn an_echo_lands_on_the_delay_and_doppler_it_was_built_with() {
        let cpi = 4_096;
        let reference = noise(cpi, 0xC0FF);
        let delay = 37usize;
        let doppler_hz = 3.0 * (RATE / cpi as f64) as f32;
        let surveillance: Vec<Complex<f32>> = (0..cpi)
            .map(|index| {
                let source = index.saturating_sub(delay);
                reference[source]
                    * Complex::from_polar(1.0f32, TAU * doppler_hz * index as f32 / RATE as f32)
            })
            .collect();

        let mut caf = Caf::new(cpi, 128, 21, RATE);
        let mut surface = Surface::default();
        caf.compute(&reference, &surveillance, &mut surface);
        let (row, range) = peak(&surface);
        assert_eq!(range, delay);
        assert!(
            (surface.doppler_hz(row) - doppler_hz).abs() < 0.5 * surface.doppler_step_hz,
            "row {row} is {} Hz, wanted {doppler_hz}",
            surface.doppler_hz(row)
        );
    }

    #[test]
    fn a_stationary_copy_sits_at_zero_range_and_zero_doppler() {
        let cpi = 2_048;
        let reference = noise(cpi, 0x1234);
        let mut caf = Caf::new(cpi, 64, 11, RATE);
        let mut surface = Surface::default();
        caf.compute(&reference, &reference, &mut surface);
        assert_eq!(peak(&surface), (5, 0));
        assert!(surface.doppler_hz(5).abs() < 1e-6);
    }

    #[test]
    fn an_unrelated_surveillance_lane_produces_no_peak_worth_the_name() {
        let cpi = 2_048;
        let reference = noise(cpi, 0xAAAA);
        let surveillance = noise(cpi, 0xBBBB);
        let mut caf = Caf::new(cpi, 64, 11, RATE);
        let mut surface = Surface::default();
        caf.compute(&reference, &surveillance, &mut surface);
        let mean: f32 = surface.power.iter().sum::<f32>() / surface.power.len() as f32;
        let (row, range) = peak(&surface);
        assert!(
            surface.at(row, range) < 40.0 * mean,
            "a peak stands out of pure noise"
        );
    }

    #[test]
    fn an_empty_block_gives_an_empty_surface_of_the_right_shape() {
        let mut caf = Caf::new(1_024, 32, 7, RATE);
        let mut surface = Surface::default();
        caf.compute(&[], &[], &mut surface);
        assert_eq!(surface.ranges, 32);
        assert_eq!(surface.dopplers, 7);
        assert!(surface.power.iter().all(|value| *value == 0.0));
    }
}
