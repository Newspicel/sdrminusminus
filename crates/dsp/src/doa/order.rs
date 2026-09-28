use super::{DOMINANT_WITHIN_DB, NOISE_ABOVE_DB, OrderRule, STABLE_REPORTS, SquelchConfig};

const TINY_EIGENVALUE: f64 = 1e-300;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCounter {
    adopted: Option<usize>,
    candidate: usize,
    repeats: u8,
}

impl SourceCounter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            adopted: None,
            candidate: 0,
            repeats: 0,
        }
    }

    pub const fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn update(
        &mut self,
        rule: OrderRule,
        eigenvalues_desc: &[f32],
        snapshots: f64,
    ) -> (usize, usize) {
        let raw = match rule {
            OrderRule::Dominance => dominance_count(eigenvalues_desc),
            OrderRule::Mdl => mdl_count(eigenvalues_desc, snapshots),
        };
        (self.adopt(raw), raw)
    }

    fn adopt(&mut self, raw: usize) -> usize {
        let Some(current) = self.adopted.filter(|&current| current != raw) else {
            self.adopted = Some(raw);
            self.repeats = 0;
            return raw;
        };
        if self.repeats > 0 && self.candidate == raw {
            self.repeats = self.repeats.saturating_add(1);
        } else {
            self.candidate = raw;
            self.repeats = 1;
        }
        if self.repeats >= STABLE_REPORTS {
            self.adopted = Some(raw);
            self.repeats = 0;
            return raw;
        }
        current
    }
}

#[must_use]
pub fn dominance_count(eigenvalues_desc: &[f32]) -> usize {
    let Some(&top) = eigenvalues_desc.first() else {
        return 0;
    };
    let mut count = 0;
    for k in 0..eigenvalues_desc.len().saturating_sub(1) {
        let value = eigenvalues_desc[k];
        let rest = &eigenvalues_desc[k + 1..];
        let floor = rest.iter().sum::<f32>() / rest.len() as f32;
        if !(value > 0.0 && floor > 0.0) {
            break;
        }
        let above = decibels(value / floor);
        let within = decibels(top / value);
        if above >= NOISE_ABOVE_DB && within <= DOMINANT_WITHIN_DB {
            count += 1;
        } else {
            break;
        }
    }
    count
}

#[must_use]
pub fn mdl_count(eigenvalues_desc: &[f32], snapshots: f64) -> usize {
    let snapshots = snapshots.max(1.0);
    (0..eigenvalues_desc.len())
        .map(|k| (k, description_length(eigenvalues_desc, k, snapshots)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(k, _)| k)
}

fn description_length(eigenvalues_desc: &[f32], k: usize, snapshots: f64) -> f64 {
    let m = eigenvalues_desc.len() as f64;
    let tail = &eigenvalues_desc[k..];
    let len = tail.len() as f64;
    let arithmetic = tail.iter().map(|&v| f64::from(v)).sum::<f64>() / len;
    let geometric = (tail
        .iter()
        .map(|&v| f64::from(v).max(TINY_EIGENVALUE).ln())
        .sum::<f64>()
        / len)
        .exp();
    let fit = if arithmetic > 0.0 {
        -snapshots * len * (geometric / arithmetic).ln()
    } else {
        0.0
    };
    let k = k as f64;
    fit + 0.5 * k * (2.0 * m - k) * snapshots.ln()
}

fn decibels(ratio: f32) -> f32 {
    10.0 * ratio.log10()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Squelch {
    open: bool,
}

impl Squelch {
    #[must_use]
    pub const fn new() -> Self {
        Self { open: false }
    }

    pub const fn reset(&mut self) {
        self.open = false;
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub fn update(&mut self, config: Option<SquelchConfig>, ratio_db: f32) -> bool {
        self.open = match config {
            None => true,
            Some(config) if self.open => ratio_db >= config.open_db - config.hysteresis_db,
            Some(config) => ratio_db >= config.open_db,
        };
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise_with(sources: &[f32]) -> Vec<f32> {
        let mut values: Vec<f32> = sources.to_vec();
        values.extend([1.02, 1.0, 0.99, 0.97, 0.95].iter().take(5 - sources.len()));
        values
    }

    #[test]
    fn dominance_counts_sources_well_above_the_floor() {
        assert_eq!(dominance_count(&noise_with(&[])), 0);
        assert_eq!(dominance_count(&noise_with(&[51.0])), 1);
        assert_eq!(dominance_count(&noise_with(&[51.0, 13.5])), 2);
        assert_eq!(dominance_count(&noise_with(&[1e4, 40.0])), 1);
        assert_eq!(dominance_count(&[]), 0);
        assert_eq!(dominance_count(&[3.0]), 0);
        assert_eq!(dominance_count(&[5.0, 0.0, 0.0]), 0);
    }

    #[test]
    fn mdl_finds_the_knee() {
        assert_eq!(mdl_count(&noise_with(&[]), 4096.0), 0);
        assert_eq!(mdl_count(&noise_with(&[51.0]), 4096.0), 1);
        assert_eq!(mdl_count(&noise_with(&[1e4, 40.0]), 4096.0), 2);
        assert_eq!(mdl_count(&[], 4096.0), 0);
    }

    #[test]
    fn source_count_waits_three_reports_before_changing() {
        let mut counter = SourceCounter::new();
        let one = noise_with(&[51.0]);
        let two = noise_with(&[51.0, 13.5]);
        assert_eq!(counter.update(OrderRule::Dominance, &one, 4096.0), (1, 1));
        assert_eq!(counter.update(OrderRule::Dominance, &two, 4096.0), (1, 2));
        assert_eq!(counter.update(OrderRule::Dominance, &two, 4096.0), (1, 2));
        assert_eq!(counter.update(OrderRule::Dominance, &two, 4096.0), (2, 2));
        assert_eq!(counter.update(OrderRule::Dominance, &one, 4096.0), (2, 1));
        assert_eq!(counter.update(OrderRule::Dominance, &two, 4096.0), (2, 2));
        assert_eq!(counter.update(OrderRule::Dominance, &one, 4096.0), (2, 1));
        assert_eq!(counter.update(OrderRule::Dominance, &one, 4096.0), (2, 1));
        counter.reset();
        assert_eq!(counter.update(OrderRule::Dominance, &one, 4096.0), (1, 1));
    }

    #[test]
    fn squelch_opens_above_threshold_and_closes_below_hysteresis() {
        let config = Some(SquelchConfig {
            open_db: 6.0,
            hysteresis_db: 1.0,
        });
        let mut squelch = Squelch::new();
        assert!(!squelch.update(config, 5.9));
        assert!(squelch.update(config, 6.0));
        assert!(squelch.update(config, 5.5));
        assert!(!squelch.update(config, 4.9));
        assert!(!squelch.update(config, 5.5));
        assert!(!squelch.update(config, f32::NAN));
        assert!(squelch.update(None, -40.0));
        assert!(squelch.is_open());
        squelch.reset();
        assert!(!squelch.is_open());
    }
}
