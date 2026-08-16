use std::{collections::HashMap, ops::RangeInclusive};

use sdrmm_wire::{OccupancyBucket, OccupancyReport};

pub const BUCKET_HZ: u64 = 12_500;

pub const OCCUPIED_MARGIN_DB: f32 = 10.0;

pub const MAX_BUCKETS: usize = 8192;

const HOURS: usize = 24;

#[derive(Clone, Debug, Default)]
struct Bucket {
    seen: [u64; HOURS],
    occupied: [u64; HOURS],
    last_seen_ms: i64,
}

impl Bucket {
    fn totals(&self) -> (u64, u64) {
        (self.seen.iter().sum(), self.occupied.iter().sum())
    }
}

#[derive(Debug, Default)]
pub struct Occupancy {
    buckets: HashMap<u64, Bucket>,
    started_ms: Option<i64>,
}

impl Occupancy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(
        &mut self,
        db: &[f32],
        center_hz: f64,
        span_hz: f32,
        lo_guard: Option<RangeInclusive<usize>>,
        now_ms: i64,
    ) {
        let n = db.len();
        if n == 0 || !span_hz.is_finite() || span_hz <= 0.0 || !center_hz.is_finite() {
            return;
        }
        let Some(floor) = noise_floor(db) else {
            return;
        };
        let threshold = floor + OCCUPIED_MARGIN_DB;
        self.started_ms.get_or_insert(now_ms);
        let hour = hour_of_day(now_ms);
        let bin_hz = f64::from(span_hz) / n as f64;
        let first_hz = center_hz - f64::from(span_hz) / 2.0;

        let mut current: Option<(u64, bool)> = None;
        for (i, &level) in db.iter().enumerate() {
            if !level.is_finite() || lo_guard.as_ref().is_some_and(|g| g.contains(&i)) {
                continue;
            }
            let hz = first_hz + (i as f64 + 0.5) * bin_hz;
            if hz < 0.0 {
                continue;
            }
            let key = (hz as u64) / BUCKET_HZ;
            let busy = level >= threshold;
            match &mut current {
                Some((open, seen_busy)) if *open == key => *seen_busy |= busy,
                Some((open, seen_busy)) => {
                    let (was, busy_was) = (*open, *seen_busy);
                    self.record(was, busy_was, hour, now_ms);
                    current = Some((key, busy));
                }
                None => current = Some((key, busy)),
            }
        }
        if let Some((key, busy)) = current {
            self.record(key, busy, hour, now_ms);
        }
        self.evict();
    }

    fn record(&mut self, key: u64, busy: bool, hour: usize, now_ms: i64) {
        let bucket = self.buckets.entry(key).or_default();
        bucket.seen[hour] += 1;
        if busy {
            bucket.occupied[hour] += 1;
        }
        bucket.last_seen_ms = now_ms;
    }

    fn evict(&mut self) {
        if self.buckets.len() <= MAX_BUCKETS {
            return;
        }
        let mut ages: Vec<(u64, i64)> = self
            .buckets
            .iter()
            .map(|(key, bucket)| (*key, bucket.last_seen_ms))
            .collect();
        ages.sort_unstable_by_key(|(_, last)| *last);
        for (key, _) in ages.iter().take(self.buckets.len() - MAX_BUCKETS) {
            self.buckets.remove(key);
        }
    }

    #[must_use]
    pub fn report(&self, min_samples: u64) -> OccupancyReport {
        let mut buckets: Vec<OccupancyBucket> = self
            .buckets
            .iter()
            .filter_map(|(key, bucket)| {
                let (seen, occupied) = bucket.totals();
                if seen < min_samples.max(1) {
                    return None;
                }
                Some(OccupancyBucket {
                    freq_hz: key * BUCKET_HZ + BUCKET_HZ / 2,
                    duty: occupied as f32 / seen as f32,
                    samples: seen,
                    by_hour: (0..HOURS)
                        .map(|hour| {
                            let seen = bucket.seen[hour];
                            if seen == 0 {
                                0.0
                            } else {
                                bucket.occupied[hour] as f32 / seen as f32
                            }
                        })
                        .collect(),
                    last_seen: stamp(bucket.last_seen_ms),
                })
            })
            .collect();
        buckets.sort_unstable_by(|a, b| {
            b.duty
                .partial_cmp(&a.duty)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.freq_hz.cmp(&b.freq_hz))
        });
        OccupancyReport {
            bucket_hz: BUCKET_HZ,
            since: self.started_ms.map(stamp).unwrap_or_default(),
            buckets,
        }
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
        self.started_ms = None;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

fn noise_floor(db: &[f32]) -> Option<f32> {
    let mut finite: Vec<f32> = db.iter().copied().filter(|v| v.is_finite()).collect();
    let last = finite.len().checked_sub(1)?;
    let at = last / 4;
    let (_, nth, _) = finite.select_nth_unstable_by(at, f32::total_cmp);
    Some(*nth)
}

fn hour_of_day(now_ms: i64) -> usize {
    let seconds = now_ms.div_euclid(1000);
    (seconds.rem_euclid(86_400) / 3600) as usize % HOURS
}

