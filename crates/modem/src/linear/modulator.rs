use num_complex::Complex;

use super::params::LinearParams;

#[derive(Clone, Debug)]
pub struct LinearMod {
    params: LinearParams,
    symbols_in: u64,
    tail: Vec<Complex<f32>>,
}

impl LinearMod {
    #[must_use]
    pub fn new(params: LinearParams) -> Self {
        Self {
            params,
            symbols_in: 0,
            tail: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.params
    }

    #[must_use]
    pub fn point(&self, k: u64, label: u32) -> Complex<f32> {
        let table = self.params.constellation();
        let index = table
            .labels()
            .iter()
            .position(|&l| l == label)
            .unwrap_or_default();
        let p = table.points()[index];
        if self.params.rotation_rad() == 0.0 {
            return p;
        }
        let theta = (k as f64 * self.params.rotation_rad()) % std::f64::consts::TAU;
        p * Complex::new(theta.cos() as f32, theta.sin() as f32)
    }

    pub fn modulate(&mut self, labels: &[u32], out: &mut Vec<Complex<f32>>) {
        let sps = self.params.sps();
        let pulse = self.params.pulse();
        let stagger = self.params.stagger_samples();
        let span = labels.len() * sps + pulse.len() + stagger;
        if self.tail.len() < span {
            self.tail.resize(span, Complex::new(0.0, 0.0));
        }
        for (j, &label) in labels.iter().enumerate() {
            let s = self.point(self.symbols_in + j as u64, label);
            let base = j * sps;
            for (m, &h) in pulse.iter().enumerate() {
                self.tail[base + m].re += s.re * h;
                self.tail[base + m + stagger].im += s.im * h;
            }
        }
        self.symbols_in += labels.len() as u64;
        let complete = labels.len() * sps;
        out.extend_from_slice(&self.tail[..complete]);
        self.tail.drain(..complete);
    }

    pub fn flush(&mut self, out: &mut Vec<Complex<f32>>) {
        out.append(&mut self.tail);
    }

    pub fn reset(&mut self) {
        self.symbols_in = 0;
        self.tail.clear();
    }

    #[must_use]
    pub fn transmission(params: &LinearParams, labels: &[u32]) -> Vec<Complex<f32>> {
        let mut m = Self::new(params.clone());
        let mut out = Vec::with_capacity(labels.len() * params.sps() + params.pulse().len());
        m.modulate(labels, &mut out);
        m.flush(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::{impair::signal_energy, rng::Rng};

    use super::*;
    use crate::{
        constellation::tables,
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn params(m: u32) -> LinearParams {
        LinearParams::new(tables::qam_square(m).unwrap(), rrc(), SPS).unwrap()
    }

    fn random_labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    #[test]
    fn block_energy_is_one_per_symbol() {
        for m in [4u32, 16, 64] {
            let labels = random_labels(2048, m, 0xe9e);
            let wave = LinearMod::transmission(&params(m), &labels);
            let es = signal_energy(&wave) / labels.len() as f64;
            assert!((es - 1.0).abs() < 0.02, "M={m}: Es = {es}");
        }
    }

    #[test]
    fn any_block_split_gives_the_same_waveform() {
        let p = params(16);
        let labels = random_labels(300, 16, 0x5137);
        let whole = LinearMod::transmission(&p, &labels);
        let mut m = LinearMod::new(p);
        let mut split = Vec::new();
        for chunk in labels.chunks(37) {
            m.modulate(chunk, &mut split);
        }
        m.flush(&mut split);
        assert_eq!(split, whole);
    }

    #[test]
    fn the_rotation_schedule_advances_one_step_per_symbol() {
        let p = params(4).with_rotation(tables::PI_4_ROTATION).unwrap();
        let m = LinearMod::new(p);
        let base = m.point(0, 0);
        for k in [1u64, 2, 7, 1_000_003] {
            let want = (k as f64 * std::f64::consts::FRAC_PI_4) % std::f64::consts::TAU;
            let got = f64::from((m.point(k, 0) / base).arg());
            let diff = (got - want + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            assert!(diff.abs() < 1e-5, "k={k}: {got} vs {want}");
        }
    }

    #[test]
    fn the_stagger_delays_the_quadrature_rail_by_half_a_symbol() {
        let p = params(4).with_offset(true).unwrap();
        let wave = LinearMod::transmission(&p, &[0b11]);
        let peak = |f: fn(&Complex<f32>) -> f32| {
            wave.iter()
                .enumerate()
                .max_by(|a, b| f(a.1).abs().total_cmp(&f(b.1).abs()))
                .map(|(i, _)| i)
                .unwrap()
        };
        assert_eq!(peak(|s| s.im) - peak(|s| s.re), SPS / 2);
        let aligned = LinearMod::transmission(&params(4), &[0b11]);
        let peak_a = |f: fn(&Complex<f32>) -> f32| {
            aligned
                .iter()
                .enumerate()
                .max_by(|a, b| f(a.1).abs().total_cmp(&f(b.1).abs()))
                .map(|(i, _)| i)
                .unwrap()
        };
        assert_eq!(peak_a(|s| s.im), peak_a(|s| s.re));
    }

    #[test]
    fn the_stagger_keeps_the_trajectory_off_the_origin() {
        let labels = random_labels(512, 4, 0x0f5e7);
        let plain = LinearMod::transmission(&params(4), &labels);
        let staggered = LinearMod::transmission(&params(4).with_offset(true).unwrap(), &labels);
        let near_origin = |w: &[Complex<f32>], frac: f64| {
            let inner = &w[8 * SPS..w.len() - 8 * SPS];
            let rms = (signal_energy(inner) / inner.len() as f64).sqrt();
            inner
                .iter()
                .filter(|s| f64::from(s.norm()) < frac * rms)
                .count() as f64
                / inner.len() as f64
        };
        let (a, b) = (near_origin(&plain, 0.2), near_origin(&staggered, 0.2));
        assert!(a > 0.02, "QPSK spends only {a} of its time near the origin");
        assert!(b * 20.0 < a, "OQPSK {b} vs QPSK {a}");
        assert!(near_origin(&plain, 0.1) > 0.005);
        assert_eq!(near_origin(&staggered, 0.1), 0.0);
        let (ea, eb) = (signal_energy(&plain), signal_energy(&staggered));
        assert!((ea / eb - 1.0).abs() < 0.02, "{ea} vs {eb}");
    }

    #[test]
    fn labels_index_the_table_by_value() {
        let table = tables::qam_cross(32).unwrap();
        assert!(
            table
                .labels()
                .iter()
                .enumerate()
                .any(|(i, &l)| l != i as u32),
            "the test needs a table whose labels are not their own indices"
        );
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let m = LinearMod::new(p);
        for (i, &label) in table.labels().iter().enumerate() {
            assert_eq!(m.point(0, label), table.points()[i]);
        }
    }
}
