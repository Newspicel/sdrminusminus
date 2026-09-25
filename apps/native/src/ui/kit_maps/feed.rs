use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use sdrmm_wire::{
    coherent::{DfFusionState, RadarDetection},
    decode::{DecodedRecord, DecoderEvent},
    position::PositionFix,
    satellite::SatelliteStatus,
    ws::ServerEvent,
};
use zgui::prelude::*;
use zgui::reactive::ArcRwSignal;

use super::{millis_of, now_ms};
use crate::{store::Store, ui::map::Geo};

pub const POSITION_HISTORY: usize = 5_000;
pub const STATION_CAPACITY: usize = 1_000;
pub const BEARING_HISTORY: usize = 64;
pub const MAP_KINDS: [&str; 3] = ["adsb", "ais", "aprs"];

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at: Geo,
    pub altitude_m: Option<f64>,
    pub accuracy_m: Option<f64>,
    pub received_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub fix: Option<PositionFix>,
    pub error: Option<String>,
    pub history: Vec<Sample>,
}

pub fn append(history: &mut Vec<Sample>, sample: Sample) {
    if let Some(last) = history.last_mut()
        && last.at == sample.at
        && last.altitude_m == sample.altitude_m
    {
        *last = sample;
        return;
    }
    history.push(sample);
    let overflow = history.len().saturating_sub(POSITION_HISTORY);
    history.drain(..overflow);
}

#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub kind: &'static str,
    pub id: String,
    pub event: DecoderEvent,
    pub last_seen: i64,
    pub freq_hz: f64,
    pub frames: u32,
}

#[must_use]
pub fn station_id(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Adsb(message) => Some(message.icao.clone()),
        DecoderEvent::Ais(message) => Some(message.mmsi.to_string()),
        DecoderEvent::Aprs(packet) => Some(packet.source.clone()),
        _ => None,
    }
}

