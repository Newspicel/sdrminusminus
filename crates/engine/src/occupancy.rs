//! Band-occupancy analytics: how much of the time each slice of spectrum is actually in use.
//!
//! Fed from the same spectrum snapshots the waterfall is drawn from, so it costs no extra DSP. A
//! bin counts as occupied when it stands far enough above that frame's own noise floor — the
//! floor is measured per frame rather than configured, because it moves with the gain, the
//! antenna and the band.
//!
//! Accumulated against *absolute* frequency, not against the display span: the radio retunes, a
//! scanner retunes constantly, and the question this answers — "when is this frequency busy" —
//! outlives any one tuning.
//!
//! The hour-of-day histogram is the point of the whole thing. A single duty-cycle number says a
//! repeater is used 8% of the time; the histogram says it is used at seven in the morning.

use std::collections::HashMap;

use sdrmm_wire::{OccupancyBucket, OccupancyReport};

/// Width of one frequency bucket. 12.5 kHz is the narrowest channel spacing in common use, so a
/// bucket holds at most one channel and never straddles two.
pub const BUCKET_HZ: u64 = 12_500;

/// How far above the frame's noise floor a bin must sit to count as occupied. Wide enough that
/// floor wander does not register as traffic, narrow enough to catch a weak signal.
pub const OCCUPIED_MARGIN_DB: f32 = 10.0;

/// Frequency buckets kept. A day of scanning a busy band reaches a few thousand; past this the
/// least recently seen are dropped, so an unattended server cannot grow without bound.
pub const MAX_BUCKETS: usize = 8192;

const HOURS: usize = 24;

#[derive(Clone, Debug, Default)]
struct Bucket {
    /// Observations, and how many of them were occupied, per hour of the day (UTC).
    seen: [u64; HOURS],
    occupied: [u64; HOURS],
    /// Milliseconds since the epoch, for the retention sweep and the report.
    last_seen_ms: i64,
}

impl Bucket {
    fn totals(&self) -> (u64, u64) {
        (self.seen.iter().sum(), self.occupied.iter().sum())
    }
}

/// Rolling occupancy statistics, keyed by frequency bucket.
#[derive(Debug, Default)]
pub struct Occupancy {
    buckets: HashMap<u64, Bucket>,
    /// When the accumulator started collecting, for the report's `since`.
    started_ms: Option<i64>,
}

