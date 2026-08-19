use std::{array, f32::consts::PI};

use num_complex::Complex;

const PHASE_BINS: usize = 32;

#[derive(Clone, Copy, Default)]
struct Bin {
    fourth: Complex<f32>,
    eighth: Complex<f32>,
    carrier: Complex<f32>,
    fourth_step: Complex<f32>,
    eighth_step: Complex<f32>,
    previous_fourth: Option<Complex<f32>>,
    previous_eighth: Option<Complex<f32>>,
    power: f64,
    count: u32,
    octants: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Acquired {
    pub locked: bool,
    pub snr_db: f32,
    pub frequency_error_hz: f32,
}

pub struct Acquisition {
    symbol_rate: f64,
    input_rate: f64,
    phase: f64,
    bins: [Bin; PHASE_BINS],
    samples: usize,
}

impl Acquisition {
    #[must_use]
    pub fn new(symbol_rate: f64, input_rate: f64) -> Self {
        Self {
            symbol_rate,
            input_rate,
            phase: 0.0,
            bins: array::from_fn(|_| Bin::default()),
            samples: 0,
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.clear();
    }

    fn clear(&mut self) {
        self.bins = array::from_fn(|_| Bin::default());
        self.samples = 0;
    }

    pub fn push(&mut self, iq: &[Complex<f32>], out: &mut Vec<Acquired>) {
        let step = self.symbol_rate / self.input_rate;
        for &sample in iq {
            let norm = sample.norm();
            if norm > 1e-6 && norm.is_finite() {
                self.accumulate(sample / norm, sample.norm_sqr());
            }
            self.phase += step;
            self.phase -= self.phase.floor();
            self.samples += 1;
            if self.samples >= self.input_rate as usize {
                out.push(self.report());
            }
        }
    }

    fn accumulate(&mut self, unit: Complex<f32>, power: f32) {
        let index = ((self.phase * PHASE_BINS as f64) as usize).min(PHASE_BINS - 1);
        let bin = &mut self.bins[index];
        let fourth = unit * unit * unit * unit;
        let eighth = fourth * fourth;
        bin.fourth += fourth;
        bin.eighth += eighth;
        bin.carrier += unit;
        if let Some(previous) = bin.previous_fourth {
            bin.fourth_step += fourth * previous.conj();
        }
        if let Some(previous) = bin.previous_eighth {
            bin.eighth_step += eighth * previous.conj();
        }
        bin.previous_fourth = Some(fourth);
        bin.previous_eighth = Some(eighth);
        bin.power += f64::from(power);
        bin.count = bin.count.saturating_add(1);
        let octant = ((unit.arg() + PI) * (4.0 / PI)).floor() as i32;
        bin.octants |= 1 << octant.rem_euclid(8);
    }

    fn report(&mut self) -> Acquired {
        let (best_index, order, coherence) = self
            .bins
            .iter()
            .enumerate()
            .flat_map(|(index, bin)| {
                let count = bin.count.max(1) as f32;
                [
                    (index, 4u8, bin.fourth.norm() / count),
                    (index, 8u8, bin.eighth.norm() / count),
                ]
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .unwrap_or((0, 4, 0.0));
        let best = self.bins[best_index];
        let count = best.count.max(1) as f32;
        let carrier = best.carrier.norm() / count;
        let occupied = best.octants.count_ones();
        let locked = coherence > 0.68 && carrier < 0.75 && occupied >= 3 && best.power > 1e-8;
        let step = if order == 4 {
            best.fourth_step
        } else {
            best.eighth_step
        };
        self.clear();
        Acquired {
            locked,
            snr_db: (-10.0 * (1.0 - coherence.clamp(0.0, 0.9999)).log10()).min(40.0),
            frequency_error_hz: step.arg() * self.symbol_rate as f32
                / (2.0 * PI * f32::from(order)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT_RATE_HZ: f64 = 2_000_000.0;

    fn drive(iq: &[Complex<f32>], symbol_rate: f64) -> Acquired {
        let mut acquisition = Acquisition::new(symbol_rate, INPUT_RATE_HZ);
        let mut out = Vec::new();
        for chunk in iq.chunks(997) {
            acquisition.push(chunk, &mut out);
        }
        out.pop().expect("a report after one second")
    }

    fn repeated(points: &[Complex<f32>], sps: usize) -> Vec<Complex<f32>> {
        let mut iq = Vec::with_capacity(INPUT_RATE_HZ as usize);
        for index in 0..(INPUT_RATE_HZ as usize / sps) {
            iq.extend(std::iter::repeat_n(
                points[(index * 13 + index / 7) % points.len()],
                sps,
            ));
        }
        iq
    }

    #[test]
    fn a_quadrature_carrier_acquires_at_the_configured_symbol_rate() {
        let points = [
            Complex::new(1.0, 1.0),
            Complex::new(-1.0, 1.0),
            Complex::new(-1.0, -1.0),
            Complex::new(1.0, -1.0),
        ];
        let report = drive(&repeated(&points, 8), 250_000.0);
        assert!(report.locked, "{report:?}");
    }

    #[test]
    fn an_eight_phase_carrier_acquires() {
        let points: Vec<_> = (0..8)
            .map(|index| Complex::from_polar(1.0, index as f32 * PI / 4.0))
            .collect();
        let report = drive(&repeated(&points, 8), 250_000.0);
        assert!(report.locked, "{report:?}");
    }

    #[test]
    fn a_single_unmodulated_carrier_is_not_a_transmission() {
        let report = drive(
            &vec![Complex::new(1.0, 0.0); INPUT_RATE_HZ as usize],
            250_000.0,
        );
        assert!(!report.locked, "{report:?}");
    }
}
