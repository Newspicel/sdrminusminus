use num_complex::Complex;
use sdrmm_wire::{ArrayCalRecord, CalSourceKind, LaneKey, LaneSolution, array::EQ_POINTS};

use super::capture::SolveSummary;

pub(crate) const WARM_CENTER_MIN_HZ: f64 = 1e6;
pub(crate) const WARM_CENTER_FRACTION: f64 = 0.005;
pub(crate) const WARM_GAIN_DB: f64 = 0.5;
const RATE_TOLERANCE: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WarmUse {
    pub(crate) gain: bool,
    pub(crate) equaliser: bool,
    pub(crate) delay_prior: bool,
    pub(crate) phase: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Band {
    pub(crate) center_hz: f64,
    pub(crate) sample_rate: f64,
    pub(crate) gain_db: Option<f64>,
    pub(crate) keeps_phase: bool,
}

pub(crate) fn assess(
    record: &ArrayCalRecord,
    lanes: &[LaneKey],
    center_hz: f64,
    rate: f64,
    gain_db: Option<f64>,
    keeps_phase: bool,
) -> Option<WarmUse> {
    let same_lanes = record.lanes == lanes && record.solution.len() == lanes.len();
    let same_rate = (record.sample_rate - rate).abs() <= RATE_TOLERANCE * rate.abs().max(1.0);
    let tolerance = WARM_CENTER_MIN_HZ.max(WARM_CENTER_FRACTION * center_hz.abs());
    let near = (record.center_hz - center_hz).abs() <= tolerance;
    let same_gain = match (record.gain_db, gain_db) {
        (Some(stored), Some(now)) => (stored - now).abs() <= WARM_GAIN_DB,
        (None, None) => true,
        _ => false,
    };
    if !(same_lanes && same_rate && near && same_gain) {
        return None;
    }
    Some(WarmUse {
        gain: true,
        equaliser: record
            .solution
            .iter()
            .all(|lane| lane.equaliser.len() == EQ_POINTS),
        delay_prior: true,
        phase: record.keeps_phase && keeps_phase,
    })
}

pub(crate) fn record(
    lanes: &[LaneKey],
    band: &Band,
    source: CalSourceKind,
    solution: &SolveSummary,
    delays: &[f64],
    eq: &[Vec<Complex<f32>>],
) -> ArrayCalRecord {
    let count = lanes
        .len()
        .min(usize::from(solution.lanes))
        .min(delays.len());
    ArrayCalRecord {
        lanes: lanes[..count].to_vec(),
        center_hz: band.center_hz,
        sample_rate: band.sample_rate,
        gain_db: band.gain_db,
        source,
        keeps_phase: band.keeps_phase,
        solved_at: format!("{:.3}", jiff::Timestamp::now()),
        solution: (0..count)
            .map(|lane| LaneSolution {
                delay_samples: delays[lane],
                phase_deg: f64::from(solution.phase_deg[lane]),
                gain_db: f64::from(solution.gain_db[lane]),
                coherence: solution.coherence[lane],
                equaliser: eq
                    .get(lane)
                    .map(|points| points.iter().map(|point| [point.re, point.im]).collect())
                    .unwrap_or_default(),
            })
            .collect(),
    }
}

pub(crate) fn equaliser(lane: &LaneSolution) -> Option<Vec<Complex<f32>>> {
    (lane.equaliser.len() == EQ_POINTS).then(|| {
        lane.equaliser
            .iter()
            .map(|[re, im]| Complex::new(*re, *im))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KRAKEN_RATE: f64 = 2_400_000.0;

    fn keys(device: &str, lanes: u32) -> Vec<LaneKey> {
        (0..lanes)
            .map(|stream| LaneKey {
                device: device.to_owned(),
                stream,
            })
            .collect()
    }

    fn summary(lanes: usize) -> SolveSummary {
        let mut summary = SolveSummary {
            lanes: lanes as u8,
            phase_ready: true,
            gain_ready: true,
            ..SolveSummary::default()
        };
        for lane in 1..lanes {
            summary.phase_deg[lane] = 12.5 * lane as f32;
            summary.gain_db[lane] = -0.5 * lane as f32;
            summary.coherence[lane] = 0.99;
        }
        summary.coherence[0] = 1.0;
        summary
    }

    fn solved(center_hz: f64, keeps_phase: bool, eq: bool) -> ArrayCalRecord {
        let lanes = keys("kraken:1000", 5);
        let delays: Vec<f64> = (0..5).map(|lane| 100.0 * lane as f64 + 0.25).collect();
        let points = if eq {
            vec![vec![Complex::new(1.0, 0.1); EQ_POINTS]; 5]
        } else {
            Vec::new()
        };
        record(
            &lanes,
            &Band {
                center_hz,
                sample_rate: KRAKEN_RATE,
                gain_db: Some(30.0),
                keeps_phase,
            },
            CalSourceKind::Noise,
            &summary(5),
            &delays,
            &points,
        )
    }

    #[test]
    fn a_matching_record_warms_gain_and_eq_but_not_bank_phase() {
        let stored = solved(433.92e6, false, true);
        let warm = assess(
            &stored,
            &keys("kraken:1000", 5),
            434.5e6,
            KRAKEN_RATE,
            Some(30.2),
            false,
        );
        assert_eq!(
            warm,
            Some(WarmUse {
                gain: true,
                equaliser: true,
                delay_prior: true,
                phase: false,
            })
        );
        let plain = solved(433.92e6, false, false);
        let warm = assess(
            &plain,
            &keys("kraken:1000", 5),
            433.92e6,
            KRAKEN_RATE,
            Some(30.0),
            false,
        );
        assert_eq!(warm.map(|warm| warm.equaliser), Some(false));
    }

    #[test]
    fn a_record_from_another_band_is_ignored() {
        let stored = solved(868e6, false, true);
        let lanes = keys("kraken:1000", 5);
        assert_eq!(
            assess(&stored, &lanes, 433.92e6, KRAKEN_RATE, Some(30.0), false),
            None
        );
        assert_eq!(
            assess(&stored, &lanes, 868e6 + 5e6, KRAKEN_RATE, Some(30.0), false),
            None
        );
        assert!(assess(&stored, &lanes, 868e6 + 4e6, KRAKEN_RATE, Some(30.0), false).is_some());
        assert_eq!(
            assess(&stored, &lanes, 868e6, 2_048_000.0, Some(30.0), false),
            None
        );
        assert_eq!(
            assess(&stored, &lanes, 868e6, KRAKEN_RATE, Some(31.0), false),
            None
        );
        assert_eq!(
            assess(&stored, &lanes, 868e6, KRAKEN_RATE, None, false),
            None
        );
        assert_eq!(
            assess(
                &stored,
                &keys("kraken:1001", 5),
                868e6,
                KRAKEN_RATE,
                Some(30.0),
                false
            ),
            None
        );
        let mut swapped = keys("kraken:1000", 5);
        swapped.swap(1, 2);
        assert_eq!(
            assess(&stored, &swapped, 868e6, KRAKEN_RATE, Some(30.0), false),
            None
        );
    }

    #[test]
    fn phase_keeping_hardware_warms_phase() {
        let lanes = keys("kraken:1000", 5);
        let keeping = solved(100e6, true, false);
        let warm = assess(&keeping, &lanes, 100e6, KRAKEN_RATE, Some(30.0), true);
        assert_eq!(warm.map(|warm| warm.phase), Some(true));
        let warm = assess(&keeping, &lanes, 100e6, KRAKEN_RATE, Some(30.0), false);
        assert_eq!(warm.map(|warm| warm.phase), Some(false));
        let losing = solved(100e6, false, false);
        let warm = assess(&losing, &lanes, 100e6, KRAKEN_RATE, Some(30.0), true);
        assert_eq!(warm.map(|warm| warm.phase), Some(false));
    }

    #[test]
    fn a_solved_record_carries_lanes_delays_and_equalisers() {
        let stored = solved(433.92e6, false, true);
        assert_eq!(stored.lanes.len(), 5);
        assert_eq!(stored.solution.len(), 5);
        assert!((stored.solution[3].delay_samples - 300.25).abs() < 1e-12);
        assert!((stored.solution[2].phase_deg - 25.0).abs() < 1e-6);
        assert!((stored.solution[4].gain_db + 2.0).abs() < 1e-6);
        let points = equaliser(&stored.solution[1]).expect("an equaliser");
        assert_eq!(points.len(), EQ_POINTS);
        assert!((points[0] - Complex::new(1.0, 0.1)).norm() < 1e-6);
        assert!(stored.solved_at.contains('T'));
    }
}
