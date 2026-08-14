//! The feedforward timing tier: one estimate for a whole burst, from the signal's own square-law
//! spectral line, rather than a loop that walks toward the answer.
use num_complex::Complex;
use sdrmm_dsp::{Decimator, farrow};

use super::params::LinearParams;

/// Samples per symbol below which the square-law line aliases onto its own image and the
/// estimate stops meaning anything (Oerder & Meyr's `N ≥ 4`).
pub const MIN_SPS: usize = 4;

/// Feedforward symbol-timing recovery over a whole burst.
pub struct FeedforwardTiming {
    matched: Decimator,
    sps: usize,
    filtered: Vec<Complex<f32>>,
}

impl FeedforwardTiming {
    /// `receive_filter` is the entry's matched filter as unit-energy taps, as everywhere else in
    /// this engine.
    ///
    /// # Panics
    /// If `receive_filter` is empty or not unit-energy, or the entry's `sps` is below
    /// [`MIN_SPS`].
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

    /// Matched-filter a burst and sample it at the estimated symbol instants. Returns the symbols
    /// and the estimated offset in samples, which a caller records as the measurement it is.
    ///
    /// One call is one burst: the filter state is not carried, because an estimate made over one
    /// block and applied to another would be neither feedforward nor correct.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> f64 {
        self.matched.process(iq, &mut self.filtered);
        let offset = square_law_offset(&self.filtered, self.sps);
        resample_at(&self.filtered, self.sps, offset, out);
        offset
    }
}

/// The Oerder–Meyr estimate: the phase of the square-law spectral line at the symbol rate, as an
/// offset in samples inside `[0, sps)`. Returns 0 for a burst with no energy — no line, no phase,
/// and sampling from the block start is as good as anything.
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

/// Sample `filtered` at `offset + k·sps` for every whole k the block supports, interpolating with
/// the workspace's one Farrow kernel — the same four-tap parabolic interpolation `SymbolSync`
/// uses, so a comparison between the two timing tiers reads the estimator and not the
/// interpolator.
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
    use super::*;
    use crate::{
        ber::{
            impair::{Awgn, Impairment},
            rng::Rng,
        },
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

    /// The estimate must read back a *known* sub-sample delay. The delay is injected by
    /// modulating at a higher oversampling and decimating with an offset, which is an exact
    /// fractional shift of the transmitted waveform rather than an interpolation of it.
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
            // The delay advances the instant by `shift/fine` of a sample; the estimate lives
            // modulo a symbol, so the comparison does too.
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

    /// Residual EVM through the whole tier on a clean burst, per table. This is the number the
    /// high-order rows exist on: 256-QAM's slicing margin is 0.077, and a chain whose own error
    /// is a third of that has no waterfall left to measure.
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

    /// …and the comparison that justifies the tier existing: the tracking loop, at its own best
    /// bandwidth, on the identical burst.
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

    /// Noise degrades the estimate as 1/√(symbols), so a long burst is barely affected: the same
    /// 256-QAM burst at its own operating SNR must stay inside its margin.
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