fn stamp(now_ms: i64) -> String {
    jiff::Timestamp::from_millisecond(now_ms)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize, carrier: Option<usize>) -> Vec<f32> {
        let mut db = vec![-100.0f32; n];
        if let Some(at) = carrier {
            db[at] = -40.0;
        }
        db
    }

    const HOUR_MS: i64 = 3_600_000;

    #[test]
    fn a_carrier_reads_as_occupied_and_the_noise_around_it_does_not() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 0);

        let report = occupancy.report(1);
        let busy: Vec<&OccupancyBucket> = report.buckets.iter().filter(|b| b.duty > 0.0).collect();
        assert_eq!(busy.len(), 1, "more than the carrier read as occupied");
        assert_eq!(busy[0].duty, 1.0);
        assert!(
            (busy[0].freq_hz as f64 - 100e6).abs() <= BUCKET_HZ as f64,
            "carrier filed at {} Hz",
            busy[0].freq_hz
        );
    }

    #[test]
    fn the_front_ends_own_spike_is_not_filed_as_a_busy_frequency() {
        let mut occupancy = Occupancy::new();
        for _ in 0..10 {
            occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, Some(62..=66), 0);
        }
        let report = occupancy.report(1);
        assert!(
            report.buckets.iter().all(|b| b.duty == 0.0),
            "a guarded spike was recorded as occupancy"
        );
        assert!(
            !report.buckets.is_empty(),
            "guarding the LO silenced the whole sweep"
        );
    }

    #[test]
    fn duty_is_the_fraction_of_observations_that_were_busy() {
        let mut occupancy = Occupancy::new();
        for i in 0..10 {
            occupancy.observe(&frame(128, (i < 3).then_some(64)), 100e6, 1.6e6, None, 0);
        }
        let report = occupancy.report(1);
        let carrier = report
            .buckets
            .iter()
            .find(|b| b.duty > 0.0)
            .expect("the carrier bucket");
        assert!((carrier.duty - 0.3).abs() < 1e-6, "duty {}", carrier.duty);
        assert_eq!(carrier.samples, 10);
    }

    #[test]
    fn the_hour_histogram_separates_a_busy_hour_from_a_quiet_one() {
        let mut occupancy = Occupancy::new();
        for _ in 0..10 {
            occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 7 * HOUR_MS);
        }
        for _ in 0..10 {
            occupancy.observe(&frame(128, None), 100e6, 1.6e6, None, 3 * HOUR_MS);
        }

        let report = occupancy.report(1);
        let carrier = report
            .buckets
            .iter()
            .find(|b| b.duty > 0.0)
            .expect("the carrier bucket");
        assert_eq!(carrier.by_hour.len(), 24);
        assert_eq!(carrier.by_hour[7], 1.0);
        assert_eq!(carrier.by_hour[3], 0.0);
        assert_eq!(carrier.by_hour[12], 0.0);
        assert!((carrier.duty - 0.5).abs() < 1e-6);
    }

    #[test]
    fn retuning_files_observations_under_absolute_frequency() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 0);
        occupancy.observe(&frame(128, Some(32)), 100.4e6, 1.6e6, None, 0);

        let report = occupancy.report(1);
        let busy: Vec<&OccupancyBucket> = report.buckets.iter().filter(|b| b.duty > 0.0).collect();
        assert_eq!(busy.len(), 1, "one carrier was filed as two frequencies");
        assert_eq!(busy[0].samples, 2, "the two sightings did not accumulate");
    }

    #[test]
    fn a_thinly_observed_bucket_is_left_out_of_the_report() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 0);
        assert!(occupancy.report(2).buckets.is_empty());
        assert!(!occupancy.report(1).buckets.is_empty());
    }

    #[test]
    fn the_report_leads_with_the_busiest() {
        let mut occupancy = Occupancy::new();
        for i in 0..10 {
            occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 0);
            occupancy.observe(&frame(128, (i < 3).then_some(96)), 100e6, 1.6e6, None, 0);
        }
        let report = occupancy.report(1);
        assert!(report.buckets.len() >= 2);
        assert!(report.buckets[0].duty > report.buckets[1].duty);
    }

    #[test]
    fn an_empty_or_impossible_frame_is_ignored_rather_than_recorded() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&[], 100e6, 1.6e6, None, 0);
        occupancy.observe(&frame(128, None), 100e6, 0.0, None, 0);
        occupancy.observe(&frame(128, None), f64::NAN, 1.6e6, None, 0);
        occupancy.observe(&vec![f32::NEG_INFINITY; 128], 100e6, 1.6e6, None, 0);
        assert!(occupancy.is_empty());
    }

    #[test]
    fn clearing_forgets_everything_including_when_it_started() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, None, 0);
        assert!(!occupancy.report(1).since.is_empty());
        occupancy.clear();
        assert!(occupancy.is_empty());
        assert!(occupancy.report(1).since.is_empty());
    }

    #[test]
    fn the_oldest_buckets_are_dropped_once_the_cap_is_passed() {
        let mut occupancy = Occupancy::new();
        for step in 0..(MAX_BUCKETS / 64 + 40) {
            let center = 100e6 + step as f64 * 800e3;
            occupancy.observe(&frame(64, None), center, 800e3, None, step as i64 * 1000);
        }
        assert!(
            occupancy.len() <= MAX_BUCKETS,
            "grew to {} buckets",
            occupancy.len()
        );
        let report = occupancy.report(1);
        let highest = report.buckets.iter().map(|b| b.freq_hz).max().unwrap_or(0);
        assert!(
            highest > 100_000_000,
            "the newest observations were evicted"
        );
    }

    #[test]
    fn hour_of_day_wraps_the_clock_in_both_directions() {
        assert_eq!(hour_of_day(0), 0);
        assert_eq!(hour_of_day(7 * HOUR_MS), 7);
        assert_eq!(hour_of_day(25 * HOUR_MS), 1);
        assert!(hour_of_day(-HOUR_MS) < 24);
    }
}
