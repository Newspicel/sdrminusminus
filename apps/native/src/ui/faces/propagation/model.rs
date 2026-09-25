use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    decode::{DecodedRecord, DecoderEvent},
    position::PositionFix,
};

use crate::ui::{
    kit_maps::{millis_of, now_ms},
    map::geo::{EARTH_RADIUS_KM, Geo, along_great_circle, bearing_deg, great_circle_km},
};

pub const PROPAGATION_KINDS: [&str; 3] = ["ft8", "ft4", "wspr"];
pub const MIN_MUF_PATH_KM: f64 = 500.0;
pub const OBSERVATION_CAPACITY: usize = 20_000;
pub const DECAYS_KEPT: f64 = 8.0;
pub const HISTORY_WINDOW_MS: i64 = 6 * 60 * 60_000;
const PATHS_DRAWN: usize = 400;

#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub key: String,
    pub at: i64,
    pub kind: &'static str,
    pub callsign: String,
    pub grid: String,
    pub freq_hz: f64,
    pub snr_db: f64,
    pub transmitter: Geo,
    pub distance_km: f64,
    pub bearing_deg: f64,
    pub hops: u32,
    pub muf3000_mhz: Option<f64>,
    pub control: Vec<Geo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub key: String,
    pub centre: Geo,
    pub weight: f64,
    pub decodes: u32,
    pub callsigns: usize,
    pub best_freq_hz: f64,
    pub best_snr_db: f64,
    pub measured_muf3000_mhz: Option<f64>,
    pub median_distance_km: f64,
    pub last_seen: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub key: String,
    pub from: Geo,
    pub to: Geo,
    pub weight: f64,
    pub freq_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub half_life_minutes: f64,
    pub now_ms: i64,
}

fn letter(byte: u8, last: u8) -> Option<u32> {
    let value = byte.to_ascii_uppercase().checked_sub(b'A')?;
    (value <= last).then_some(u32::from(value))
}

fn digit(byte: u8) -> Option<u32> {
    let value = byte.checked_sub(b'0')?;
    (value <= 9).then_some(u32::from(value))
}

#[must_use]
pub fn grid_to_geo(grid: &str) -> Option<Geo> {
    let text = grid.trim().as_bytes();
    if text.len() < 4 || !text.len().is_multiple_of(2) || text.len() > 8 {
        return None;
    }
    let mut lon = f64::from(letter(text[0], 17)?) * 20.0 + f64::from(digit(text[2])?) * 2.0;
    let mut lat = f64::from(letter(text[1], 17)?) * 10.0 + f64::from(digit(text[3])?);
    let (mut lon_size, mut lat_size) = (2.0, 1.0);
    if text.len() >= 6 {
        lon_size /= 24.0;
        lat_size /= 24.0;
        lon += f64::from(letter(text[4], 23)?) * lon_size;
        lat += f64::from(letter(text[5], 23)?) * lat_size;
    }
    if text.len() == 8 {
        lon_size /= 10.0;
        lat_size /= 10.0;
        lon += f64::from(digit(text[6])?) * lon_size;
        lat += f64::from(digit(text[7])?) * lat_size;
    }
    Some(Geo::new(
        lat + lat_size / 2.0 - 90.0,
        lon + lon_size / 2.0 - 180.0,
    ))
}

#[must_use]
pub fn geo_to_grid(at: Geo) -> String {
    let lon = (at.lon + 180.0).clamp(0.0, 359.999_999);
    let lat = (at.lat + 90.0).clamp(0.0, 179.999_999);
    let field = |value: f64, size: f64| char::from(b'A' + (value / size).floor() as u8);
    let square =
        |value: f64, size: f64, step: f64| char::from(b'0' + ((value % size) / step).floor() as u8);
    [
        field(lon, 20.0),
        field(lat, 10.0),
        square(lon, 20.0, 2.0),
        square(lat, 10.0, 1.0),
    ]
    .into_iter()
    .collect()
}

fn is_grid4(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 4
        && (b'A'..=b'R').contains(&bytes[0])
        && (b'A'..=b'R').contains(&bytes[1])
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}

