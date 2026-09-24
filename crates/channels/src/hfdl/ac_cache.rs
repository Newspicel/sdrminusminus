use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const DEFAULT_TTL: Duration = Duration::from_secs(3_600);

struct Entry {
    icao: String,
    inserted: Instant,
}

pub struct AcCache {
    map: HashMap<u8, Entry>,
    ttl: Duration,
}

impl AcCache {
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            map: HashMap::new(),
            ttl,
        }
    }

    pub fn insert(&mut self, ac_id: u8, icao: &str) {
        self.remove_by_icao(icao);
        self.map.insert(
            ac_id,
            Entry {
                icao: icao.to_owned(),
                inserted: Instant::now(),
            },
        );
    }

    pub fn lookup(&self, ac_id: u8) -> Option<&str> {
        self.map
            .get(&ac_id)
            .filter(|entry| entry.inserted.elapsed() <= self.ttl)
            .map(|entry| entry.icao.as_str())
    }

    pub fn remove_by_icao(&mut self, icao: &str) {
        self.map.retain(|_, entry| entry.icao != icao);
    }

    #[cfg(test)]
    fn live(&self) -> usize {
        self.map
            .values()
            .filter(|entry| entry.inserted.elapsed() <= self.ttl)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_lookup() {
        let mut cache = AcCache::new();
        cache.insert(0xC7, "040087");
        assert_eq!(cache.lookup(0xC7), Some("040087"));
        assert_eq!(cache.lookup(0x01), None);
    }

    #[test]
    fn remove_by_icao_clears_entry() {
        let mut cache = AcCache::new();
        cache.insert(0xC7, "040087");
        cache.remove_by_icao("040087");
        assert_eq!(cache.lookup(0xC7), None);
        assert_eq!(cache.live(), 0);
    }

    #[test]
    fn relogon_under_new_id_drops_old_mapping() {
        let mut cache = AcCache::new();
        cache.insert(0x10, "04C11B");
        cache.insert(0x20, "04C11B");
        assert_eq!(cache.lookup(0x20), Some("04C11B"));
        assert_eq!(cache.lookup(0x10), None);
        assert_eq!(cache.live(), 1);
    }

    #[test]
    fn entries_expire_after_ttl() {
        let mut cache = AcCache::with_ttl(Duration::ZERO);
        cache.insert(0xC7, "040087");
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(cache.lookup(0xC7), None);
        assert_eq!(cache.live(), 0);
    }
}
