#[derive(Clone, Debug)]
pub struct Rng {
    state: [u64; 4],
    spare: Option<f64>,
}

const UNIFORM_SCALE: f64 = 1.0 / (1u64 << 53) as f64;

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        Self {
            state: [
                splitmix64(&mut sm),
                splitmix64(&mut sm),
                splitmix64(&mut sm),
                splitmix64(&mut sm),
            ],
            spare: None,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[0]
            .wrapping_add(self.state[3])
            .rotate_left(23)
            .wrapping_add(self.state[0]);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * UNIFORM_SCALE
    }

    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v * m);
                return u * m;
            }
        }
    }

    pub fn normal_pair(&mut self) -> (f64, f64) {
        (self.normal(), self.normal())
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::Rng;

    const CALIBRATION_SAMPLES: usize = 1_000_000;

    #[test]
    fn same_seed_replays_the_identical_stream() {
        let mut a = Rng::new(0x5eed);
        let mut b = Rng::new(0x5eed);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        for _ in 0..1000 {
            assert_eq!(a.uniform().to_bits(), b.uniform().to_bits());
            assert_eq!(a.normal().to_bits(), b.normal().to_bits());
            let (ai, aq) = a.normal_pair();
            let (bi, bq) = b.normal_pair();
            assert_eq!(ai.to_bits(), bi.to_bits());
            assert_eq!(aq.to_bits(), bq.to_bits());
        }
    }

    #[test]
    fn cloned_rng_replays_from_the_fork_point() {
        let mut a = Rng::new(7);
        for _ in 0..100 {
            a.normal();
        }
        let mut b = a.clone();
        for _ in 0..1000 {
            assert_eq!(a.normal().to_bits(), b.normal().to_bits());
        }
    }

    #[test]
    fn different_seeds_give_different_streams() {
        for pair in [
            (0u64, 1u64),
            (1, 2),
            (0x5eed, 0x5eee),
            (u64::MAX - 1, u64::MAX),
        ] {
            let mut a = Rng::new(pair.0);
            let mut b = Rng::new(pair.1);
            let stream_a: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
            let stream_b: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
            assert_ne!(stream_a, stream_b, "seeds {:#x} and {:#x}", pair.0, pair.1);
        }
    }

    #[test]
    fn uniform_stays_in_unit_interval() {
        let mut rng = Rng::new(42);
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for _ in 0..CALIBRATION_SAMPLES {
            let x = rng.uniform();
            assert!((0.0..1.0).contains(&x), "uniform draw {x} outside [0,1)");
            min = min.min(x);
            max = max.max(x);
        }
        assert!(min < 1e-4, "min uniform {min}");
        assert!(max > 1.0 - 1e-4, "max uniform {max}");
    }

    #[test]
    fn normal_is_calibrated() {
        let mut rng = Rng::new(0xca11b8a7e);
        let samples: Vec<f64> = (0..CALIBRATION_SAMPLES).map(|_| rng.normal()).collect();
        let n = samples.len() as f64;

        let mean = samples.iter().sum::<f64>() / n;
        assert!(mean.abs() < 5e-3, "mean {mean}");

        let variance = samples.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
        assert!((variance - 1.0).abs() < 0.01, "variance {variance}");

        let lag1_cov = samples
            .windows(2)
            .map(|w| (w[0] - mean) * (w[1] - mean))
            .sum::<f64>()
            / n;
        let lag1_corr = lag1_cov / variance;
        assert!(lag1_corr.abs() < 5e-3, "lag-1 autocorrelation {lag1_corr}");
    }

    #[test]
    fn normal_pair_components_are_uncorrelated() {
        let mut rng = Rng::new(0x1004);
        let pairs: Vec<(f64, f64)> = (0..CALIBRATION_SAMPLES)
            .map(|_| rng.normal_pair())
            .collect();
        let n = pairs.len() as f64;

        let mean_i = pairs.iter().map(|p| p.0).sum::<f64>() / n;
        let mean_q = pairs.iter().map(|p| p.1).sum::<f64>() / n;
        let var_i = pairs
            .iter()
            .map(|p| (p.0 - mean_i) * (p.0 - mean_i))
            .sum::<f64>()
            / n;
        let var_q = pairs
            .iter()
            .map(|p| (p.1 - mean_q) * (p.1 - mean_q))
            .sum::<f64>()
            / n;
        let cov = pairs
            .iter()
            .map(|p| (p.0 - mean_i) * (p.1 - mean_q))
            .sum::<f64>()
            / n;
        let corr = cov / (var_i * var_q).sqrt();
        assert!(corr.abs() < 5e-3, "I/Q correlation {corr}");
        assert!((var_i - 1.0).abs() < 0.01, "I variance {var_i}");
        assert!((var_q - 1.0).abs() < 0.01, "Q variance {var_q}");
    }
}
