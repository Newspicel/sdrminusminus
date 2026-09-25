use sdrmm_wire::{
    rest::{OccupancyBucket, OccupancyReport},
    units,
};

pub const HOURS: usize = 24;
pub const MAX_ROWS: usize = 60;
pub const MIN_SAMPLES: u32 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Busiest,
    Frequency,
}

#[must_use]
pub fn bucket_hz(hz: u64) -> String {
    units::hertz(hz as f64)
}

#[must_use]
pub fn rows(
    report: Option<&OccupancyReport>,
    sort: Sort,
    query: &str,
    limit: usize,
) -> Vec<OccupancyBucket> {
    let Some(report) = report else {
        return Vec::new();
    };
    let needle = query.trim().to_lowercase();
    let mut matched: Vec<OccupancyBucket> = report
        .buckets
        .iter()
        .filter(|bucket| {
            needle.is_empty() || bucket_hz(bucket.freq_hz).to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    if sort == Sort::Frequency {
        matched.sort_by_key(|bucket| bucket.freq_hz);
    }
    matched.truncate(limit);
    matched
}

#[must_use]
pub fn duty_text(duty: f32) -> String {
    if !duty.is_finite() || duty <= 0.0 {
        return String::from("-");
    }
    format!("{}%", (duty * 100.0).round())
}

#[must_use]
pub fn duty_alpha(duty: f32) -> f32 {
    if !duty.is_finite() || duty <= 0.0 {
        return 0.0;
    }
    duty.min(1.0).sqrt().min(1.0)
}

#[must_use]
pub fn busiest_hour(bucket: &OccupancyBucket) -> Option<usize> {
    let mut best = None;
    let mut peak = 0.0;
    for (hour, duty) in bucket.by_hour.iter().enumerate() {
        if *duty > peak {
            peak = *duty;
            best = Some(hour);
        }
    }
    best
}

#[must_use]
pub fn hour_text(hour: usize) -> String {
    format!("{hour:02}:00")
}

#[must_use]
pub fn has_occupancy(report: Option<&OccupancyReport>) -> bool {
    report.is_some_and(|report| !report.buckets.is_empty())
}

#[must_use]
pub fn row_hint(bucket: &OccupancyBucket) -> String {
    match busiest_hour(bucket) {
        Some(peak) => format!(
            "{}, busiest around {}, {} observations",
            bucket_hz(bucket.freq_hz),
            hour_text(peak),
            bucket.samples
        ),
        None => format!(
            "{}, {} observations",
            bucket_hz(bucket.freq_hz),
            bucket.samples
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(freq_hz: u64, duty: f32, by_hour: &[(usize, f32)]) -> OccupancyBucket {
        let mut hours = vec![0.0; HOURS];
        for (hour, value) in by_hour {
            hours[*hour] = *value;
        }
        OccupancyBucket {
            freq_hz,
            duty,
            samples: 100,
            by_hour: hours,
            last_seen: "2026-08-15T05:00:00Z".to_owned(),
        }
    }

    fn report(buckets: Vec<OccupancyBucket>) -> OccupancyReport {
        OccupancyReport {
            bucket_hz: 12_500,
            since: "2026-08-15T00:00:00Z".to_owned(),
            buckets,
        }
    }

    fn freqs(rows: &[OccupancyBucket]) -> Vec<u64> {
        rows.iter().map(|row| row.freq_hz).collect()
    }

    #[test]
    fn keeps_the_servers_busiest_first_order_or_sorts_by_frequency() {
        let two = report(vec![
            bucket(145_500_000, 0.4, &[]),
            bucket(144_800_000, 0.1, &[]),
        ]);
        assert_eq!(
            freqs(&rows(Some(&two), Sort::Busiest, "", MAX_ROWS)),
            [145_500_000, 144_800_000]
        );
        assert_eq!(
            freqs(&rows(Some(&two), Sort::Frequency, "", MAX_ROWS)),
            [144_800_000, 145_500_000]
        );
        assert_eq!(
            freqs(&rows(Some(&two), Sort::Busiest, "145.5", MAX_ROWS)),
            [145_500_000]
        );
    }

    #[test]
    fn caps_the_rows_and_has_nothing_before_a_report() {
        let many = report(
            (0..MAX_ROWS as u64 + 20)
                .map(|at| bucket(100_000_000 + at * 12_500, 0.5, &[]))
                .collect(),
        );
        assert_eq!(
            rows(Some(&many), Sort::Busiest, "", MAX_ROWS).len(),
            MAX_ROWS
        );
        assert_eq!(rows(Some(&many), Sort::Busiest, "", 5).len(), 5);
        assert!(rows(None, Sort::Busiest, "", MAX_ROWS).is_empty());
        assert!(!has_occupancy(None));
        assert!(!has_occupancy(Some(&report(Vec::new()))));
        assert!(has_occupancy(Some(&report(vec![bucket(
            1_000_000,
            0.0,
            &[]
        )]))));
    }

    #[test]
    fn lifts_the_low_end_without_claiming_a_quiet_frequency_is_busy() {
        assert!((duty_alpha(0.04) - 0.2).abs() < 1e-6);
        assert_eq!(duty_alpha(1.0), 1.0);
        assert_eq!(duty_alpha(0.0), 0.0);
        assert_eq!(duty_alpha(-1.0), 0.0);
        assert_eq!(duty_alpha(f32::NAN), 0.0);
        assert!(duty_alpha(0.5) > duty_alpha(0.2));
        assert_eq!(duty_alpha(4.0), 1.0);
    }

    #[test]
    fn finds_the_busiest_hour() {
        let hours: Vec<(usize, f32)> = (0..HOURS)
            .map(|hour| (hour, if hour == 7 { 0.9 } else { 0.1 }))
            .collect();
        assert_eq!(busiest_hour(&bucket(145_500_000, 0.2, &hours)), Some(7));
        assert_eq!(busiest_hour(&bucket(145_500_000, 0.0, &[])), None);
    }

    #[test]
    fn formats_buckets_duty_and_hours() {
        assert_eq!(bucket_hz(145_506_300), "145.5063 MHz");
        assert_eq!(bucket_hz(145_500_000), "145.5 MHz");
        assert_eq!(duty_text(0.123), "12%");
        assert_eq!(duty_text(1.0), "100%");
        assert_eq!(duty_text(0.0), "-");
        assert_eq!(duty_text(f32::NAN), "-");
        assert_eq!(hour_text(0), "00:00");
        assert_eq!(hour_text(7), "07:00");
        assert_eq!(hour_text(23), "23:00");
    }
}
