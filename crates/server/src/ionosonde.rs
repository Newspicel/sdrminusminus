use std::time::Duration;

use reqwest::Client;
use sdrmm_wire::{
    IONOSONDE_MAX_STATIONS, IONOSONDE_SOURCE, IONOSONDE_URL, IonosondeReport, IonosondeStation,
};
use serde::Deserialize;
use tokio::sync::Mutex;

const FEED_URL: &str = "https://prop.kc2g.com/api/stations.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const FRESH_FOR: Duration = Duration::from_secs(15 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_READING_AGE: jiff::SignedDuration = jiff::SignedDuration::from_hours(6);
const MIN_MUF_MHZ: f64 = 1.0;
const MAX_MUF_MHZ: f64 = 100.0;
const USER_AGENT: &str = concat!(
    "sdr--/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    "propagation map)"
);

#[derive(Debug, Deserialize)]
struct FeedEntry {
    station: FeedStation,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    mufd: Option<f64>,
    #[serde(default)]
    fof2: Option<f64>,
    #[serde(default)]
    md: Option<String>,
    #[serde(default)]
    cs: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FeedStation {
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
    latitude: String,
    longitude: String,
}

#[derive(Default)]
pub(crate) struct Ionosonde {
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    report: Option<IonosondeReport>,
    fetched: Option<std::time::Instant>,
    failed: Option<std::time::Instant>,
}

impl Cache {
    fn is_fresh(&self, now: std::time::Instant) -> bool {
        match (&self.report, self.fetched) {
            (Some(report), Some(at)) => {
                report.error.is_none() && now.duration_since(at) < FRESH_FOR
            }
            _ => false,
        }
    }

    fn is_backing_off(&self, now: std::time::Instant) -> bool {
        self.failed
            .is_some_and(|at| now.duration_since(at) < RETRY_AFTER)
    }
}

impl Ionosonde {
    pub(crate) async fn report(&self) -> IonosondeReport {
        self.report_from(FEED_URL, jiff::Timestamp::now()).await
    }

    async fn report_from(&self, url: &str, now_utc: jiff::Timestamp) -> IonosondeReport {
        let mut cache = self.cache.lock().await;
        let now = std::time::Instant::now();
        if cache.is_fresh(now)
            && let Some(report) = &cache.report
        {
            return report.clone();
        }
        if cache.is_backing_off(now)
            && let Some(report) = &cache.report
        {
            return report.clone();
        }
        match fetch(url, now_utc).await {
            Ok(report) => {
                cache.fetched = Some(now);
                cache.failed = None;
                cache.report = Some(report.clone());
                report
            }
            Err(error) => {
                cache.failed = Some(now);
                let stale = cache.report.clone();
                let report = match stale {
                    Some(mut report) => {
                        report.error = Some(format!("{error} — showing the last good fetch"));
                        report
                    }
                    None => IonosondeReport::empty(Some(error)),
                };
                cache.report = Some(report.clone());
                report
            }
        }
    }
}

async fn fetch(url: &str, now_utc: jiff::Timestamp) -> Result<IonosondeReport, String> {
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| format!("could not build the ionosonde HTTP client: {error}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("could not reach the ionosonde feed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("the ionosonde feed answered {status}"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("could not read the ionosonde feed: {error}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "the ionosonde feed returned {} bytes, over the {MAX_BODY_BYTES} byte budget",
            body.len()
        ));
    }
    let entries: Vec<FeedEntry> = serde_json::from_slice(&body)
        .map_err(|error| format!("could not read the ionosonde feed as JSON: {error}"))?;
    Ok(normalize(entries, now_utc))
}

fn normalize(entries: Vec<FeedEntry>, now_utc: jiff::Timestamp) -> IonosondeReport {
    let mut stations: Vec<IonosondeStation> = entries
        .into_iter()
        .filter_map(|entry| station(entry, now_utc))
        .collect();
    stations.sort_unstable_by(|a, b| a.code.cmp(&b.code));
    stations.dedup_by(|a, b| a.code == b.code);
    stations.truncate(IONOSONDE_MAX_STATIONS);
    IonosondeReport {
        source: IONOSONDE_SOURCE.to_owned(),
        url: IONOSONDE_URL.to_owned(),
        fetched_at: Some(now_utc.to_string()),
        stations,
        error: None,
    }
}

fn station(entry: FeedEntry, now_utc: jiff::Timestamp) -> Option<IonosondeStation> {
    let muf3000_mhz = entry
        .mufd
        .filter(|muf| muf.is_finite() && (MIN_MUF_MHZ..=MAX_MUF_MHZ).contains(muf))?;
    let measured = measured_at(entry.time.as_deref())?;
    if now_utc.duration_since(measured) > MAX_READING_AGE {
        return None;
    }
    let latitude = entry.station.latitude.trim().parse::<f64>().ok()?;
    let longitude = wrap_longitude(entry.station.longitude.trim().parse::<f64>().ok()?);
    if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
        return None;
    }
    Some(IonosondeStation {
        code: entry.station.code,
        name: entry.station.name,
        latitude,
        longitude,
        muf3000_mhz,
        fof2_mhz: entry.fof2.filter(|value| value.is_finite() && *value > 0.0),
        m3000: entry
            .md
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0),
        confidence: entry
            .cs
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value)),
        measured_at: measured.to_string(),
    })
}