#[must_use]
pub fn message_grid(text: &str) -> Option<String> {
    let upper = text.trim().to_uppercase();
    let last = upper.split_whitespace().last()?;
    (last != "RR73" && is_grid4(last)).then(|| last.to_owned())
}

#[must_use]
pub fn message_callsign(text: &str) -> Option<String> {
    let upper = text.trim().to_uppercase();
    let tokens: Vec<&str> = upper.split_whitespace().collect();
    let before = tokens.len().checked_sub(2).and_then(|at| tokens.get(at))?;
    let trimmed = before.strip_prefix('<').unwrap_or(before);
    let trimmed = trimmed.strip_suffix('>').unwrap_or(trimmed);
    Some(trimmed.to_owned())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub grid: String,
    pub callsign: String,
    pub snr_db: f64,
}

#[must_use]
pub fn event_spot(event: &DecoderEvent) -> Option<Spot> {
    match event {
        DecoderEvent::Ft8(message) | DecoderEvent::Ft4(message) => Some(Spot {
            grid: message_grid(&message.text)?,
            callsign: message_callsign(&message.text).unwrap_or_default(),
            snr_db: f64::from(message.snr_db),
        }),
        DecoderEvent::Wspr(spot) => {
            let grid = spot.grid.clone()?;
            grid_to_geo(&grid)?;
            Some(Spot {
                grid,
                callsign: spot.callsign.clone(),
                snr_db: f64::from(spot.snr_db),
            })
        }
        _ => None,
    }
}

#[must_use]
pub fn is_propagation_kind(kind: &str) -> bool {
    PROPAGATION_KINDS.contains(&kind)
}

#[must_use]
pub fn max_hop_km(height_km: f64) -> f64 {
    let r = EARTH_RADIUS_KM;
    2.0 * r * (r / (r + height_km)).min(1.0).acos()
}

#[must_use]
pub fn hop_count(distance_km: f64, height_km: f64) -> u32 {
    let reach = max_hop_km(height_km);
    if reach <= 0.0 || distance_km <= 0.0 || !reach.is_finite() || !distance_km.is_finite() {
        return 1;
    }
    ((distance_km / reach).ceil() as u32).max(1)
}

#[must_use]
pub fn obliquity_factor(hop_km: f64, height_km: f64) -> f64 {
    let r = EARTH_RADIUS_KM;
    let delta = hop_km / (2.0 * r);
    let denominator = r + height_km - r * delta.cos();
    if denominator <= 0.0 || !denominator.is_finite() {
        return 1.0;
    }
    1.0f64.hypot(r * delta.sin() / denominator)
}

#[must_use]
pub fn muf3000_mhz(freq_hz: f64, distance_km: f64, height_km: f64) -> Option<f64> {
    if !freq_hz.is_finite() || freq_hz <= 0.0 || distance_km < MIN_MUF_PATH_KM {
        return None;
    }
    let hops = hop_count(distance_km, height_km);
    let factor = obliquity_factor(distance_km / f64::from(hops), height_km);
    (factor > 0.0).then(|| freq_hz / 1e6 * obliquity_factor(3_000.0, height_km) / factor)
}

#[must_use]
pub fn control_points(from: Geo, to: Geo, hops: u32) -> Vec<Geo> {
    (0..hops)
        .map(|hop| along_great_circle(from, to, f64::from(2 * hop + 1) / f64::from(2 * hops)))
        .collect()
}

#[must_use]
pub fn observation_of(
    record: &DecodedRecord,
    receiver: Geo,
    height_km: f64,
) -> Option<Observation> {
    let kind = record.event.kind();
    if !is_propagation_kind(kind) {
        return None;
    }
    let spot = event_spot(&record.event)?;
    let transmitter = grid_to_geo(&spot.grid)?;
    let distance_km = great_circle_km(receiver, transmitter);
    let hops = hop_count(distance_km, height_km);
    Some(Observation {
        key: format!(
            "{}|{}:{}|{}|{}",
            record.at, record.device_set, record.channel, spot.callsign, spot.grid
        ),
        at: millis_of(&record.at).unwrap_or_else(now_ms),
        kind,
        callsign: spot.callsign,
        grid: spot.grid,
        freq_hz: record.freq_hz,
        snr_db: spot.snr_db,
        transmitter,
        distance_km,
        bearing_deg: bearing_deg(receiver, transmitter),
        hops,
        muf3000_mhz: muf3000_mhz(record.freq_hz, distance_km, height_km),
        control: control_points(receiver, transmitter, hops),
    })
}