#[must_use]
pub fn merge_forward(previous: &DecoderEvent, next: &DecoderEvent) -> DecoderEvent {
    let (Ok(mut held), Ok(fresh)) = (serde_json::to_value(previous), serde_json::to_value(next))
    else {
        return next.clone();
    };
    if previous.kind() != next.kind() {
        return next.clone();
    }
    if let (Some(held_data), Some(fresh_data)) = (
        held.get_mut("data")
            .and_then(serde_json::Value::as_object_mut),
        fresh.get("data").and_then(serde_json::Value::as_object),
    ) {
        for (key, value) in fresh_data {
            if !value.is_null() {
                held_data.insert(key.clone(), value.clone());
            }
        }
    }
    serde_json::from_value(held).unwrap_or_else(|_| next.clone())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bearing {
    pub deg: f64,
    pub confidence: f64,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finder {
    pub history: Vec<Bearing>,
    pub fusion: Option<DfFusionState>,
    pub detections: Vec<RadarDetection>,
}

#[derive(Default)]
pub struct Held {
    pub tracks: HashMap<String, Track>,
    pub stations: HashMap<&'static str, HashMap<String, Station>>,
    pub finders: HashMap<String, Finder>,
    pub satellites: HashMap<String, SatelliteStatus>,
}

impl Held {
    pub fn position(
        &mut self,
        node: &str,
        fix: Option<PositionFix>,
        error: Option<String>,
        now: i64,
    ) {
        let track = self.tracks.entry(node.to_owned()).or_default();
        if let Some(fix) = &fix {
            append(
                &mut track.history,
                Sample {
                    at: Geo::new(fix.latitude, fix.longitude),
                    altitude_m: fix.altitude_m,
                    accuracy_m: fix.accuracy_m,
                    received_at: now,
                },
            );
        }
        track.fix = fix;
        track.error = error;
    }

    pub fn decoded(&mut self, record: &DecodedRecord) -> bool {
        let Some(id) = station_id(&record.event) else {
            return false;
        };
        let kind = record.event.kind();
        let stations = self.stations.entry(kind).or_default();
        let previous = stations.get(&id);
        let station = Station {
            kind,
            id: id.clone(),
            event: previous.map_or_else(
                || record.event.clone(),
                |held| merge_forward(&held.event, &record.event),
            ),
            last_seen: millis_of(&record.at).unwrap_or_else(now_ms),
            freq_hz: record.freq_hz,
            frames: previous.map_or(0, |held| held.frames) + 1,
        };
        stations.insert(id, station);
        if stations.len() > STATION_CAPACITY {
            let mut ages: Vec<(i64, String)> = stations
                .values()
                .map(|held| (held.last_seen, held.id.clone()))
                .collect();
            ages.sort();
            for (_, id) in ages.into_iter().take(stations.len() - STATION_CAPACITY) {
                stations.remove(&id);
            }
        }
        true
    }

    pub fn age_out(&mut self, max_age_ms: i64, now: i64) {
        for stations in self.stations.values_mut() {
            stations.retain(|_, station| station.last_seen >= now - max_age_ms);
        }
    }

    pub fn bearing(&mut self, node: &str, deg: f64, confidence: f64, now: i64) {
        if confidence <= 0.0 {
            return;
        }
        let finder = self.finders.entry(node.to_owned()).or_default();
        finder.history.push(Bearing {
            deg,
            confidence,
            at: now,
        });
        let overflow = finder.history.len().saturating_sub(BEARING_HISTORY);
        finder.history.drain(..overflow);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Topic {
    Positions,
    Stations,
    Finders,
    Satellites,
}

pub struct Feed {
    pub held: RefCell<Held>,
    positions: ArcRwSignal<u64>,
    stations: ArcRwSignal<u64>,
    finders: ArcRwSignal<u64>,
    satellites: ArcRwSignal<u64>,
    holders: RefCell<Vec<u64>>,
    next: Cell<u64>,
    seeded: Cell<bool>,
}

thread_local! {
    static FEED: Rc<Feed> = Rc::new(Feed {
        held: RefCell::new(Held::default()),
        positions: ArcRwSignal::new(0),
        stations: ArcRwSignal::new(0),
        finders: ArcRwSignal::new(0),
        satellites: ArcRwSignal::new(0),
        holders: RefCell::new(Vec::new()),
        next: Cell::new(0),
        seeded: Cell::new(false),
    });
}

impl Feed {
    pub fn get() -> Rc<Self> {
        FEED.with(Rc::clone)
    }

    fn signal(&self, topic: Topic) -> &ArcRwSignal<u64> {
        match topic {
            Topic::Positions => &self.positions,
            Topic::Stations => &self.stations,
            Topic::Finders => &self.finders,
            Topic::Satellites => &self.satellites,
        }
    }

    pub fn track(&self, topic: Topic) {
        self.signal(topic).with(|_| ());
    }

    fn bump(&self, topic: Topic) {
        self.signal(topic).update(|count| *count += 1);
    }

    pub fn with<T>(&self, topic: Topic, read: impl FnOnce(&Held) -> T) -> T {
        self.track(topic);
        read(&self.held.borrow())
    }

    pub fn change(&self, topic: Topic, edit: impl FnOnce(&mut Held)) {
        edit(&mut self.held.borrow_mut());
        self.bump(topic);
    }

    pub fn listen(self: &Rc<Self>, store: Store) {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.holders.borrow_mut().push(id);
        if !self.seeded.replace(true) {
            let seed = store.decoded.get_untracked();
            self.change(Topic::Stations, |held| {
                for record in seed.all_frames() {
                    held.decoded(record);
                }
            });
        }
        let feed = self.clone();
        on_cleanup_local(move || feed.holders.borrow_mut().retain(|held| *held != id));
        let feed = self.clone();
        store.on_event(move |event| {
            if feed.holders.borrow().first() == Some(&id) {
                feed.absorb(event);
            }
        });
    }

    fn absorb(&self, event: &ServerEvent) {
        let now = now_ms();
        match event {
            ServerEvent::PositionChanged { node, fix, error } => {
                self.change(Topic::Positions, |held| {
                    held.position(node, fix.clone(), error.clone(), now)
                });
            }
            ServerEvent::Decoded(record) => {
                if self.held.borrow_mut().decoded(record) {
                    self.bump(Topic::Stations);
                }
            }
            ServerEvent::DecodedBacklog { records } => {
                let mut changed = false;
                for record in records {
                    changed |= self.held.borrow_mut().decoded(record);
                }
                if changed {
                    self.bump(Topic::Stations);
                }
            }
            ServerEvent::DfUpdate { node, reading, .. } => {
                let (deg, confidence) = (
                    f64::from(reading.bearing_deg),
                    f64::from(reading.confidence),
                );
                self.change(Topic::Finders, |held| {
                    held.bearing(node, deg, confidence, now)
                });
            }
            ServerEvent::DfFusionUpdate { node, state } => {
                self.change(Topic::Finders, |held| {
                    held.finders.entry(node.clone()).or_default().fusion = Some((**state).clone());
                });
            }
            ServerEvent::RadarDetections {
                node, detections, ..
            } => {
                self.change(Topic::Finders, |held| {
                    held.finders
                        .entry(node.clone())
                        .or_default()
                        .detections
                        .clone_from(detections);
                });
            }
            ServerEvent::SatelliteUpdate { status } => {
                self.change(Topic::Satellites, |held| {
                    held.satellites
                        .insert(status.node.clone(), (**status).clone());
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::decode::{AdsbMessage, AisMessage};

    use super::*;
    use crate::ui::kit_maps::iso_of;

    fn sample(lat: f64, received_at: i64) -> Sample {
        Sample {
            at: Geo::new(lat, 13.0),
            altitude_m: None,
            accuracy_m: None,
            received_at,
        }
    }

    #[test]
    fn a_fix_that_did_not_move_refreshes_the_last_sample() {
        let mut history = Vec::new();
        append(&mut history, sample(52.0, 1));
        append(&mut history, sample(52.0, 2));
        append(&mut history, sample(52.1, 3));
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].received_at, 2);
    }

    fn adsb(data: AdsbMessage, at: i64) -> DecodedRecord {
        DecodedRecord {
            origin: None,
            device_set: 0,
            channel: 0,
            at: iso_of(at),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(data),
            sinks: Vec::new(),
        }
    }

    #[test]
    fn a_station_keeps_what_earlier_frames_said() {
        let mut held = Held::default();
        let first = AdsbMessage {
            icao: "3c6444".to_owned(),
            df: 17,
            raw: "8d".to_owned(),
            callsign: Some("DLH123".to_owned()),
            ..AdsbMessage::default()
        };
        let second = AdsbMessage {
            lat: Some(52.5),
            lon: Some(13.4),
            ..first.clone()
        };
        let second = AdsbMessage {
            callsign: None,
            ..second
        };
        assert!(held.decoded(&adsb(first, 1_000)));
        assert!(held.decoded(&adsb(second, 2_000)));
        let station = &held.stations["adsb"]["3c6444"];
        assert_eq!(station.frames, 2);
        assert_eq!(station.last_seen, 2_000);
        let DecoderEvent::Adsb(merged) = &station.event else {
            panic!("an adsb event");
        };
        assert_eq!(merged.callsign.as_deref(), Some("DLH123"));
        assert_eq!(merged.lat, Some(52.5));
    }

    #[test]
    fn stations_age_out_and_only_positioned_kinds_are_held() {
        let mut held = Held::default();
        let ship = DecoderEvent::Ais(AisMessage {
            mmsi: 211_234_560,
            ..AisMessage::default()
        });
        assert_eq!(station_id(&ship).as_deref(), Some("211234560"));
        held.decoded(&adsb(
            AdsbMessage {
                icao: "a".to_owned(),
                ..AdsbMessage::default()
            },
            1_000,
        ));
        held.age_out(500, 2_000);
        assert!(held.stations["adsb"].is_empty());
    }

    #[test]
    fn a_bearing_trail_keeps_only_confident_recent_readings() {
        let mut held = Held::default();
        held.bearing("df", 45.0, 0.0, 1);
        assert!(!held.finders.contains_key("df"));
        for at in 0..(BEARING_HISTORY as i64 + 5) {
            held.bearing("df", 45.0, 0.9, at);
        }
        assert_eq!(held.finders["df"].history.len(), BEARING_HISTORY);
        assert_eq!(held.finders["df"].history[0].at, 5);
    }
}
