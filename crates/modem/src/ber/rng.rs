//! The one random source every harness run draws from (MODEM-PLAN §4.1: fixed seeds
//! everywhere).
//!
//! Reproducibility is a harness invariant, not a convenience: a committed curve or limits
//! table that cannot be regenerated bit-for-bit from its stated seed is a bug in the harness.
//! That rules out every ambient source — OS entropy differs per run, `std`'s hasher seeds
//! differ per process, and an external RNG crate could change its stream between versions
//! without this crate noticing. So the generator is written here, once, and its stream is part
//! of the harness contract.
//!
//! The algorithm is xoshiro256++ (Blackman & Vigna, public domain). Quality matters as much as
//! determinism: a sweep resolving BER 1e-5 consumes ~1e7 noise samples per point, and a
//! generator with lattice structure or short-range correlation at that depth would put its own
//! artefacts into the measured curve — the harness would be reading the generator, not the
//! channel. xoshiro256++ passes BigCrush and PractRand at scales far beyond any sweep here,
//! with a 2^256−1 period, and costs a handful of integer ops per draw — nothing for the
//! throughput measurements in `perf` to notice.
//!
//! Seeding goes through SplitMix64 (Steele, Lea & Flood; public domain) so a run is named by a
//! single u64. xoshiro forbids the all-zero state, and nearby seeds handed to it directly would
//! start in nearby states; one SplitMix64 pass per state word escapes zero for every seed and
//! makes seed and seed+1 unrelated streams — sweeps may number their runs 0, 1, 2, … and still
//! get independent noise.
//!
//! The integer stream is exact on every platform. The floating-point derivations stay that way
//! because they use only IEEE-754 correctly-rounded operations (`*`, `/`, `+`, `sqrt`) plus
//! `ln`, whose platform libms agree to well under an ulp — orders of magnitude below anything a
//! BER count can resolve.

/// Seeded deterministic generator for Monte-Carlo BER work. Cheap to [`Clone`]: a forked copy
/// replays the identical stream from the fork point, which is how a sweep reruns one point of
/// a curve without regenerating everything before it.
#[derive(Clone, Debug)]
pub struct Rng {
    state: [u64; 4],
    /// Marsaglia polar produces normals in pairs; the undrawn half waits here. It is part of
    /// the stream contract: draws alternate compute/spare, so interleaving other draw types
    /// between them is deterministic too.
    spare: Option<f64>,
}

/// Maps the raw 64-bit draw onto the 2^53 evenly spaced doubles in [0, 1). Using exactly the
/// 53 bits the mantissa can hold means every value is representable without rounding — no
/// draw can round up to 1.0, which as a noise or threshold input would be an off-by-one the
/// channel models must never see.
const UNIFORM_SCALE: f64 = 1.0 / (1u64 << 53) as f64;

impl Rng {
    /// A run is named by one u64 (curve labels quote it), expanded to the full 256-bit state
    /// via SplitMix64 — see the module docs for why direct seeding would be wrong.
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

    /// The xoshiro256++ core step; every other draw type is derived from this stream.
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

    /// Uniform draw in [0, 1) with the full 53 bits of double precision — coarser would
    /// quantise the tails of everything derived from it, and the normal tail is exactly where
    /// high-Eb/N0 bit errors come from.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * UNIFORM_SCALE
    }

    /// Standard normal draw, N(0, 1), by Marsaglia's polar method. Polar over Box–Muller
    /// because it needs no trig — `sqrt` is correctly rounded by IEEE-754 where `sin`/`cos`
    /// are not, so this keeps the platform-dependence of the whole stream down to `ln` alone.
    /// Each accepted polar sample yields two normals; the spare is cached, not discarded, so
    /// consecutive draws cost one rejection loop between them, not two.
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            // Rejection keeps (u, v) strictly inside the unit circle: s ≥ 1 would bend the
            // distribution, s = 0 would hand ln a zero. ~21.5% of pairs retry.
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v * m);
                return u * m;
            }
        }
    }

    /// One sample of circularly-symmetric complex noise as an i.i.d. N(0, 1) pair — the two
    /// normals of one polar acceptance are independent, so I and Q come from a single loop
    /// pass. Each component has unit variance, so the pair has total power 2: an AWGN channel
    /// scales each component by √(N0/2) to land at noise power N0 per complex sample.
    pub fn normal_pair(&mut self) -> (f64, f64) {
        (self.normal(), self.normal())
    }
}

/// SplitMix64: one output per call, advancing `state` by the golden-ratio increment. Only used
/// to expand seeds — it is a fine generator but its 2^64 period is too short to be the main
/// stream for a full nightly sweep.
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

    /// Enough samples that the acceptance bounds below sit at ≥5σ of their estimators —
    /// a failure means the generator (or an edit to it) is broken, not an unlucky seed.
    const CALIBRATION_SAMPLES: usize = 1_000_000;

    #[test]
    fn same_seed_replays_the_identical_stream() {
        let mut a = Rng::new(0x5eed);
        let mut b = Rng::new(0x5eed);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // Derived draws too, interleaved, so the spare cache is part of what is compared.
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

    /// Adjacent seeds must diverge immediately — sweeps number their runs sequentially.
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
        // The draws should also fill the interval, not huddle in part of it.
        assert!(min < 1e-4, "min uniform {min}");
        assert!(max > 1.0 - 1e-4, "max uniform {max}");
    }

    /// Calibration of the normal path: mean, variance and lag-1 autocorrelation over 1e6
    /// draws. Estimator standard errors are ~1e-3 for the mean and lag-1 correlation and
    /// ~1.4e-3 for the variance, so the bounds are ≥5σ. Serial correlation is what a BER
    /// measurement is most sensitive to — correlated noise samples change the effective
    /// noise bandwidth and would bias every curve the same direction.
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

    /// I and Q of the complex-noise helper must be uncorrelated: correlated components make
    /// the noise elliptical, which is an IQ-imbalance impairment, not AWGN.
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