#[must_use]
pub fn decay_weight(age_ms: i64, half_life_minutes: f64) -> f64 {
    let half_life_ms = half_life_minutes.max(1.0) * 60_000.0;
    if age_ms <= 0 {
        return 1.0;
    }
    (-(age_ms as f64) / half_life_ms).exp2()
}

struct Gathering {
    cell: Cell,
    calls: HashSet<String>,
    distances: Vec<f64>,
}

#[must_use]
pub fn cells(observations: &[Observation], options: Options) -> Vec<Cell> {
    let mut gathered: HashMap<String, Gathering> = HashMap::new();
    for observation in observations {
        let weight = decay_weight(options.now_ms - observation.at, options.half_life_minutes);
        if weight <= 0.0 {
            continue;
        }
        for point in &observation.control {
            let key = geo_to_grid(*point);
            let Some(centre) = grid_to_geo(&key) else {
                continue;
            };
            let held = gathered.entry(key.clone()).or_insert_with(|| Gathering {
                cell: Cell {
                    key,
                    centre,
                    weight: 0.0,
                    decodes: 0,
                    callsigns: 0,
                    best_freq_hz: 0.0,
                    best_snr_db: f64::NEG_INFINITY,
                    measured_muf3000_mhz: None,
                    median_distance_km: 0.0,
                    last_seen: 0,
                },
                calls: HashSet::new(),
                distances: Vec::new(),
            });
            let cell = &mut held.cell;
            cell.weight += weight;
            cell.decodes += 1;
            held.calls.insert(observation.callsign.clone());
            held.distances.push(observation.distance_km);
            cell.best_freq_hz = cell.best_freq_hz.max(observation.freq_hz);
            cell.best_snr_db = cell.best_snr_db.max(observation.snr_db);
            cell.last_seen = cell.last_seen.max(observation.at);
            if let Some(muf) = observation.muf3000_mhz {
                cell.measured_muf3000_mhz =
                    Some(cell.measured_muf3000_mhz.map_or(muf, |held| held.max(muf)));
            }
        }
    }
    let mut out: Vec<Cell> = gathered
        .into_values()
        .map(|held| {
            let mut cell = held.cell;
            cell.callsigns = held.calls.len();
            if cell.best_snr_db == f64::NEG_INFINITY {
                cell.best_snr_db = 0.0;
            }
            cell.median_distance_km = median(held.distances);
            cell
        })
        .collect();
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

#[must_use]
pub fn paths(observations: &[Observation], receiver: Geo, options: Options) -> Vec<Path> {
    let mut newest: HashMap<String, &Observation> = HashMap::new();
    for observation in observations {
        let key = format!(
            "{}|{}",
            observation.grid,
            (observation.freq_hz / 1e5).round()
        );
        match newest.get(&key) {
            Some(held) if held.at >= observation.at => {}
            _ => {
                newest.insert(key, observation);
            }
        }
    }
    let mut chosen: Vec<&Observation> = newest.into_values().collect();
    chosen.sort_by_key(|observation| std::cmp::Reverse(observation.at));
    chosen
        .into_iter()
        .take(PATHS_DRAWN)
        .map(|observation| Path {
            key: observation.key.clone(),
            from: receiver,
            to: observation.transmitter,
            weight: decay_weight(options.now_ms - observation.at, options.half_life_minutes),
            freq_hz: observation.freq_hz,
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub decodes: usize,
    pub grids: usize,
    pub callsigns: usize,
    pub bands: usize,
    pub best_freq_hz: f64,
    pub best_muf3000_mhz: Option<f64>,
    pub farthest_km: f64,
}

#[must_use]
pub fn summary(observations: &[Observation]) -> Summary {
    let mut grids = HashSet::new();
    let mut callsigns = HashSet::new();
    let mut bands = HashSet::new();
    let mut out = Summary {
        decodes: observations.len(),
        ..Summary::default()
    };
    for observation in observations {
        grids.insert(observation.grid.as_str());
        callsigns.insert(observation.callsign.as_str());
        bands.insert((observation.freq_hz / 1e5).round() as i64);
        out.best_freq_hz = out.best_freq_hz.max(observation.freq_hz);
        out.farthest_km = out.farthest_km.max(observation.distance_km);
        if let Some(muf) = observation.muf3000_mhz {
            out.best_muf3000_mhz = Some(out.best_muf3000_mhz.map_or(muf, |held| held.max(muf)));
        }
    }
    out.grids = grids.len();
    out.callsigns = callsigns.len();
    out.bands = bands.len();
    out
}

#[must_use]
pub fn receiver_of(fix: Option<&PositionFix>) -> Option<Geo> {
    let fix = fix?;
    (fix.latitude.is_finite() && fix.longitude.is_finite())
        .then(|| Geo::new(fix.latitude, fix.longitude))
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

#[must_use]
pub fn merge(
    held: &[Observation],
    added: &[Observation],
    capacity: usize,
) -> Option<Vec<Observation>> {
    let seen: HashSet<&str> = held
        .iter()
        .map(|observation| observation.key.as_str())
        .collect();
    let mut fresh_keys = HashSet::new();
    let fresh: Vec<&Observation> = added
        .iter()
        .filter(|observation| {
            !seen.contains(observation.key.as_str()) && fresh_keys.insert(observation.key.as_str())
        })
        .collect();
    if fresh.is_empty() {
        return None;
    }
    let mut merged: Vec<Observation> = held.iter().chain(fresh).cloned().collect();
    merged.sort_by_key(|observation| observation.at);
    let overflow = merged.len().saturating_sub(capacity);
    merged.drain(..overflow);
    Some(merged)
}

#[must_use]
pub fn live(observations: &[Observation], options: Options, cleared_at: i64) -> Vec<Observation> {
    let decayed = options.now_ms - (options.half_life_minutes * 60_000.0 * DECAYS_KEPT) as i64;
    let cutoff = cleared_at.max(decayed);
    observations
        .iter()
        .filter(|observation| observation.at >= cutoff)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use sdrmm_wire::decode::{MorseText, WsjtMessage, WsprSpot};

    use super::*;
    use crate::ui::kit_maps::iso_of;

    const NOW: i64 = 1_786_881_600_000;
    const BERLIN: Geo = Geo::new(52.5, 13.0);

    fn close(a: f64, b: f64, digits: i32) -> bool {
        (a - b).abs() < 0.5 * 10f64.powi(-digits)
    }

    fn ft8(text: &str) -> DecoderEvent {
        DecoderEvent::Ft8(WsjtMessage {
            text: text.to_owned(),
            snr_db: -12.0,
            audio_hz: 1500.0,
            time_offset_s: 0.0,
            hard_errors: 0,
        })
    }

    fn record(event: DecoderEvent) -> DecodedRecord {
        DecodedRecord {
            origin: None,
            device_set: 0,
            channel: 0,
            at: iso_of(NOW),
            freq_hz: 14_074_000.0,
            event,
            sinks: Vec::new(),
        }
    }

    fn wspr(grid: Option<&str>) -> DecoderEvent {
        DecoderEvent::Wspr(WsprSpot {
            text: "K1ABC FN42 37".to_owned(),
            callsign: "K1ABC".to_owned(),
            grid: grid.map(str::to_owned),
            power_dbm: 37,
            snr_db: -24.0,
            audio_hz: 1500.0,
            time_offset_s: 0.0,
            drift_hz: 0.0,
        })
    }

    pub(crate) fn observation(key: &str) -> Observation {
        Observation {
            key: key.to_owned(),
            at: NOW,
            kind: "ft8",
            callsign: "W1AW".to_owned(),
            grid: "FN42".to_owned(),
            freq_hz: 14_074_000.0,
            snr_db: -12.0,
            transmitter: Geo::new(42.5, -71.0),
            distance_km: 6_000.0,
            bearing_deg: 290.0,
            hops: 2,
            muf3000_mhz: Some(16.0),
            control: vec![Geo::new(50.0, -20.0)],
        }
    }

    fn options() -> Options {
        Options {
            half_life_minutes: 30.0,
            now_ms: NOW,
        }
    }

    #[test]
    fn a_four_character_square_sits_at_its_own_centre() {
        assert_eq!(grid_to_geo("FN42"), Some(Geo::new(42.5, -71.0)));
        assert_eq!(grid_to_geo("JO62"), Some(Geo::new(52.5, 13.0)));
        assert_eq!(grid_to_geo("IO91"), Some(Geo::new(51.5, -1.0)));
    }

    #[test]
    fn lower_case_and_finer_subsquares_are_read() {
        let coarse = grid_to_geo("JO62").expect("coarse");
        let fine = grid_to_geo("jo62qm").expect("fine");
        assert!((fine.lat - coarse.lat).abs() < 0.5);
        assert!((fine.lon - coarse.lon).abs() < 1.0);
        assert!(grid_to_geo("JO62QM12").is_some());
    }

    #[test]
    fn anything_that_is_not_a_locator_is_refused() {
        for bad in [
            "", "FN4", "FN421", "ZZ99", "F142", "FN4X", "JO62yy", "JO62QM1",
        ] {
            assert!(grid_to_geo(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_square_round_trips_through_its_centre() {
        for grid in ["FN42", "JO62", "IO91", "PM95", "AA00", "RR99"] {
            let centre = grid_to_geo(grid).expect("a centre");
            assert_eq!(geo_to_grid(centre), grid);
        }
    }

    #[test]
    fn the_grid_a_cq_or_a_reply_carries_is_taken() {
        assert_eq!(message_grid("CQ W1AW FN42").as_deref(), Some("FN42"));
        assert_eq!(message_grid("CQ DX JA1ABC PM95").as_deref(), Some("PM95"));
        assert_eq!(message_grid("W9XYZ W1AW FN42").as_deref(), Some("FN42"));
        assert_eq!(message_grid("cq w1aw fn42").as_deref(), Some("FN42"));
    }

    #[test]
    fn rr73_and_signal_reports_are_not_squares() {
        for text in [
            "W9XYZ W1AW RR73",
            "W9XYZ W1AW RRR",
            "W9XYZ W1AW 73",
            "W9XYZ W1AW -12",
            "W9XYZ W1AW R-12",
            "W9XYZ W1AW R+05",
            "TU; W9XYZ W1AW R 579 MA",
        ] {
            assert!(message_grid(text).is_none(), "{text}");
        }
    }

    #[test]
    fn the_station_the_square_belongs_to_is_named() {
        assert_eq!(message_callsign("CQ W1AW FN42").as_deref(), Some("W1AW"));
        assert_eq!(message_callsign("W9XYZ W1AW FN42").as_deref(), Some("W1AW"));
        assert_eq!(message_callsign("CQ <W1AW> FN42").as_deref(), Some("W1AW"));
    }

    #[test]
    fn a_wspr_spot_brings_its_own_grid() {
        assert_eq!(
            event_spot(&wspr(Some("FN42"))),
            Some(Spot {
                grid: "FN42".to_owned(),
                callsign: "K1ABC".to_owned(),
                snr_db: -24.0
            })
        );
        assert!(event_spot(&wspr(None)).is_none());
        assert!(event_spot(&ft8("W9XYZ W1AW 73")).is_none());
    }

    #[test]
    fn the_textbook_m3000_for_the_f2_layer_is_reproduced() {
        assert!(close(obliquity_factor(3_000.0, 300.0), 3.2798, 3));
        assert!(close(obliquity_factor(1.0, 300.0), 1.0, 4));
    }

    #[test]
    fn a_path_longer_than_one_hop_is_split() {
        assert!((3_800.0..3_900.0).contains(&max_hop_km(300.0)));
        assert_eq!(hop_count(2_000.0, 300.0), 1);
        assert_eq!(hop_count(8_000.0, 300.0), 3);
        assert_eq!(hop_count(0.0, 300.0), 1);
    }

    #[test]
    fn a_3000_km_decode_stays_at_its_own_frequency() {
        let muf = muf3000_mhz(14_074_000.0, 3_000.0, 300.0).expect("a muf");
        assert!(close(muf, 14.074, 6));
        let short = muf3000_mhz(14_074_000.0, 1_000.0, 300.0).expect("short");
        let long = muf3000_mhz(14_074_000.0, 8_000.0, 300.0).expect("long");
        assert!((24.0..26.0).contains(&short));
        assert!(long > 14.074 && long < 15.5);
    }

    #[test]
    fn a_path_too_short_for_the_ionosphere_is_not_inverted() {
        assert!(muf3000_mhz(14_074_000.0, MIN_MUF_PATH_KM - 1.0, 300.0).is_none());
        assert!(muf3000_mhz(0.0, 3_000.0, 300.0).is_none());
        assert!(muf3000_mhz(f64::NAN, 3_000.0, 300.0).is_none());
    }

    #[test]
    fn control_points_sit_at_the_hop_midpoints() {
        let single = control_points(Geo::new(0.0, 0.0), Geo::new(0.0, 60.0), 1);
        assert_eq!(single.len(), 1);
        assert!(close(single[0].lon, 30.0, 6));
        let triple = control_points(Geo::new(0.0, 0.0), Geo::new(0.0, 60.0), 3);
        assert_eq!(triple.len(), 3);
        assert!(close(triple[0].lon, 10.0, 6));
        assert!(close(triple[2].lon, 50.0, 6));
    }

    #[test]
    fn a_decode_becomes_a_path_to_the_transmitters_square() {
        let built = observation_of(&record(ft8("CQ W1AW FN42")), BERLIN, 300.0).expect("a path");
        assert_eq!(built.grid, "FN42");
        assert_eq!(built.callsign, "W1AW");
        assert!((5_800.0..6_100.0).contains(&built.distance_km));
        assert_eq!(built.hops, 2);
        assert_eq!(built.control.len(), 2);
        assert!(built.muf3000_mhz.is_some());
        assert_eq!(built.at, NOW);
    }

    #[test]
    fn a_decode_without_a_square_or_of_another_kind_is_ignored() {
        assert!(observation_of(&record(ft8("W9XYZ W1AW RR73")), BERLIN, 300.0).is_none());
        let morse = DecoderEvent::Morse(MorseText {
            text: "CQ".to_owned(),
            wpm: 18.0,
        });
        assert!(observation_of(&record(morse), BERLIN, 300.0).is_none());
    }

    #[test]
    fn a_weight_halves_over_one_half_life() {
        assert_eq!(decay_weight(0, 30.0), 1.0);
        assert!(close(decay_weight(30 * 60_000, 30.0), 0.5, 9));
        assert!(close(decay_weight(60 * 60_000, 30.0), 0.25, 9));
    }

    #[test]
    fn reflection_points_gather_in_their_square_and_keep_the_highest_band() {
        let gathered = cells(
            &[
                observation("a"),
                Observation {
                    control: vec![Geo::new(50.4, -19.5)],
                    freq_hz: 21_074_000.0,
                    muf3000_mhz: Some(24.0),
                    callsign: "K1ABC".to_owned(),
                    ..observation("b")
                },
                Observation {
                    control: vec![Geo::new(10.0, 10.0)],
                    freq_hz: 7_074_000.0,
                    muf3000_mhz: Some(9.0),
                    ..observation("c")
                },
            ],
            options(),
        );
        assert_eq!(gathered.len(), 2);
        let busiest = &gathered[0];
        assert_eq!(busiest.decodes, 2);
        assert_eq!(busiest.callsigns, 2);
        assert_eq!(busiest.best_freq_hz, 21_074_000.0);
        assert_eq!(busiest.measured_muf3000_mhz, Some(24.0));
        assert!(close(busiest.weight, 2.0, 6));
    }

    #[test]
    fn an_old_decode_weighs_less_than_a_fresh_one() {
        let gathered = cells(
            &[
                Observation {
                    at: NOW - 60 * 60_000,
                    ..observation("old")
                },
                Observation {
                    control: vec![Geo::new(10.0, 10.0)],
                    ..observation("new")
                },
            ],
            options(),
        );
        assert!(close(gathered[0].weight, 1.0, 6));
        assert!(close(gathered[1].weight, 0.25, 6));
        let unmeasured = cells(
            &[Observation {
                muf3000_mhz: None,
                ..observation("a")
            }],
            options(),
        );
        assert_eq!(unmeasured[0].measured_muf3000_mhz, None);
    }

    #[test]
    fn one_line_per_station_and_band_newest_first() {
        let drawn = paths(
            &[
                Observation {
                    at: NOW - 1000,
                    ..observation("old")
                },
                observation("new"),
                Observation {
                    grid: "JO62".to_owned(),
                    at: NOW - 500,
                    ..observation("other")
                },
            ],
            BERLIN,
            options(),
        );
        assert_eq!(drawn.len(), 2);
        assert_eq!(drawn[0].key, "new");
        assert_eq!(drawn[0].from, BERLIN);
    }

    #[test]
    fn the_summary_counts_what_was_heard() {
        let counted = summary(&[
            observation("a"),
            Observation {
                grid: "JO62".to_owned(),
                callsign: "DL1ABC".to_owned(),
                freq_hz: 28_074_000.0,
                muf3000_mhz: Some(31.0),
                distance_km: 400.0,
                ..observation("b")
            },
        ]);
        assert_eq!(counted.decodes, 2);
        assert_eq!(counted.grids, 2);
        assert_eq!(counted.callsigns, 2);
        assert_eq!(counted.bands, 2);
        assert_eq!(counted.best_freq_hz, 28_074_000.0);
        assert_eq!(counted.best_muf3000_mhz, Some(31.0));
        assert_eq!(counted.farthest_km, 6_000.0);
    }

    #[test]
    fn merging_keeps_one_of_each_oldest_first_and_drops_the_overflow() {
        let held = merge(
            &[],
            &[Observation {
                at: NOW - 10,
                ..observation("a")
            }],
            10,
        )
        .expect("merged");
        assert!(
            merge(
                &held,
                &[Observation {
                    at: NOW - 10,
                    ..observation("a")
                }],
                10
            )
            .is_none()
        );
        let merged = merge(
            &held,
            &[Observation {
                at: NOW - 20,
                ..observation("b")
            }],
            10,
        )
        .expect("merged");
        let keys: Vec<&str> = merged.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["b", "a"]);
        let capped = merge(
            &[],
            &[
                Observation {
                    at: NOW - 2,
                    ..observation("x")
                },
                Observation {
                    at: NOW - 1,
                    ..observation("y")
                },
                observation("z"),
            ],
            2,
        )
        .expect("merged");
        let keys: Vec<&str> = capped.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["y", "z"]);
    }

    #[test]
    fn a_clear_holds_and_decayed_history_is_forgotten() {
        let kept = live(
            &[
                Observation {
                    at: NOW - 120_000,
                    ..observation("before")
                },
                Observation {
                    at: NOW - 30_000,
                    ..observation("after")
                },
            ],
            options(),
            NOW - 60_000,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].key, "after");
        let kept = live(
            &[
                Observation {
                    at: NOW - 60_000,
                    ..observation("recent")
                },
                Observation {
                    at: NOW - 24 * 3_600_000,
                    ..observation("ancient")
                },
            ],
            options(),
            0,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].key, "recent");
    }

    #[test]
    fn the_receiver_is_a_fix_that_is_a_position() {
        let fix = PositionFix {
            latitude: 52.5,
            longitude: 13.0,
            altitude_m: None,
            accuracy_m: None,
            speed_mps: None,
            track_deg: None,
            time: "2026-08-16T12:00:00Z".to_owned(),
        };
        assert_eq!(receiver_of(Some(&fix)), Some(BERLIN));
        assert_eq!(receiver_of(None), None);
        let bad = PositionFix {
            latitude: f64::NAN,
            ..fix
        };
        assert_eq!(receiver_of(Some(&bad)), None);
        assert!(close(
            great_circle_km(Geo::new(0.0, 0.0), Geo::new(0.0, 180.0)),
            PI * EARTH_RADIUS_KM,
            3
        ));
    }
}
