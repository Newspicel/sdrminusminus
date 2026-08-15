use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use sdrmm_wire::DecodedRecord;

const RECORDS_PER_STATION: usize = 6;

const STATIONS_PER_KIND: usize = 1_000;

const TTL: Duration = Duration::from_secs(300);

const MAX_BACKLOG: usize = 2_000;

#[derive(Debug)]
struct Station {
    records: Vec<DecodedRecord>,
    last_seen: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct Tracks {
    stations: Mutex<HashMap<(&'static str, String), Station>>,
}

impl Tracks {
    pub(crate) fn observe(&self, record: &DecodedRecord) {
        self.observe_at(record, Instant::now());
    }

    fn observe_at(&self, record: &DecodedRecord, now: Instant) {
        let Some(id) = record.event.station() else {
            return;
        };
        let kind = record.event.kind();
        let mut stations = self.lock();
        let station = stations.entry((kind, id)).or_insert_with(|| Station {
            records: Vec::with_capacity(RECORDS_PER_STATION),
            last_seen: now,
        });
        station.last_seen = now;
        if station.records.len() == RECORDS_PER_STATION {
            station.records.remove(0);
        }
        station.records.push(record.clone());
        prune(&mut stations, kind, now);
    }

    pub(crate) fn backlog(&self) -> Vec<DecodedRecord> {
        self.backlog_at(Instant::now())
    }

    fn backlog_at(&self, now: Instant) -> Vec<DecodedRecord> {
        let stations = self.lock();
        let mut live: Vec<&Station> = stations
            .values()
            .filter(|station| now.duration_since(station.last_seen) < TTL)
            .collect();
        live.sort_unstable_by_key(|station| std::cmp::Reverse(station.last_seen));

        let mut records: Vec<DecodedRecord> = Vec::new();
        for station in live {
            if records.len() + station.records.len() > MAX_BACKLOG {
                break;
            }
            records.extend(station.records.iter().cloned());
        }
        records.sort_by(|a, b| a.at.cmp(&b.at));
        records
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(&'static str, String), Station>> {
        self.stations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn prune(
    stations: &mut HashMap<(&'static str, String), Station>,
    kind: &'static str,
    now: Instant,
) {
    let of_kind = |stations: &HashMap<(&'static str, String), Station>| {
        stations.keys().filter(|(held, _)| *held == kind).count()
    };
    if of_kind(stations) <= STATIONS_PER_KIND {
        return;
    }
    stations.retain(|_, station| now.duration_since(station.last_seen) < TTL);
    let excess = of_kind(stations).saturating_sub(STATIONS_PER_KIND);
    if excess == 0 {
        return;
    }
    let mut by_age: Vec<(String, Instant)> = stations
        .iter()
        .filter(|((held, _), _)| *held == kind)
        .map(|((_, id), station)| (id.clone(), station.last_seen))
        .collect();
    by_age.sort_unstable_by_key(|(_, last_seen)| *last_seen);
    for (id, _) in by_age.into_iter().take(excess) {
        stations.remove(&(kind, id));
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AdsbMessage, DecoderEvent, MorseText};

    use super::*;

    fn adsb(icao: &str, at: &str) -> DecodedRecord {
        DecodedRecord {
            device_set: 0,
            channel: 0,
            at: at.to_string(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(AdsbMessage {
                icao: icao.to_string(),
                ..AdsbMessage::default()
            }),
        }
    }

    #[test]
    fn a_station_keeps_its_last_records_oldest_first() {
        let tracks = Tracks::default();
        for n in 0..RECORDS_PER_STATION + 3 {
            tracks.observe(&adsb(
                "3c6444",
                &format!("2026-08-10T00:00:{n:02}.000000000Z"),
            ));
        }

        let backlog = tracks.backlog();
        assert_eq!(backlog.len(), RECORDS_PER_STATION);
        assert_eq!(backlog[0].at, "2026-08-10T00:00:03.000000000Z");
        assert!(backlog.windows(2).all(|pair| pair[0].at <= pair[1].at));
    }

    #[test]
    fn records_from_several_stations_come_back_in_time_order() {
        let tracks = Tracks::default();
        tracks.observe(&adsb("aaaaaa", "2026-08-10T00:00:03.000000000Z"));
        tracks.observe(&adsb("bbbbbb", "2026-08-10T00:00:01.000000000Z"));
        tracks.observe(&adsb("aaaaaa", "2026-08-10T00:00:04.000000000Z"));

        let backlog = tracks.backlog();
        let ats: Vec<&str> = backlog.iter().map(|record| record.at.as_str()).collect();
        assert_eq!(
            ats,
            [
                "2026-08-10T00:00:01.000000000Z",
                "2026-08-10T00:00:03.000000000Z",
                "2026-08-10T00:00:04.000000000Z",
            ]
        );
    }

    #[test]
    fn a_silent_station_ages_out() {
        let tracks = Tracks::default();
        let start = Instant::now();
        tracks.observe_at(&adsb("3c6444", "2026-08-10T00:00:00.000000000Z"), start);

        assert_eq!(
            tracks
                .backlog_at(start + TTL - Duration::from_secs(1))
                .len(),
            1
        );
        assert!(tracks.backlog_at(start + TTL).is_empty());
    }

    #[test]
    fn an_event_with_no_station_identity_is_not_tracked() {
        let tracks = Tracks::default();
        tracks.observe(&DecodedRecord {
            device_set: 0,
            channel: 0,
            at: "2026-08-10T00:00:00.000000000Z".to_string(),
            freq_hz: 14_000_000.0,
            event: DecoderEvent::Morse(MorseText {
                text: "CQ".to_string(),
                wpm: 18.0,
            }),
        });

        assert!(tracks.backlog().is_empty());
    }

    #[test]
    fn a_kind_over_its_cap_gives_up_its_oldest_station() {
        let tracks = Tracks::default();
        let start = Instant::now();
        for n in 0..=STATIONS_PER_KIND {
            tracks.observe_at(
                &adsb(&format!("{n:06x}"), "2026-08-10T00:00:00.000000000Z"),
                start + Duration::from_millis(n as u64),
            );
        }

        let held = tracks.lock();
        assert_eq!(held.len(), STATIONS_PER_KIND);
        assert!(!held.contains_key(&("adsb", "000000".to_string())));
    }

    #[test]
    fn the_backlog_is_capped() {
        let tracks = Tracks::default();
        let start = Instant::now();
        let stations = MAX_BACKLOG / RECORDS_PER_STATION + 10;
        for n in 0..stations {
            for r in 0..RECORDS_PER_STATION {
                tracks.observe_at(
                    &adsb(
                        &format!("{n:06x}"),
                        &format!("2026-08-10T00:00:{r:02}.000000000Z"),
                    ),
                    start + Duration::from_millis(n as u64),
                );
            }
        }

        assert!(tracks.backlog_at(start + Duration::from_secs(1)).len() <= MAX_BACKLOG);
    }
}
