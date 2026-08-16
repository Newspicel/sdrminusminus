use std::time::{Duration, Instant};

use sdrmm_wire::DeviceInfo;

/// How long a network search's result stands before another request may pay for a fresh one.
/// A plugged or unplugged radio expires it early, so this only paces repeated asking.
const SETTLED: Duration = Duration::from_secs(10);

/// What the last network-reaching search found beyond what is attached to this machine.
///
/// A radio answering over the network takes seconds to find, which is too long to make the device
/// list wait: the list is answered from what is attached now plus whatever the last deep search
/// turned up, and the fresh search runs behind it.
#[derive(Debug, Default)]
pub(crate) struct Discovery {
    extras: Vec<DeviceInfo>,
    searching: bool,
    searched: Option<Instant>,
}

impl Discovery {
    pub(crate) fn merge(&self, mut attached: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
        let ids: Vec<String> = attached.iter().map(DeviceInfo::id).collect();
        attached.extend(
            self.extras
                .iter()
                .filter(|extra| !ids.contains(&extra.id()))
                .cloned(),
        );
        attached.sort_by_key(DeviceInfo::id);
        attached
    }

    /// Claims the right to search, so that a burst of requests starts one search and not one each.
    pub(crate) fn claim(&mut self, now: Instant) -> bool {
        if self.searching || self.searched.is_some_and(|at| now < at + SETTLED) {
            return false;
        }
        self.searching = true;
        true
    }

    /// Files what the search found and reports whether the device list changed because of it.
    pub(crate) fn searched(&mut self, extras: Vec<DeviceInfo>, now: Instant) -> bool {
        self.searching = false;
        self.searched = Some(now);
        let changed = self.extras != extras;
        self.extras = extras;
        changed
    }

    /// Lets the next request search again, for when the machine changed under us.
    pub(crate) fn expire(&mut self) {
        self.searched = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(driver: &str, key: &str) -> DeviceInfo {
        DeviceInfo {
            driver: driver.to_string(),
            key: key.to_string(),
            label: format!("{driver} {key}"),
            serial: None,
            profile: None,
        }
    }

    fn ids(devices: &[DeviceInfo]) -> Vec<String> {
        devices.iter().map(DeviceInfo::id).collect()
    }

    #[test]
    fn a_network_radio_joins_the_attached_ones() {
        let mut discovery = Discovery::default();
        discovery.searched(vec![device("soapy", "remote")], Instant::now());
        assert_eq!(
            ids(&discovery.merge(vec![device("rtlsdr", "0")])),
            vec!["rtlsdr:0", "soapy:remote"]
        );
    }

    #[test]
    fn a_radio_the_quick_search_already_found_is_listed_once() {
        let mut discovery = Discovery::default();
        discovery.searched(vec![device("soapy", "one")], Instant::now());
        assert_eq!(
            ids(&discovery.merge(vec![device("soapy", "one")])),
            vec!["soapy:one"]
        );
    }

    #[test]
    fn one_search_runs_however_often_the_list_is_asked_for() {
        let now = Instant::now();
        let mut discovery = Discovery::default();
        assert!(discovery.claim(now));
        assert!(!discovery.claim(now), "a search is already running");
        discovery.searched(Vec::new(), now);
        assert!(!discovery.claim(now), "the answer is still fresh");
        assert!(discovery.claim(now + SETTLED));
    }

    #[test]
    fn a_changed_machine_earns_a_fresh_search_at_once() {
        let now = Instant::now();
        let mut discovery = Discovery::default();
        assert!(discovery.claim(now));
        discovery.searched(Vec::new(), now);
        discovery.expire();
        assert!(discovery.claim(now));
    }

    #[test]
    fn only_a_different_result_counts_as_a_change() {
        let now = Instant::now();
        let mut discovery = Discovery::default();
        assert!(discovery.searched(vec![device("soapy", "remote")], now));
        assert!(!discovery.searched(vec![device("soapy", "remote")], now));
        assert!(discovery.searched(Vec::new(), now));
    }
}