impl Occupancy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one spectrum frame in.
    ///
    /// `db` is DC-centred, so bin `i` sits at `center_hz + (i - n/2) · span/n`. `now_ms` is the
    /// wall clock the observation is filed under — passed in rather than read here so the whole
    /// accumulator stays testable without a clock.
    pub fn observe(&mut self, db: &[f32], center_hz: f64, span_hz: f32, now_ms: i64) {
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

        // One pass over the bins, folding each into its bucket. Several bins share a bucket at any
        // sane span, so the bucket is occupied if *any* of them is: a narrow carrier does not stop
        // being traffic because the bucket around it is quiet.
        let mut current: Option<(u64, bool)> = None;
        for (i, &level) in db.iter().enumerate() {
            if !level.is_finite() {
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

    /// Drop the least recently seen buckets once the cap is passed. Batched — evicting one per
    /// insertion would scan the whole map on every frame of a scan.
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

    /// What has been seen, busiest first.
    ///
    /// `min_samples` drops buckets too thinly observed to mean anything — a scanner that swept
    /// past a frequency twice has not measured its duty cycle.
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

/// The frame's noise floor: the level a quarter of its bins sit below. The same shape of estimate
/// the display's own dB window uses, and for the same reason — a mean would be dragged upward by
/// the very carriers being detected.
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

    /// A flat noise floor with a carrier planted in one bin.
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
        // 128 bins over 1.6 MHz centred on 100 MHz: 12.5 kHz per bin, one bin per bucket.
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 0);

        let report = occupancy.report(1);
        let busy: Vec<&OccupancyBucket> = report.buckets.iter().filter(|b| b.duty > 0.0).collect();
        assert_eq!(busy.len(), 1, "more than the carrier read as occupied");
        assert_eq!(busy[0].duty, 1.0);
        // Bin 64 of 128 is the centre bin, which starts at the centre frequency.
        assert!(
            (busy[0].freq_hz as f64 - 100e6).abs() <= BUCKET_HZ as f64,
            "carrier filed at {} Hz",
            busy[0].freq_hz
        );
    }

    #[test]
    fn duty_is_the_fraction_of_observations_that_were_busy() {
        let mut occupancy = Occupancy::new();
        for i in 0..10 {
            occupancy.observe(&frame(128, (i < 3).then_some(64)), 100e6, 1.6e6, 0);
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

    /// The reason the histogram exists: a frequency busy only in the morning must read as busy
    /// only in the morning, not as uniformly quiet.
    #[test]
    fn the_hour_histogram_separates_a_busy_hour_from_a_quiet_one() {
        let mut occupancy = Occupancy::new();
        for _ in 0..10 {
            occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 7 * HOUR_MS);
        }
        for _ in 0..10 {
            occupancy.observe(&frame(128, None), 100e6, 1.6e6, 3 * HOUR_MS);
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
        // An hour never observed reads zero rather than as a hole.
        assert_eq!(carrier.by_hour[12], 0.0);
        // And the overall figure is the average of the two, not either one.
        assert!((carrier.duty - 0.5).abs() < 1e-6);
    }

    #[test]
    fn retuning_files_observations_under_absolute_frequency() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 0);
        // The radio moves; the same carrier is now at a different *bin*, same frequency.
        occupancy.observe(&frame(128, Some(32)), 100.4e6, 1.6e6, 0);

        let report = occupancy.report(1);
        let busy: Vec<&OccupancyBucket> = report.buckets.iter().filter(|b| b.duty > 0.0).collect();
        assert_eq!(busy.len(), 1, "one carrier was filed as two frequencies");
        assert_eq!(busy[0].samples, 2, "the two sightings did not accumulate");
    }

    #[test]
    fn a_thinly_observed_bucket_is_left_out_of_the_report() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 0);
        assert!(occupancy.report(2).buckets.is_empty());
        assert!(!occupancy.report(1).buckets.is_empty());
    }

    #[test]
    fn the_report_leads_with_the_busiest() {
        let mut occupancy = Occupancy::new();
        for i in 0..10 {
            // One carrier always on, another on a third of the time.
            occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 0);
            occupancy.observe(&frame(128, (i < 3).then_some(96)), 100e6, 1.6e6, 0);
        }
        let report = occupancy.report(1);
        assert!(report.buckets.len() >= 2);
        assert!(report.buckets[0].duty > report.buckets[1].duty);
    }

    #[test]
    fn an_empty_or_impossible_frame_is_ignored_rather_than_recorded() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&[], 100e6, 1.6e6, 0);
        occupancy.observe(&frame(128, None), 100e6, 0.0, 0);
        occupancy.observe(&frame(128, None), f64::NAN, 1.6e6, 0);
        occupancy.observe(&vec![f32::NEG_INFINITY; 128], 100e6, 1.6e6, 0);
        assert!(occupancy.is_empty());
    }

    #[test]
    fn clearing_forgets_everything_including_when_it_started() {
        let mut occupancy = Occupancy::new();
        occupancy.observe(&frame(128, Some(64)), 100e6, 1.6e6, 0);
        assert!(!occupancy.report(1).since.is_empty());
        occupancy.clear();
        assert!(occupancy.is_empty());
        assert!(occupancy.report(1).since.is_empty());
    }

    /// An unattended server sweeping for days must not grow without bound.
    #[test]
    fn the_oldest_buckets_are_dropped_once_the_cap_is_passed() {
        let mut occupancy = Occupancy::new();
        // Each frame is a fresh megahertz, so every one brings new buckets.
        for step in 0..(MAX_BUCKETS / 64 + 40) {
            let center = 100e6 + step as f64 * 800e3;
            occupancy.observe(&frame(64, None), center, 800e3, step as i64 * 1000);
        }
        assert!(
            occupancy.len() <= MAX_BUCKETS,
            "grew to {} buckets",
            occupancy.len()
        );
        // And what survived is the recent end, not an arbitrary slice.
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
        // Before the epoch is not a real timestamp here, but it must not panic or index out of
        // range if a clock ever hands one over.
        assert!(hour_of_day(-HOUR_MS) < 24);
    }
}