fn measured_at(time: Option<&str>) -> Option<jiff::Timestamp> {
    let time = time?.trim();
    if let Ok(stamp) = time.parse::<jiff::Timestamp>() {
        return Some(stamp);
    }
    time.parse::<jiff::civil::DateTime>()
        .ok()?
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp())
}

fn wrap_longitude(longitude: f64) -> f64 {
    if !longitude.is_finite() {
        return f64::NAN;
    }
    let wrapped = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped == -180.0 { 180.0 } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"[
        {
          "station": {"code": "AU930", "id": 1, "name": "Austin, TX, USA",
                      "latitude": "30.4", "longitude": "262.3"},
          "time": "2026-08-16T18:10:05", "mufd": 28.827, "fof2": 8.6,
          "md": "3.352", "cs": 100.0
        },
        {
          "station": {"code": "EA036", "id": 2, "name": "El Arenosillo, Spain",
                      "latitude": "37.1", "longitude": "353.3"},
          "time": "2026-08-16T18:20:01", "mufd": 24.882, "fof2": 7.5,
          "md": "3.318", "cs": 70.0
        },
        {
          "station": {"code": "ST000", "id": 3, "name": "Stale, Nowhere",
                      "latitude": "10.0", "longitude": "10.0"},
          "time": "2026-03-19T22:10:05", "mufd": 28.0, "cs": 100.0
        },
        {
          "station": {"code": "NM000", "id": 4, "name": "No reading",
                      "latitude": "10.0", "longitude": "20.0"},
          "time": "2026-08-16T18:20:01", "mufd": null, "cs": 100.0
        },
        {
          "station": {"code": "UC000", "id": 5, "name": "Unscored, Somewhere",
                      "latitude": "-38.7", "longitude": "297.7"},
          "time": "2026-08-16T18:20:00", "mufd": 25.7, "cs": -1.0
        }
    ]"#;

    fn now() -> jiff::Timestamp {
        "2026-08-16T18:30:00Z".parse().expect("fixed clock")
    }

    fn report() -> IonosondeReport {
        normalize(serde_json::from_str(FEED).expect("feed fixture"), now())
    }

    #[test]
    fn the_feed_becomes_stations_with_signed_longitudes() {
        let report = report();
        let austin = report
            .stations
            .iter()
            .find(|station| station.code == "AU930")
            .expect("Austin survives");
        assert!((austin.longitude - (-97.7)).abs() < 1e-9, "{austin:?}");
        assert!((austin.latitude - 30.4).abs() < 1e-9);
        assert!((austin.muf3000_mhz - 28.827).abs() < 1e-9);
        assert_eq!(austin.m3000, Some(3.352));
        assert_eq!(austin.confidence, Some(100.0));
        assert_eq!(austin.measured_at, "2026-08-16T18:10:05Z");

        let arenosillo = report
            .stations
            .iter()
            .find(|station| station.code == "EA036")
            .expect("El Arenosillo survives");
        assert!(
            (arenosillo.longitude - (-6.7)).abs() < 1e-9,
            "{arenosillo:?}"
        );
    }

    #[test]
    fn a_stale_or_empty_reading_is_left_out() {
        let report = report();
        let codes: Vec<&str> = report
            .stations
            .iter()
            .map(|station| station.code.as_str())
            .collect();
        assert_eq!(codes, ["AU930", "EA036", "UC000"]);
    }

    #[test]
    fn a_normalized_report_names_its_source_and_when_it_was_fetched() {
        let report = report();
        assert_eq!(report.source, IONOSONDE_SOURCE);
        assert_eq!(report.url, IONOSONDE_URL);
        assert_eq!(report.fetched_at.as_deref(), Some("2026-08-16T18:30:00Z"));
        assert_eq!(report.error, None);
    }

    #[test]
    fn an_unscored_reading_carries_no_confidence_rather_than_a_bad_one() {
        let report = report();
        let unscored = report
            .stations
            .iter()
            .find(|station| station.code == "UC000")
            .expect("the unscored site is still a reading");
        assert_eq!(unscored.confidence, None);
        assert_eq!(unscored.m3000, None);
    }

    #[test]
    fn longitudes_wrap_into_the_signed_range() {
        for (input, want) in [
            (0.0, 0.0),
            (179.0, 179.0),
            (180.0, 180.0),
            (181.0, -179.0),
            (262.3, -97.7),
            (359.9, -0.1),
            (-95.0, -95.0),
        ] {
            assert!(
                (wrap_longitude(input) - want).abs() < 1e-9,
                "{input} wrapped to {}",
                wrap_longitude(input)
            );
        }
    }

    #[tokio::test]
    async fn an_unreachable_feed_answers_with_the_reason_rather_than_failing() {
        let ionosonde = Ionosonde::default();
        let report = ionosonde
            .report_from("http://127.0.0.1:1/stations.json", now())
            .await;
        assert!(report.stations.is_empty());
        assert!(report.error.is_some(), "the failure was swallowed");
        assert_eq!(report.source, IONOSONDE_SOURCE);
    }

    #[tokio::test]
    async fn a_failure_backs_off_rather_than_hammering_the_feed() {
        let ionosonde = Ionosonde::default();
        let url = "http://127.0.0.1:1/stations.json";
        let first = ionosonde.report_from(url, now()).await;
        let second = ionosonde.report_from(url, now()).await;
        assert_eq!(first.error, second.error);
        let cache = ionosonde.cache.lock().await;
        assert!(cache.is_backing_off(std::time::Instant::now()));
    }
}
