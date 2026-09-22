use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use reqwest::Client;
use sdrmm_orbit::Tle;
use sdrmm_wire::{
    CatalogSatellite, MAX_CATALOG_RESULTS, MAX_SATELLITE_QUERY_LEN, MAX_TRANSMITTER_ID_LEN,
    SATELLITE_CATALOG_SOURCE, SATELLITE_CATALOG_URL, SatelliteCatalogResponse, TRANSMITTER_SOURCE,
    TRANSMITTER_URL, Transmitter, TransmittersResponse,
};
use serde::Deserialize;
use tokio::sync::Mutex;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const FRESH_FOR: Duration = Duration::from_secs(2 * 3_600);
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHED: usize = 64;
const AMATEUR_GROUP: &str = "amateur";
const USER_AGENT: &str = concat!(
    "SDR--/",
    env!("CARGO_PKG_VERSION"),
    " (+satellite tracking)"
);

#[derive(Default)]
pub(crate) struct Catalog {
    searches: Mutex<HashMap<String, (Instant, SatelliteCatalogResponse)>>,
    transmitters: Mutex<HashMap<String, (Instant, TransmittersResponse)>>,
}

#[derive(Debug, Deserialize)]
struct SatnogsTransmitter {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    alive: bool,
    #[serde(default)]
    status: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    downlink_low: Option<f64>,
    #[serde(default)]
    uplink_low: Option<f64>,
}

impl Catalog {
    pub(crate) async fn search(&self, query: &str) -> Result<SatelliteCatalogResponse, String> {
        let query = checked_query(query)?;
        if let Some(fresh) = cached(&self.searches, &query).await {
            return Ok(fresh);
        }
        let params: [(&str, &str); 2] = match query.as_str() {
            "" => [("GROUP", AMATEUR_GROUP), ("FORMAT", "TLE")],
            digits if digits.bytes().all(|b| b.is_ascii_digit()) => {
                [("CATNR", digits), ("FORMAT", "TLE")]
            }
            name => [("NAME", name), ("FORMAT", "TLE")],
        };
        let body = fetch(SATELLITE_CATALOG_URL, &params).await?;
        let response = SatelliteCatalogResponse {
            satellites: element_sets(&String::from_utf8_lossy(&body)),
            source: SATELLITE_CATALOG_SOURCE.to_owned(),
        };
        remember(&self.searches, query, response.clone()).await;
        Ok(response)
    }

    pub(crate) async fn transmitters(&self, catalog: &str) -> Result<TransmittersResponse, String> {
        if catalog.is_empty() || catalog.len() > 9 || !catalog.bytes().all(|b| b.is_ascii_digit()) {
            return Err("a catalog number is digits only".to_owned());
        }
        let key = catalog.trim_start_matches('0').to_owned();
        if let Some(fresh) = cached(&self.transmitters, &key).await {
            return Ok(fresh);
        }
        let body = fetch(
            TRANSMITTER_URL,
            &[
                ("satellite__norad_cat_id", key.as_str()),
                ("format", "json"),
            ],
        )
        .await?;
        let listed: Vec<SatnogsTransmitter> = serde_json::from_slice(&body)
            .map_err(|error| format!("could not read the transmitter list: {error}"))?;
        let response = TransmittersResponse {
            transmitters: transmitters(listed),
            source: TRANSMITTER_SOURCE.to_owned(),
        };
        remember(&self.transmitters, key, response.clone()).await;
        Ok(response)
    }
}

fn checked_query(query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.len() > MAX_SATELLITE_QUERY_LEN {
        return Err(format!(
            "a search is at most {MAX_SATELLITE_QUERY_LEN} characters"
        ));
    }
    if !query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " -()/._+".contains(c))
    {
        return Err("a search holds letters, digits and - ( ) / . _ + only".to_owned());
    }
    Ok(query.to_uppercase())
}

async fn cached<T: Clone>(cache: &Mutex<HashMap<String, (Instant, T)>>, key: &str) -> Option<T> {
    cache
        .lock()
        .await
        .get(key)
        .filter(|(at, _)| at.elapsed() < FRESH_FOR)
        .map(|(_, value)| value.clone())
}

