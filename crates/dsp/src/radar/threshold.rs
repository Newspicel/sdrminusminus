use crate::special::gamma::{gamma_p, gamma_q, ln_gamma};

pub const MIN_ALPHA: f64 = 1e-3;
pub const MAX_ALPHA: f64 = 1e6;

const BISECTION_STEPS: usize = 60;
const Z_MIN: f64 = 1e-18;
const Z_MAX: f64 = 40.0;
const INTERVALS: usize = 4096;
const SPREAD_WIDTHS: f64 = 40.0;
const DIRECT_SUM_LIMIT: f64 = 700.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CfarStatistic {
    Ca,
    Os { rank: f32 },
    Go,
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum ThresholdError {
    #[error("CFAR has too few training cells: {0}")]
    Cells(usize),
    #[error("CFAR needs at least one look")]
    Looks,
    #[error("range correlation must be at least 1, got {0}")]
    Correlation(f64),
    #[error("OS rank must be above 0 and at most 1, got {0}")]
    Rank(f32),
    #[error("false alarm rate must be between 0 and 1, got {0}")]
    Pfa(f64),
    #[error("no threshold from {MIN_ALPHA} to {MAX_ALPHA} gives a false alarm rate of {0}")]
    Unreachable(f64),
}

#[must_use]
pub fn os_order(rank: f32, cells: usize) -> usize {
    ((f64::from(rank) * cells as f64).round() as usize).clamp(1, cells.max(1))
}

#[must_use]
pub fn pfa(stat: CfarStatistic, cells: usize, alpha: f64, looks: u32, correlation: f64) -> f64 {
    if !(alpha >= 0.0 && alpha.is_finite()) {
        return f64::NAN;
    }
    Model::new(stat, cells, looks, correlation).map_or(f64::NAN, |model| model.pfa(alpha))
}

pub fn alpha(
    stat: CfarStatistic,
    cells: usize,
    pfa: f64,
    looks: u32,
    correlation: f64,
) -> Result<f64, ThresholdError> {
    if !(pfa > 0.0 && pfa < 1.0) {
        return Err(ThresholdError::Pfa(pfa));
    }
    let model = Model::new(stat, cells, looks, correlation)?;
    if model.pfa(MIN_ALPHA) < pfa || model.pfa(MAX_ALPHA) > pfa {
        return Err(ThresholdError::Unreachable(pfa));
    }
    let (mut low, mut high) = (MIN_ALPHA.ln(), MAX_ALPHA.ln());
    for _ in 0..BISECTION_STEPS {
        let middle = 0.5 * (low + high);
        if model.pfa(middle.exp()) > pfa {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok((0.5 * (low + high)).exp())
}

enum Model {
    Closed {
        looks: u32,
        shape: f64,
        cells: f64,
    },
    Integral {
        looks: u32,
        nodes: Vec<f64>,
        weights: Vec<f64>,
    },
}

impl Model {
    fn new(
        stat: CfarStatistic,
        cells: usize,
        looks: u32,
        correlation: f64,
    ) -> Result<Self, ThresholdError> {
        if cells == 0 {
            return Err(ThresholdError::Cells(cells));
        }
        if looks == 0 {
            return Err(ThresholdError::Looks);
        }
        if !(correlation >= 1.0 && correlation.is_finite()) {
            return Err(ThresholdError::Correlation(correlation));
        }
        let effective = (cells as f64 / correlation).max(1.0);
        let per_cell = f64::from(looks);
        match stat {
            CfarStatistic::Ca => Ok(Self::Closed {
                looks,
                shape: effective * per_cell,
                cells: effective,
            }),
            CfarStatistic::Os { rank } => {
                if !(rank > 0.0 && rank <= 1.0) {
                    return Err(ThresholdError::Rank(rank));
                }
                let order = (os_order(rank, cells) as f64 / correlation).max(1.0);
                let ln_choose = ln_gamma(effective + 1.0)
                    - ln_gamma(order + 1.0)
                    - ln_gamma(effective - order + 1.0);
                Ok(Self::integral(
                    looks,
                    order_span(effective, order, per_cell),
                    |z| order_density(z, effective, order, per_cell, ln_choose),
                ))
            }
            CfarStatistic::Go => {
                if cells < 2 {
                    return Err(ThresholdError::Cells(cells));
                }
                let half = (0.5 * effective).max(1.0) * per_cell;
                Ok(Self::integral(looks, span(1.0, half.sqrt().recip()), |z| {
                    greatest_density(z, half)
                }))
            }
        }
    }

    fn integral(looks: u32, span: (f64, f64), density: impl Fn(f64) -> f64) -> Self {
        let low = span.0.max(Z_MIN).ln();
        let step = (span.1.ln() - low) / INTERVALS as f64;
        let (nodes, weights) = (0..=INTERVALS)
            .map(|index| {
                let z = (low + index as f64 * step).exp();
                (z, simpson_coefficient(index) * step / 3.0 * z * density(z))
            })
            .unzip();
        Self::Integral {
            looks,
            nodes,
            weights,
        }
    }

    fn pfa(&self, alpha: f64) -> f64 {
        match self {
            Self::Closed {
                looks,
                shape,
                cells,
            } => closed_pfa(*looks, *shape, alpha / cells),
            Self::Integral {
                looks,
                nodes,
                weights,
            } => nodes
                .iter()
                .zip(weights)
                .map(|(z, weight)| weight * survival(*looks, alpha * z))
                .sum(),
        }
    }
}

fn span(center: f64, spread: f64) -> (f64, f64) {
    if !(spread > 0.0 && spread.is_finite() && center.is_finite()) {
        return (0.0, Z_MAX);
    }
    (
        (center - SPREAD_WIDTHS * spread).max(0.0),
        (center + SPREAD_WIDTHS * spread).min(Z_MAX),
    )
}

fn order_span(cells: f64, order: f64, looks: f64) -> (f64, f64) {
    let fraction = order / (cells + 1.0);
    let center = quantile(looks, fraction);
    let density = ln_unit_mean_gamma_pdf(looks, center).exp();
    span(
        center,
        (fraction * (1.0 - fraction) / (cells + 2.0)).sqrt() / density,
    )
}

fn quantile(looks: f64, fraction: f64) -> f64 {
    let (mut low, mut high) = (0.0, Z_MAX);
    for _ in 0..BISECTION_STEPS {
        let middle = 0.5 * (low + high);
        if gamma_p(looks, looks * middle) < fraction {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

fn closed_pfa(looks: u32, shape: f64, ratio: f64) -> f64 {
    let ln_shape = ln_gamma(shape);
    let ln_ratio = ratio.ln();
    let ln_rise = ratio.ln_1p();
    (0..looks)
        .map(|term| {
            let i = f64::from(term);
            (ln_gamma(shape + i) - ln_gamma(i + 1.0) - ln_shape + power_ln(i, ln_ratio)
                - (shape + i) * ln_rise)
                .exp()
        })
        .sum()
}

fn survival(looks: u32, y: f64) -> f64 {
    let x = f64::from(looks) * y;
    if x > DIRECT_SUM_LIMIT {
        return gamma_q(f64::from(looks), x);
    }
    let mut term = (-x).exp();
    let mut sum = term;
    for j in 1..looks {
        term *= x / f64::from(j);
        sum += term;
    }
    sum
}

fn order_density(z: f64, cells: f64, order: f64, looks: f64, ln_choose: f64) -> f64 {
    let below = gamma_p(looks, looks * z);
    let above = gamma_q(looks, looks * z);
    (order.ln()
        + ln_choose
        + power_ln(order - 1.0, below.ln())
        + power_ln(cells - order, above.ln())
        + ln_unit_mean_gamma_pdf(looks, z))
    .exp()
}

fn greatest_density(z: f64, shape: f64) -> f64 {
    2.0 * gamma_p(shape, shape * z) * ln_unit_mean_gamma_pdf(shape, z).exp()
}

fn ln_unit_mean_gamma_pdf(shape: f64, z: f64) -> f64 {
    shape * shape.ln() + power_ln(shape - 1.0, z.ln()) - shape * z - ln_gamma(shape)
}

fn power_ln(exponent: f64, ln_value: f64) -> f64 {
    if exponent == 0.0 {
        0.0
    } else {
        exponent * ln_value
    }
}

fn simpson_coefficient(index: usize) -> f64 {
    if index == 0 || index == INTERVALS {
        1.0
    } else if index % 2 == 1 {
        4.0
    } else {
        2.0
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_1_SQRT_2, TAU};

    use num_complex::Complex;

    use super::*;

    const STATS: [CfarStatistic; 3] = [
        CfarStatistic::Ca,
        CfarStatistic::Os { rank: 0.75 },
        CfarStatistic::Go,
    ];

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn uniform(&mut self) -> f64 {
            ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }

        fn gaussian(&mut self) -> Complex<f64> {
            let radius = (-2.0 * self.uniform().ln()).sqrt();
            Complex::from_polar(radius * FRAC_1_SQRT_2, TAU * self.uniform())
        }

        fn looks(&mut self, looks: u32) -> f64 {
            (0..looks).map(|_| -self.uniform().ln()).sum::<f64>() / f64::from(looks)
        }
    }

    fn oversampled_noise(len: usize, seed: u64) -> Vec<Complex<f64>> {
        let mut rng = Rng(seed);
        let white: Vec<Complex<f64>> = (0..=len).map(|_| rng.gaussian()).collect();
        white
            .windows(2)
            .map(|pair| (pair[0] + pair[1]) * FRAC_1_SQRT_2)
            .collect()
    }

    fn range_correlation(samples: &[Complex<f64>]) -> f64 {
        let lag = |tau: usize| {
            samples[tau..]
                .iter()
                .zip(samples)
                .map(|(late, early)| late * early.conj())
                .sum::<Complex<f64>>()
        };
        let zero = lag(0).norm_sqr();
        1.0 + 2.0 * (1..=3).map(|tau| lag(tau).norm_sqr() / zero).sum::<f64>()
    }

    fn noise_estimate(
        stat: CfarStatistic,
        left: &[f64],
        right: &[f64],
        scratch: &mut [f64],
    ) -> f64 {
        let mean = |cells: &[f64]| cells.iter().sum::<f64>() / cells.len() as f64;
        match stat {
            CfarStatistic::Ca => 0.5 * (mean(left) + mean(right)),
            CfarStatistic::Go => mean(left).max(mean(right)),
            CfarStatistic::Os { rank } => {
                scratch[..left.len()].copy_from_slice(left);
                scratch[left.len()..].copy_from_slice(right);
                let order = os_order(rank, scratch.len());
                *scratch.select_nth_unstable_by(order - 1, f64::total_cmp).1
            }
        }
    }

    fn false_alarms(
        stat: CfarStatistic,
        cells: &[f64],
        guard: usize,
        train: usize,
        alpha: f64,
    ) -> f64 {
        let reach = guard + train;
        let mut scratch = vec![0.0; 2 * train];
        let mut alarms = 0usize;
        let trials = cells.len() - 2 * reach;
        for cut in reach..cells.len() - reach {
            let left = &cells[cut - reach..cut - guard];
            let right = &cells[cut + guard + 1..=cut + reach];
            if cells[cut] > alpha * noise_estimate(stat, left, right, &mut scratch) {
                alarms += 1;
            }
        }
        alarms as f64 / trials as f64
    }

    fn mean_of_greatest_half(shape: f64) -> f64 {
        let steps = 400_000;
        let width = Z_MAX / f64::from(steps);
        (0..steps)
            .map(|index| {
                let z = (f64::from(index) + 0.5) * width;
                z * greatest_density(z, shape) * width
            })
            .sum()
    }

    fn go_closed_form(half: usize, alpha: f64) -> f64 {
        let n = half as f64;
        let beta = alpha / n;
        let tail: f64 = (0..half)
            .map(|i| {
                let i = i as f64;
                (ln_gamma(n + i) - ln_gamma(i + 1.0) - ln_gamma(n) - (n + i) * (2.0 + beta).ln())
                    .exp()
            })
            .sum();
        2.0 * (-n * beta.ln_1p()).exp() - 2.0 * tail
    }

    fn os_product(cells: usize, order: usize, alpha: f64) -> f64 {
        (0..order)
            .map(|i| {
                let left = (cells - i) as f64;
                left / (left + alpha)
            })
            .product()
    }

    #[test]
    fn ca_alpha_matches_closed_form_for_one_look() {
        for cells in [8, 16, 32, 64] {
            for target in [1e-3_f64, 1e-4, 1e-6] {
                let expected = cells as f64 * (target.powf(-1.0 / cells as f64) - 1.0);
                let got = alpha(CfarStatistic::Ca, cells, target, 1, 1.0).unwrap();
                assert!(
                    (got / expected - 1.0).abs() < 1e-9,
                    "N = {cells}, pfa = {target}: {got} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn pfa_inverts_alpha() {
        for stat in STATS {
            for looks in [1, 3] {
                for correlation in [1.0, 1.6] {
                    for target in [1e-3, 1e-5] {
                        let factor = alpha(stat, 32, target, looks, correlation).unwrap();
                        let back = pfa(stat, 32, factor, looks, correlation);
                        assert!(
                            (back / target - 1.0).abs() < 1e-6,
                            "{stat:?}, L = {looks}, rho = {correlation}: {back} vs {target}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn os_matches_product_formula_for_one_look() {
        for cells in [16, 24, 32, 400, 2_000, 9_000] {
            for rank in [0.5, 0.75, 0.95] {
                let order = os_order(rank, cells);
                for factor in [2.0, 5.0, 10.0, 20.0] {
                    let expected = os_product(cells, order, factor);
                    let got = pfa(CfarStatistic::Os { rank }, cells, factor, 1, 1.0);
                    assert!(
                        (got / expected - 1.0).abs() < 1e-6,
                        "N = {cells}, rank = {rank}, alpha = {factor}: {got} vs {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn go_matches_closed_form_for_one_look() {
        for (half, factors) in [
            (4, [2.0, 8.0, 30.0]),
            (16, [2.0, 8.0, 20.0]),
            (64, [3.0, 9.0, 18.0]),
            (1_000, [3.0, 9.0, 18.0]),
            (4_500, [3.0, 9.0, 18.0]),
        ] {
            for factor in factors {
                let expected = go_closed_form(half, factor);
                let got = pfa(CfarStatistic::Go, 2 * half, factor, 1, 1.0);
                assert!(
                    (got / expected - 1.0).abs() < 1e-6,
                    "n = {half}, alpha = {factor}: {got} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn low_pfa_on_few_cells_meets_the_closed_forms() {
        for target in [1e-5, 1e-7, 1e-9] {
            for half in [1, 2, 3] {
                let factor = alpha(CfarStatistic::Go, 2 * half, target, 1, 1.0).unwrap();
                let exact = go_closed_form(half, factor);
                assert!(
                    (exact / target - 1.0).abs() < 1e-6,
                    "GO n = {half}, pfa = {target}: {exact}"
                );
            }
            for cells in [2, 3, 4, 6] {
                let rank = 0.75;
                let factor = alpha(CfarStatistic::Os { rank }, cells, target, 1, 1.0).unwrap();
                let exact = os_product(cells, os_order(rank, cells), factor);
                assert!(
                    (exact / target - 1.0).abs() < 1e-6,
                    "OS N = {cells}, pfa = {target}: {exact}"
                );
            }
        }
    }

    #[test]
    fn go_keeps_one_independent_cell_a_side() {
        let floor = alpha(CfarStatistic::Go, 2, 1e-5, 1, 1.0).unwrap();
        for (cells, correlation) in [(16, 14.0), (6, 7.0), (4, 2.5)] {
            let factor = alpha(CfarStatistic::Go, cells, 1e-5, 1, correlation).unwrap();
            assert!(
                (factor / floor - 1.0).abs() < 1e-9,
                "{cells}, {correlation}"
            );
        }
        let wide = alpha(CfarStatistic::Go, 64, 1e-5, 1, 14.0).unwrap();
        assert!(wide < floor);
    }

    #[test]
    fn one_cell_statistics_agree_for_many_looks() {
        for factor in [0.5_f64, 2.0, 5.0, 10.0] {
            let two_looks = (1.0 + 3.0 * factor) / (1.0 + factor).powi(3);
            let ca = pfa(CfarStatistic::Ca, 1, factor, 2, 1.0);
            assert!(
                (ca / two_looks - 1.0).abs() < 1e-12,
                "alpha = {factor}: {ca} vs {two_looks}"
            );
            for looks in [2, 4, 9, 16] {
                let ca = pfa(CfarStatistic::Ca, 1, factor, looks, 1.0);
                let os = pfa(CfarStatistic::Os { rank: 1.0 }, 1, factor, looks, 1.0);
                assert!(
                    (os / ca - 1.0).abs() < 1e-5,
                    "L = {looks}, alpha = {factor}: OS {os} vs CA {ca}"
                );
            }
        }
    }

    #[test]
    fn measured_pfa_matches_design_for_independent_looks() {
        let mut rng = Rng(0x0dec_0de5);
        let (guard, train, design, looks) = (2, 8, 1e-3, 4);
        let cells: Vec<f64> = (0..1_000_000).map(|_| rng.looks(looks)).collect();
        for stat in STATS {
            let factor = alpha(stat, 2 * train, design, looks, 1.0).unwrap();
            let measured = false_alarms(stat, &cells, guard, train, factor);
            assert!(
                measured > 0.85 * design && measured < 1.15 * design,
                "{stat:?}: measured {measured} for design {design}"
            );
            let one_look = alpha(stat, 2 * train, design, 1, 1.0).unwrap();
            let mismatched = false_alarms(stat, &cells, guard, train, one_look);
            assert!(mismatched < 0.1 * design, "{stat:?}: {mismatched}");
        }
    }

    #[test]
    fn go_needs_more_than_ca() {
        for cells in [8, 16, 32] {
            for looks in [1, 4] {
                let ca = alpha(CfarStatistic::Ca, cells, 1e-4, looks, 1.0).unwrap();
                let go = alpha(CfarStatistic::Go, cells, 1e-4, looks, 1.0).unwrap();
                let half = cells as f64 / 2.0 * f64::from(looks);
                let go_threshold = go * mean_of_greatest_half(half);
                assert!(
                    go_threshold > ca,
                    "N = {cells}, L = {looks}: GO {go_threshold} <= CA {ca}"
                );
            }
        }
    }

    #[test]
    fn more_looks_lower_alpha() {
        for stat in STATS {
            let factors: Vec<f64> = [1, 2, 4, 8]
                .into_iter()
                .map(|looks| alpha(stat, 24, 1e-4, looks, 1.0).unwrap())
                .collect();
            assert!(
                factors.windows(2).all(|pair| pair[1] < pair[0]),
                "{stat:?}: {factors:?}"
            );
        }
    }

    #[test]
    fn correlation_raises_alpha() {
        for stat in STATS {
            for looks in [1, 4] {
                let factors: Vec<f64> = [1.0, 1.33, 2.0, 4.0]
                    .into_iter()
                    .map(|rho| alpha(stat, 32, 1e-4, looks, rho).unwrap())
                    .collect();
                assert!(
                    factors.windows(2).all(|pair| pair[1] > pair[0]),
                    "{stat:?}, L = {looks}: {factors:?}"
                );
            }
        }
    }

    #[test]
    fn os_pdf_integrates_to_one_when_correlated() {
        for cells in [16, 32, 50] {
            for looks in [1, 3] {
                for rank in [0.5, 0.75] {
                    let total = pfa(CfarStatistic::Os { rank }, cells, 0.0, looks, 1.33);
                    assert!(
                        (total - 1.0).abs() < 1e-6,
                        "N = {cells}, L = {looks}, rank = {rank}: {total}"
                    );
                }
            }
        }
        let greatest = pfa(CfarStatistic::Go, 32, 0.0, 2, 1.33);
        assert!((greatest - 1.0).abs() < 1e-6, "GO: {greatest}");
    }

    #[test]
    fn os_pfa_on_range_oversampled_noise_is_within_2x() {
        let samples = oversampled_noise(1_000_000, 0x5eed_0f0a);
        let correlation = range_correlation(&samples);
        assert!((correlation - 1.5).abs() < 0.02, "rho = {correlation}");
        let cells: Vec<f64> = samples.iter().map(|s| s.norm_sqr()).collect();
        let (guard, train, design) = (2, 16, 1e-3);
        let stat = CfarStatistic::Os { rank: 0.75 };
        let corrected = alpha(stat, 2 * train, design, 1, correlation).unwrap();
        let measured = false_alarms(stat, &cells, guard, train, corrected);
        assert!(
            measured > 0.5 * design && measured < 2.0 * design,
            "measured {measured} for design {design} (rho {correlation}, alpha {corrected})"
        );
        let naive = alpha(stat, 2 * train, design, 1, 1.0).unwrap();
        assert!(naive < corrected);
    }

    #[test]
    fn bad_inputs_are_refused() {
        let os = CfarStatistic::Os { rank: 0.75 };
        assert_eq!(
            alpha(CfarStatistic::Ca, 0, 1e-3, 1, 1.0),
            Err(ThresholdError::Cells(0))
        );
        assert_eq!(
            alpha(CfarStatistic::Ca, 16, 1e-3, 0, 1.0),
            Err(ThresholdError::Looks)
        );
        assert_eq!(
            alpha(CfarStatistic::Ca, 16, 1e-3, 1, 0.5),
            Err(ThresholdError::Correlation(0.5))
        );
        assert_eq!(
            alpha(CfarStatistic::Os { rank: 0.0 }, 16, 1e-3, 1, 1.0),
            Err(ThresholdError::Rank(0.0))
        );
        assert_eq!(
            alpha(CfarStatistic::Os { rank: 1.5 }, 16, 1e-3, 1, 1.0),
            Err(ThresholdError::Rank(1.5))
        );
        assert_eq!(alpha(os, 16, 1.0, 1, 1.0), Err(ThresholdError::Pfa(1.0)));
        assert_eq!(alpha(os, 16, 0.0, 1, 1.0), Err(ThresholdError::Pfa(0.0)));
        assert_eq!(
            alpha(CfarStatistic::Go, 1, 1e-3, 1, 1.0),
            Err(ThresholdError::Cells(1))
        );
        assert_eq!(
            alpha(CfarStatistic::Ca, 1, 1e-12, 1, 1.0),
            Err(ThresholdError::Unreachable(1e-12))
        );
        assert!(pfa(CfarStatistic::Ca, 16, -1.0, 1, 1.0).is_nan());
        assert!(pfa(CfarStatistic::Ca, 0, 1.0, 1, 1.0).is_nan());
        assert_eq!(pfa(CfarStatistic::Ca, 16, 0.0, 2, 1.0), 1.0);
    }

    #[test]
    fn os_order_stays_inside_the_window() {
        assert_eq!(os_order(0.75, 32), 24);
        assert_eq!(os_order(0.01, 8), 1);
        assert_eq!(os_order(1.0, 8), 8);
    }
}
