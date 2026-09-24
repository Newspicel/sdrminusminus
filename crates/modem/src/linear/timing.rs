use num_complex::Complex;
use sdrmm_dsp::{Decimator, farrow};

use super::{acquire::FrequencyAcquisition, params::LinearParams};

pub const MIN_SPS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TimingMetric {
    #[default]
    SquareLaw,
    RotatedPower {
        rotation_rad: f64,
        order: u32,
    },
}

pub struct FeedforwardTiming {
    matched: Decimator,
    sps: usize,
    metric: TimingMetric,
    filtered: Vec<Complex<f32>>,
    lines: Vec<Complex<f64>>,
    acquisition: FrequencyAcquisition,
    candidates: Vec<Complex<f32>>,
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
            metric: TimingMetric::SquareLaw,
            filtered: Vec::new(),
            lines: Vec::new(),
            acquisition: FrequencyAcquisition::new(),
            candidates: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_metric(mut self, metric: TimingMetric) -> Self {
        self.metric = metric;
        self
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> f64 {
        self.matched.process(iq, &mut self.filtered);
        let track = match self.metric {
            TimingMetric::SquareLaw => square_law_track(&self.filtered, self.sps, &mut self.lines),
            TimingMetric::RotatedPower {
                rotation_rad,
                order,
            } => TimingTrack {
                offset_samples: rotated_power_offset(
                    &self.filtered,
                    self.sps,
                    rotation_rad,
                    order,
                    &mut self.acquisition,
                    &mut self.candidates,
                ),
                at_sample: 0.0,
                rate: 0.0,
            },
        };
        track.resample(&self.filtered, self.sps, out);
        track.offset_at(0.0, self.sps)
    }
}

pub const TRACK_BLOCK_SYMBOLS: usize = 64;

const MIN_RATE_SIGMAS: f64 = 4.0;

const STEP_SEARCH_OVERSAMPLE: usize = 4;

const MIN_RATE_EXCURSION_SAMPLES: f64 = 0.05;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimingTrack {
    pub offset_samples: f64,
    pub at_sample: f64,
    pub rate: f64,
}

impl TimingTrack {
    #[must_use]
    pub fn offset_at(&self, sample: f64, sps: usize) -> f64 {
        (self.offset_samples + self.rate * (sample - self.at_sample)).rem_euclid(sps as f64)
    }