async fn remember<T>(cache: &Mutex<HashMap<String, (Instant, T)>>, key: String, value: T) {
    let mut cache = cache.lock().await;
    cache.retain(|_, (at, _)| at.elapsed() < FRESH_FOR);
    if cache.len() >= MAX_CACHED {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), value));
}

async fn fetch(url: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| format!("could not build the HTTP client: {error}"))?;
    let address = reqwest::Url::parse_with_params(url, params)
        .map_err(|error| format!("could not build a request to {url}: {error}"))?;
    let response = client
        .get(address)
        .send()
        .await
        .map_err(|error| format!("could not reach {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("could not read {url}: {error}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!("{url} sent more than {MAX_BODY_BYTES} bytes"));
    }
    Ok(body.to_vec())
}

fn element_sets(text: &str) -> Vec<CatalogSatellite> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|set| {
            let text = set.join("\n");
            let tle = Tle::parse(&text).ok()?;
            Some(CatalogSatellite {
                name: tle.label().to_owned(),
                catalog: tle.catalog,
                tle: text,
            })
        })
        .take(MAX_CATALOG_RESULTS)
        .collect()
}

fn transmitters(listed: Vec<SatnogsTransmitter>) -> Vec<Transmitter> {
    let frequency = |hz: Option<f64>| hz.filter(|hz| hz.is_finite() && *hz > 0.0);
    let mut out: Vec<Transmitter> = listed
        .into_iter()
        .filter(|entry| entry.status != "invalid")
        .filter(|entry| !entry.uuid.is_empty() && entry.uuid.len() <= MAX_TRANSMITTER_ID_LEN)
        .filter(|entry| frequency(entry.downlink_low).is_some())
        .map(|entry| Transmitter {
            id: entry.uuid,
            description: entry.description,
            mode: entry.mode.filter(|mode| !mode.is_empty()),
            downlink_hz: frequency(entry.downlink_low),
            uplink_hz: frequency(entry.uplink_low),
            alive: entry.alive && entry.status == "active",
        })
        .collect();
    out.sort_by_key(|transmitter| !transmitter.alive);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = "ISS (ZARYA)
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041
BROKEN
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9990
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041
";

    #[test]
    fn a_feed_keeps_only_sets_that_check_out() {
        let sets = element_sets(FEED);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "ISS (ZARYA)");
        assert_eq!(sets[0].catalog, "25544");
        assert!(Tle::parse(&sets[0].tle).is_ok());
    }

    #[test]
    fn nothing_found_is_an_empty_list() {
        assert!(element_sets("No GP data found\n").is_empty());
    }

    #[test]
    fn searches_refuse_what_could_not_be_a_name() {
        assert!(checked_query("ISS").is_ok());
        assert!(checked_query("AO-91 (FOX-1B)").is_ok());
        assert!(checked_query("a&b=c").is_err());
        assert!(checked_query(&"X".repeat(MAX_SATELLITE_QUERY_LEN + 1)).is_err());
    }

    #[test]
    fn live_transmitters_come_first_and_invalid_ones_are_dropped() {
        let listed: Vec<SatnogsTransmitter> = serde_json::from_str(
            r#"[
                {"uuid":"a","description":"Old beacon","alive":false,"status":"inactive","downlink_low":145800000},
                {"uuid":"b","description":"Bad","alive":true,"status":"invalid","downlink_low":1},
                {"uuid":"c","description":"FM voice","alive":true,"status":"active","mode":"FM","downlink_low":437800000,"uplink_low":145990000},
                {"uuid":"d","description":"No downlink","alive":true,"status":"active","downlink_low":null}
            ]"#,
        )
        .expect("parses");
        let kept = transmitters(listed);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].id, "c");
        assert_eq!(kept[0].description, "FM voice");
        assert_eq!(kept[0].uplink_hz, Some(145_990_000.0));
        assert!(!kept[1].alive);
    }
}
