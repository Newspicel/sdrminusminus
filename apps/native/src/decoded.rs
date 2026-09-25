use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use sdrmm_wire::decode::{DecodedRecord, DecoderEvent};
use serde_json::Value;

pub const RING_CAPACITY: usize = 2000;
pub const BROADCAST_DATA_CAPACITY: usize = 8;
pub const STATION_CAPACITY: usize = 1000;

pub type Frames = Arc<VecDeque<Arc<DecodedRecord>>>;
pub type Stations = Arc<HashMap<String, Arc<Station>>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub kind: &'static str,
    pub id: String,
    pub event: DecoderEvent,
    pub last_seen_ms: i64,
    pub freq_hz: f64,
    pub device_set: u32,
    pub channel: u32,
    pub frames: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Decoded {
    frames: HashMap<&'static str, Frames>,
    stations: HashMap<&'static str, Stations>,
    pub lost: u64,
    pub received: u64,
}

impl Decoded {
    #[must_use]
    pub fn frames(&self, kind: &str) -> Option<&Frames> {
        self.frames.get(kind)
    }

    #[must_use]
    pub fn stations(&self, kind: &str) -> Option<&Stations> {
        self.stations.get(kind)
    }

    pub fn all_frames(&self) -> impl Iterator<Item = &Arc<DecodedRecord>> {
        self.frames.values().flat_map(|slice| slice.iter())
    }

    pub fn publish(&mut self, batch: Vec<DecodedRecord>) {
        self.received += batch.len() as u64;
        for record in batch {
            self.merge_station(&record);
            let kind = record.event.kind();
            let slice = Arc::make_mut(self.frames.entry(kind).or_default());
            slice.push_front(Arc::new(record));
            slice.truncate(capacity_of(kind));
        }
    }

    pub fn hydrate(&mut self, records: &[DecodedRecord]) -> bool {
        let mut touched = false;
        for record in records {
            touched |= self.merge_station(record);
        }
        touched
    }

    pub fn report_lost(&mut self, count: u64) {
        self.lost += count;
    }

    #[must_use]
    pub fn stale(&self, max_age_ms: i64, now_ms: i64) -> bool {
        let cutoff = now_ms - max_age_ms;
        self.stations.values().any(|stations| {
            stations
                .values()
                .any(|station| station.last_seen_ms < cutoff)
        })
    }

    pub fn age_out(&mut self, max_age_ms: i64, now_ms: i64) {
        let cutoff = now_ms - max_age_ms;
        for stations in self.stations.values_mut() {
            if stations
                .values()
                .any(|station| station.last_seen_ms < cutoff)
            {
                Arc::make_mut(stations).retain(|_, station| station.last_seen_ms >= cutoff);
            }
        }
    }

    pub fn drop_frames(&mut self, matches: impl Fn(&DecodedRecord) -> bool) -> usize {
        let mut dropped = 0;
        for slice in self.frames.values_mut() {
            let hit = slice.iter().filter(|record| matches(record)).count();
            if hit > 0 {
                dropped += hit;
                Arc::make_mut(slice).retain(|record| !matches(record));
            }
        }
        dropped
    }

    fn merge_station(&mut self, record: &DecodedRecord) -> bool {
        let Some(id) = station_id(&record.event) else {
            return false;
        };
        let kind = record.event.kind();
        let stations = Arc::make_mut(self.stations.entry(kind).or_default());
        let previous = stations.get(&id);
        let event = previous.map_or_else(
            || record.event.clone(),
            |previous| merge_forward(&previous.event, &record.event),
        );
        let frames = previous.map_or(0, |previous| previous.frames) + 1;
        stations.insert(
            id.clone(),
            Arc::new(Station {
                kind,
                id,
                event,
                last_seen_ms: record_time_ms(record),
                freq_hz: record.freq_hz,
                device_set: record.device_set,
                channel: record.channel,
                frames,
            }),
        );
        evict_oldest(stations);
        true
    }
}

fn capacity_of(kind: &str) -> usize {
    if kind == "broadcast_data" {
        BROADCAST_DATA_CAPACITY
    } else {
        RING_CAPACITY
    }
}

fn evict_oldest(stations: &mut HashMap<String, Arc<Station>>) {
    let excess = stations.len().saturating_sub(STATION_CAPACITY);
    if excess == 0 {
        return;
    }
    let mut by_age: Vec<(i64, String)> = stations
        .values()
        .map(|station| (station.last_seen_ms, station.id.clone()))
        .collect();
    by_age.sort_unstable();
    for (_, id) in by_age.into_iter().take(excess) {
        stations.remove(&id);
    }
}

#[must_use]
pub fn station_id(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Adsb(message) => Some(message.icao.clone()),
        DecoderEvent::Ais(message) => Some(message.mmsi.to_string()),
        DecoderEvent::Aprs(packet) => Some(packet.source.clone()),
        DecoderEvent::Call(call) => call.source.map(|source| source.to_string()),
        DecoderEvent::Pocsag(page) => Some(page.address.to_string()),
        DecoderEvent::Flex(page) => Some(page.address.to_string()),
        DecoderEvent::Ermes(page) => Some(page.local_address.to_string()),
        DecoderEvent::Rds(update) => update.pi.clone(),
        DecoderEvent::Dect(frame) => frame.identity.as_ref().map(|id| id.rfpi.clone()),
        DecoderEvent::Df(bearing) => bearing.station_id.clone(),
        _ => None,
    }
}

#[must_use]
pub fn merge_forward(previous: &DecoderEvent, next: &DecoderEvent) -> DecoderEvent {
    let (Ok(Value::Object(mut older)), Ok(Value::Object(newer))) =
        (serde_json::to_value(previous), serde_json::to_value(next))
    else {
        return next.clone();
    };
    if let (Some(Value::Object(data)), Some(Value::Object(fresh))) =
        (older.get_mut("data"), newer.get("data"))
    {
        for (key, value) in fresh {
            if !value.is_null() {
                data.insert(key.clone(), value.clone());
            }
        }
    } else {
        return next.clone();
    }
    serde_json::from_value(Value::Object(older)).unwrap_or_else(|_| next.clone())
}

#[must_use]
pub fn time_ms(at: &str) -> Option<i64> {
    at.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
}

#[must_use]
pub fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn record_time_ms(record: &DecodedRecord) -> i64 {
    time_ms(&record.at).unwrap_or_else(now_ms)
}

#[cfg(test)]
mod tests;
