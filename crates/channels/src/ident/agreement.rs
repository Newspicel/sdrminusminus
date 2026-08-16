use std::collections::VecDeque;

use sdrmm_wire::Modulation;

use super::{classify::Verdict, detect::Band};

const WINDOWS: usize = 5;
const SAME_CENTRE: f64 = 0.5;
const SAME_WIDTH: f64 = 2.0;

#[derive(Clone, Copy)]
struct Seen {
    modulation: Modulation,
    confidence: f32,
}

pub(crate) struct Agreement {
    seen: VecDeque<Seen>,
    band: Option<(f64, f64)>,
}

impl Agreement {
    pub(crate) fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(WINDOWS),
            band: None,
        }
    }

    pub(crate) fn forget(&mut self) {
        self.seen.clear();
        self.band = None;
    }

    pub(crate) fn settle(&mut self, band: &Band, verdict: Verdict) -> Verdict {
        let here = (band.center_hz, band.bandwidth_hz);
        if !self.band.is_some_and(|there| same_signal(there, here)) {
            self.seen.clear();
        }
        self.band = Some(here);

        if self.seen.len() == WINDOWS {
            self.seen.pop_front();
        }
        self.seen.push_back(Seen {
            modulation: verdict.modulation,
            confidence: verdict.confidence,
        });

        let (modulation, weight, count) = self.majority(verdict.modulation);
        let total: f32 = self.seen.iter().map(|seen| seen.confidence).sum();
        let share = if total > 0.0 { weight / total } else { 1.0 };
        let mean = weight / count as f32;
        Verdict {
            modulation,
            confidence: (share * mean).clamp(0.0, 1.0),
            sideband: (modulation == verdict.modulation)
                .then_some(verdict.sideband)
                .flatten(),
        }
    }

    fn majority(&self, latest: Modulation) -> (Modulation, f32, usize) {
        let mut best = (latest, 0.0f32, 1usize);
        for candidate in self.seen.iter().map(|seen| seen.modulation) {
            let (weight, count) = self.weight_of(candidate);
            if weight > best.1 || (weight == best.1 && candidate == latest) {
                best = (candidate, weight, count);
            }
        }
        best
    }

    fn weight_of(&self, modulation: Modulation) -> (f32, usize) {
        let matching = self
            .seen
            .iter()
            .filter(|seen| seen.modulation == modulation);
        matching.fold((0.0, 0), |(weight, count), seen| {
            (weight + seen.confidence, count + 1)
        })
    }
}

fn same_signal(there: (f64, f64), here: (f64, f64)) -> bool {
    let narrower = there.1.min(here.1).max(1.0);
    let wider = there.1.max(here.1);
    (there.0 - here.0).abs() <= narrower * SAME_CENTRE && wider <= narrower * SAME_WIDTH
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(center_hz: f64, bandwidth_hz: f64) -> Band {
        Band {
            center_hz,
            bandwidth_hz,
            snr_db: 25.0,
            carrier_db: 3.0,
            flatness: 0.3,
            skew: 0.0,
            peak_hz: 0.0,
        }
    }

    fn verdict(modulation: Modulation, confidence: f32) -> Verdict {
        Verdict {
            modulation,
            confidence,
            sideband: None,
        }
    }

    #[test]
    fn a_single_window_is_reported_as_it_was_measured() {
        let mut agreement = Agreement::new();
        let settled = agreement.settle(&band(0.0, 12_500.0), verdict(Modulation::Fm, 0.8));
        assert_eq!(settled.modulation, Modulation::Fm);
        assert!(
            (settled.confidence - 0.8).abs() < 1e-6,
            "{}",
            settled.confidence
        );
    }

    #[test]
    fn one_odd_window_does_not_overturn_a_settled_reading() {
        let mut agreement = Agreement::new();
        let steady = band(0.0, 180_000.0);
        for _ in 0..3 {
            agreement.settle(&steady, verdict(Modulation::Fm, 0.8));
        }
        let settled = agreement.settle(&steady, verdict(Modulation::Ook, 0.7));
        assert_eq!(settled.modulation, Modulation::Fm);
    }

    #[test]
    fn disagreement_costs_confidence() {
        let mut agreement = Agreement::new();
        let steady = band(0.0, 180_000.0);
        let alone = agreement
            .settle(&steady, verdict(Modulation::Fm, 0.8))
            .confidence;
        agreement.settle(&steady, verdict(Modulation::Ook, 0.8));
        let disputed = agreement
            .settle(&steady, verdict(Modulation::Fm, 0.8))
            .confidence;
        assert!(disputed < alone, "{disputed} vs {alone}");
    }

    #[test]
    fn a_new_signal_starts_its_own_history() {
        let mut agreement = Agreement::new();
        for _ in 0..3 {
            agreement.settle(&band(0.0, 180_000.0), verdict(Modulation::Fm, 0.9));
        }
        let settled = agreement.settle(&band(60_000.0, 12_500.0), verdict(Modulation::Fsk4, 0.6));
        assert_eq!(settled.modulation, Modulation::Fsk4);
        assert!(
            (settled.confidence - 0.6).abs() < 1e-6,
            "{}",
            settled.confidence
        );
    }

    #[test]
    fn what_it_was_told_to_forget_stops_counting() {
        let mut agreement = Agreement::new();
        let steady = band(0.0, 12_500.0);
        for _ in 0..3 {
            agreement.settle(&steady, verdict(Modulation::Fm, 0.9));
        }
        agreement.forget();
        let settled = agreement.settle(&steady, verdict(Modulation::Ook, 0.5));
        assert_eq!(settled.modulation, Modulation::Ook);
    }
}
