use num_complex::Complex;
use sdrmm_dsp::{Decimator, farrow};

use super::params::LinearParams;

pub const MIN_SPS: usize = 4;

pub struct FeedforwardTiming {
    matched: Decimator,
    sps: usize,
    filtered: Vec<Complex<f32>>,
}

impl FeedforwardTiming {
    #[must_use]
    pub fn new(params: &LinearParams, receive_filter: &[f32]) -> Self {
        assert!(!receive_filter.is_empty(), "receive filter must have taps");
        let energy: f64 = receive_filter
            .iter()
            .map(|&h| f64::from(h) * f64::from(h))
            .sum();
        assert!(
            (energy - 1.0).abs() < 1e-3,
            "receive filter must be unit-energy (pulse::Norm::Energy), got Σh² = {energy}"
        );
        assert!(
            params.sps() >= MIN_SPS,
            "the square-law line needs at least {MIN_SPS} samples per symbol, got {}",
            params.sps()
        );
        Self {
            matched: Decimator::new(receive_filter, 1),
            sps: params.sps(),
            filtered: Vec::new(),
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> f64 {
        self.matched.process(iq, &mut self.filtered);
        let offset = square_law_offset(&self.filtered, self.sps);
        resample_at(&self.filtered, self.sps, offset, out);
        offset
    }
}

#[must_use]
pub fn square_law_offset(filtered: &[Complex<f32>], sps: usize) -> f64 {
    let mut acc = Complex::new(0.0f64, 0.0);
    let usable = filtered.len() - filtered.len() % sps;
    for (n, y) in filtered[..usable].iter().enumerate() {
        let theta = -std::f64::consts::TAU * (n % sps) as f64 / sps as f64;
        acc += Complex::new(theta.cos(), theta.sin()) * f64::from(y.norm_sqr());
    }
    if acc.norm() <= 0.0 {
        return 0.0;
    }
    let tau = -acc.arg() / std::f64::consts::TAU * sps as f64;
    tau.rem_euclid(sps as f64)
}

pub fn resample_at(
    filtered: &[Complex<f32>],
    sps: usize,
    offset: f64,
    out: &mut Vec<Complex<f32>>,
) {
    let mut position = offset;
    while position < 1.0 {
        position += sps as f64;
    }
    while (position as usize) + 2 < filtered.len() {
        let base = position as usize;
        let mu = (position - base as f64) as f32;
        out.push(farrow(&filtered[base - 1..base + 3], mu));
        position += sps as f64;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::{
        impair::{Awgn, Impairment},
        rng::Rng,
    };

    use super::*;
    use crate::{
        constellation::{Constellation, tables},
        linear::LinearMod,
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    #[test]
    fn the_estimate_reads_back_an_injected_fractional_delay() {
        let fine = 8usize;
        let table = tables::qam_square(16).unwrap();
        for shift in 0..fine {
            let dense = LinearParams::new(table.clone(), rrc_at(SPS * fine), SPS * fine).unwrap();
            let wave = LinearMod::transmission(&dense, &labels(400, 16, 0x71));
            let coarse: Vec<Complex<f32>> =
                wave.iter().skip(shift).step_by(fine).copied().collect();
            let mut matched = Decimator::new(&rrc(), 1);
            let mut filtered = Vec::new();
            matched.process(&coarse, &mut filtered);
            let measured = square_law_offset(&filtered, SPS);
            let want = (-(shift as f64) / fine as f64).rem_euclid(SPS as f64);
            let error = ((measured - want + SPS as f64 / 2.0).rem_euclid(SPS as f64)
                - SPS as f64 / 2.0)
                .abs();
            assert!(
                error < 0.05,
                "shift {shift}/{fine}: read {measured}, want {want}"
            );
        }
    }

    fn rrc_at(sps: usize) -> Vec<f32> {
        pulse::root_raised_cosine(sps as f64, 0.35, 8, Norm::Energy)
    }

    #[test]
    fn the_tier_recovers_every_table_well_inside_its_margin() {
        for (name, table) in [
            ("qam16", tables::qam_square(16).unwrap()),
            ("qam64", tables::qam_square(64).unwrap()),
            ("qam256", tables::qam_square(256).unwrap()),
            ("qam1024", tables::qam_square(1024).unwrap()),
            ("cross128", tables::qam_cross(128).unwrap()),
        ] {
            let m = table.len() as u32;
            let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
            let sent = labels(4_000, m, 0x7e);
            let wave = LinearMod::transmission(&p, &sent);
            let mut tier = FeedforwardTiming::new(&p, &rrc());
            let mut symbols = Vec::new();
            tier.process(&wave, &mut symbols);
            let rms = evm(&table, &symbols[8..symbols.len() - 8]);
            let margin = min_distance(&table) / 2.0;
            assert!(
                rms < 0.1 * margin,
                "{name}: EVM {rms} against a slicing margin of {margin}"
            );
        }
    }

    #[test]
    fn the_feedforward_estimate_beats_the_tracking_loop_on_a_burst() {
        use crate::linear::{LinearDemod, LinearTiming};
        let table = tables::qam_square(256).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let sent = labels(4_000, 256, 0x7e);
        let wave = LinearMod::transmission(&p, &sent);

        let mut tier = FeedforwardTiming::new(&p, &rrc());
        let mut feedforward = Vec::new();
        tier.process(&wave, &mut feedforward);

        let mut demod = LinearDemod::new(
            &p,
            &rrc(),
            LinearTiming {
                timing_bw: 0.005,
                power_symbols: 1_000.0,
            },
            None,
        );
        let mut tracked = Vec::new();
        demod.process(&wave, &mut tracked);

        let a = evm(&table, &feedforward[8..feedforward.len() - 8]);
        let b = evm(&table, &tracked[8..tracked.len() - 8]);
        assert!(a * 3.0 < b, "feedforward {a} vs tracked {b}");
    }

    #[test]
    fn the_estimate_survives_noise_over_a_long_burst() {
        let table = tables::qam_square(256).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let mut wave = LinearMod::transmission(&p, &labels(4_000, 256, 0x7e));
        Awgn::with_sigma((0.5 * 10f64.powf(-3.0)).sqrt()).apply(&mut wave, &mut Rng::new(0x91));
        let mut tier = FeedforwardTiming::new(&p, &rrc());
        let mut symbols = Vec::new();
        let offset = tier.process(&wave, &mut symbols);
        assert!((0.0..SPS as f64).contains(&offset), "offset {offset}");
        let clean = LinearMod::transmission(&p, &labels(4_000, 256, 0x7e));
        let mut reference = FeedforwardTiming::new(&p, &rrc());
        let mut ignored = Vec::new();
        let truth = reference.process(&clean, &mut ignored);
        let error = ((offset - truth + 4.0).rem_euclid(8.0) - 4.0).abs();
        assert!(error < 0.05, "noise moved the estimate by {error} samples");
    }

    #[test]
    fn an_empty_burst_estimates_nothing_rather_than_a_nan() {
        assert_eq!(square_law_offset(&[], SPS), 0.0);
        assert_eq!(square_law_offset(&[Complex::new(0.0, 0.0); 64], SPS), 0.0);
    }

    fn evm(table: &Constellation, symbols: &[Complex<f32>]) -> f64 {
        (symbols
            .iter()
            .map(|&y| {
                let l = table.hard_slice(y);
                let i = table.labels().iter().position(|&x| x == l).unwrap();
                f64::from((y - table.points()[i]).norm_sqr())
            })
            .sum::<f64>()
            / symbols.len() as f64)
            .sqrt()
    }

    fn min_distance(table: &Constellation) -> f64 {
        let p = table.points();
        let mut min = f64::INFINITY;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                min = min.min(f64::from((p[i] - p[j]).norm()));
            }
        }
        min
    }
}