    pub fn resample(&self, filtered: &[Complex<f32>], sps: usize, out: &mut Vec<Complex<f32>>) {
        let anchor = self.offset_samples - self.rate * self.at_sample;
        let step = sps as f64 / (1.0 - self.rate);
        let origin = anchor / (1.0 - self.rate);
        let first = ((1.0 - origin) / step).ceil();
        let mut k = first;
        loop {
            let position = origin + k * step;
            if (position as usize) + 2 >= filtered.len() {
                break;
            }
            let base = position as usize;
            let mu = (position - base as f64) as f32;
            out.push(farrow(&filtered[base - 1..base + 3], mu));
            k += 1.0;
        }
    }
}

pub fn square_law_track(
    filtered: &[Complex<f32>],
    sps: usize,
    lines: &mut Vec<Complex<f64>>,
) -> TimingTrack {
    let block = TRACK_BLOCK_SYMBOLS * sps;
    let blocks = filtered.len() / block;
    let whole = TimingTrack {
        offset_samples: square_law_offset(filtered, sps),
        at_sample: 0.0,
        rate: 0.0,
    };
    if blocks < 2 {
        return whole;
    }
    lines.clear();
    lines.extend(
        filtered
            .chunks_exact(block)
            .map(|b| square_law_line(b, sps)),
    );
    let centre = (blocks as f64 - 1.0) / 2.0;
    let coarse = strongest_step(lines, centre);
    let derotate = |b: usize, step: f64| Complex::from_polar(1.0, -step * (b as f64 - centre));
    let mean: Complex<f64> = lines
        .iter()
        .enumerate()
        .map(|(b, &l)| l * derotate(b, coarse))
        .sum();
    if mean.norm() <= 0.0 {
        return whole;
    }
    let fit = residual_slope(lines, coarse, mean, centre);
    let total = coarse + fit.slope;
    let excursion = (total * centre * sps as f64 / std::f64::consts::TAU).abs();
    let step = if total.abs() >= MIN_RATE_SIGMAS * fit.standard_error
        && excursion >= MIN_RATE_EXCURSION_SAMPLES
    {
        total
    } else {
        0.0
    };
    let acc: Complex<f64> = lines
        .iter()
        .enumerate()
        .map(|(b, &l)| l * derotate(b, step))
        .sum();
    let to_samples = -(sps as f64) / std::f64::consts::TAU;
    TimingTrack {
        offset_samples: (acc.arg() * to_samples).rem_euclid(sps as f64),
        at_sample: (centre + 0.5) * block as f64,
        rate: step * to_samples / block as f64,
    }
}

fn strongest_step(lines: &[Complex<f64>], centre: f64) -> f64 {
    let bins = STEP_SEARCH_OVERSAMPLE * lines.len();
    let power = |step: f64| {
        lines
            .iter()
            .enumerate()
            .map(|(b, &l)| l * Complex::from_polar(1.0, -step * (b as f64 - centre)))
            .sum::<Complex<f64>>()
            .norm()
    };
    let spacing = std::f64::consts::TAU / bins as f64;
    let step_of = |i: usize| (i as f64 - (bins / 2) as f64) * spacing;
    let Some((best, _)) = (0..bins)
        .map(|i| (i, power(step_of(i))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return 0.0;
    };
    let (left, mid, right) = (
        power(step_of(best) - spacing),
        power(step_of(best)),
        power(step_of(best) + spacing),
    );
    let curvature = left - 2.0 * mid + right;
    let shift = if curvature < 0.0 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    step_of(best) + shift * spacing
}

pub fn rotated_power_offset(
    filtered: &[Complex<f32>],
    sps: usize,
    rotation_rad: f64,
    order: u32,
    acquisition: &mut FrequencyAcquisition,
    symbols: &mut Vec<Complex<f32>>,
) -> f64 {
    let mut metric = |offset: usize| -> f64 {
        symbols.clear();
        symbols.extend(
            (offset..filtered.len())
                .step_by(sps)
                .enumerate()
                .map(|(k, n)| {
                    let turn = (-rotation_rad * k as f64).rem_euclid(std::f64::consts::TAU);
                    filtered[n] * Complex::new(turn.cos() as f32, turn.sin() as f32)
                }),
        );
        acquisition.peak_power(symbols, order).sqrt()
    };
    let scores: Vec<f64> = (0..sps).map(&mut metric).collect();
    let Some((best, _)) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
        return 0.0;
    };
    let (left, mid, right) = (
        scores[(best + sps - 1) % sps],
        scores[best],
        scores[(best + 1) % sps],
    );
    let curvature = left - 2.0 * mid + right;
    let shift = if curvature < 0.0 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (best as f64 + shift).rem_euclid(sps as f64)
}

struct SlopeFit {
    slope: f64,
    standard_error: f64,
}

fn residual_slope(
    lines: &[Complex<f64>],
    coarse: f64,
    mean: Complex<f64>,
    centre: f64,
) -> SlopeFit {
    let rotate = |b: usize| Complex::from_polar(1.0, -coarse * (b as f64 - centre));
    let residual = |b: usize, l: Complex<f64>| (l * rotate(b) * mean.conj()).arg();
    let (mut num, mut den, mut weight) = (0.0f64, 0.0f64, 0.0f64);
    for (b, &l) in lines.iter().enumerate() {
        let t = b as f64 - centre;
        let w = l.norm();
        num += w * t * residual(b, l);
        den += w * t * t;
        weight += w;
    }
    if den <= 0.0 || weight <= 0.0 || lines.len() < 3 {
        return SlopeFit {
            slope: 0.0,
            standard_error: f64::INFINITY,
        };
    }
    let slope = num / den;
    let scatter: f64 = lines
        .iter()
        .enumerate()
        .map(|(b, &l)| l.norm() * (residual(b, l) - slope * (b as f64 - centre)).powi(2))
        .sum::<f64>()
        / weight
        * lines.len() as f64
        / (lines.len() - 2) as f64;
    SlopeFit {
        slope,
        standard_error: (scatter * weight / lines.len() as f64 / den).sqrt(),
    }
}

fn square_law_line(filtered: &[Complex<f32>], sps: usize) -> Complex<f64> {
    let mut acc = Complex::new(0.0f64, 0.0);
    for (n, y) in filtered.iter().enumerate() {
        let theta = -std::f64::consts::TAU * (n % sps) as f64 / sps as f64;
        acc += Complex::new(theta.cos(), theta.sin()) * f64::from(y.norm_sqr());
    }
    acc
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
