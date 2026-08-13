//! Driver registry (PLAN §6): probes every registered backend and merges results, collapsing
//! duplicates by serial with native drivers winning over Soapy. At M0 only `device-virtual`
//! registers, but the merge policy is here from the start.

use std::collections::HashMap;

use sdrmm_wire::DeviceInfo;

use crate::{DeviceDriver, DeviceError, SdrDevice};

/// Priority when the same physical device is seen through multiple drivers: higher wins
/// (native RTL/HackRF claim priority over Soapy, PLAN §6). Order of registration is the
/// tie-breaker for equal priority.
#[derive(Default)]
pub struct DeviceRegistry {
    drivers: Vec<(u8, Box<dyn DeviceDriver>)>,
}

impl DeviceRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a driver with a merge priority (native backends use a higher value).
    pub fn register(&mut self, priority: u8, driver: Box<dyn DeviceDriver>) {
        self.drivers.push((priority, driver));
    }

    /// Registered driver ids with their merge priorities, in registration order. This is what
    /// `sdrmm --doctor` prints as "which backends this build has" — derived from the registry
    /// rather than from a second list of feature flags that could disagree with it.
    #[must_use]
    pub fn driver_ids(&self) -> Vec<(u8, &'static str)> {
        self.drivers.iter().map(|(p, d)| (*p, d.id())).collect()
    }

    /// Probe all drivers and merge, collapsing serial duplicates by priority (PLAN §6).
    #[must_use]
    pub fn probe_all(&self) -> Vec<DeviceInfo> {
        let mut by_serial: HashMap<String, (u8, DeviceInfo)> = HashMap::new();
        let mut serialless: Vec<DeviceInfo> = Vec::new();

        for (priority, driver) in &self.drivers {
            for info in driver.probe() {
                match &info.serial {
                    Some(serial) => {
                        let entry = by_serial.entry(serial.clone());
                        match entry {
                            std::collections::hash_map::Entry::Occupied(mut e) => {
                                if *priority > e.get().0 {
                                    e.insert((*priority, info));
                                }
                            }
                            std::collections::hash_map::Entry::Vacant(e) => {
                                e.insert((*priority, info));
                            }
                        }
                    }
                    None => serialless.push(info),
                }
            }
        }

        let mut out: Vec<DeviceInfo> = by_serial.into_values().map(|(_, info)| info).collect();
        out.extend(serialless);
        out.sort_by_key(DeviceInfo::id);
        out
    }

    /// Open the device identified by `driver:key` (PLAN §5 `device_id`), returning the probed
    /// [`DeviceInfo`] alongside it so callers keep the real label/serial instead of
    /// reconstructing one from the id string.
    ///
    /// A key no probe reports is offered to the driver's [`DeviceDriver::resolve`] before this
    /// gives up, which is how a network receiver — named by an operator, discoverable by nobody —
    /// is opened the first time.
    pub fn open(&self, device_id: &str) -> Result<(DeviceInfo, Box<dyn SdrDevice>), DeviceError> {
        let (driver, info) = self
            .find(device_id)
            .ok_or_else(|| DeviceError::NotFound(device_id.to_string()))?;
        let device = driver.open(&info)?;
        Ok((info, device))
    }

    /// Adopt a device by `driver:key` without opening it, so a later probe reports it.
    ///
    /// What restores a network receiver after a restart: the endpoint lives only in the stored
    /// workspace that names it, and a driver that has not been told about it would probe empty,
    /// leaving the node bound to a radio nothing ever offers. `None` for every driver that
    /// enumerates real hardware, where a key no probe found names a device that is not attached.
    #[must_use]
    pub fn resolve(&self, device_id: &str) -> Option<DeviceInfo> {
        let (driver_id, key) = device_id.split_once(':')?;
        self.drivers
            .iter()
            .filter(|(_, driver)| driver.id() == driver_id)
            .find_map(|(_, driver)| driver.resolve(key))
    }

    /// The driver that owns `driver:key` and the device it names: the probe result where there is
    /// one, an adopted key otherwise. Probes are consulted across every driver sharing the id
    /// before any of them is asked to adopt, so a discovered device always wins over a named one.
    fn find(&self, device_id: &str) -> Option<(&dyn DeviceDriver, DeviceInfo)> {
        let (driver_id, key) = device_id.split_once(':')?;
        let matching = || {
            self.drivers
                .iter()
                .filter(move |(_, driver)| driver.id() == driver_id)
                .map(|(_, driver)| &**driver)
        };
        matching()
            .find_map(|driver| {
                driver
                    .probe()
                    .into_iter()
                    .find(|d| d.key == key)
                    .map(|info| (driver, info))
            })
            .or_else(|| matching().find_map(|driver| Some((driver, driver.resolve(key)?))))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use sdrmm_wire::{Capabilities, DeviceSettings, Duplex, StreamScope};

    use super::*;
    use crate::{RxSink, lock};

    /// A driver over a fixed probe list, optionally able to adopt any key it is asked for — the
    /// two shapes the registry has to tell apart (attached hardware, and a named endpoint).
    struct Fake {
        id: &'static str,
        probed: Mutex<Vec<DeviceInfo>>,
        adopts: bool,
    }

    impl Fake {
        fn new(id: &'static str, keys: &[&str]) -> Self {
            Self {
                id,
                probed: Mutex::new(keys.iter().map(|key| info(id, key)).collect()),
                adopts: false,
            }
        }

        fn adopting(id: &'static str) -> Self {
            Self {
                id,
                probed: Mutex::new(Vec::new()),
                adopts: true,
            }
        }
    }

    fn info(driver: &str, key: &str) -> DeviceInfo {
        DeviceInfo {
            driver: driver.to_string(),
            key: key.to_string(),
            label: format!("{driver} {key}"),
            serial: None,
            profile: None,
        }
    }

    impl DeviceDriver for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            lock(&self.probed).clone()
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(FakeDevice))
        }

        /// Adopts by lowercasing, so a test can prove callers take the *canonical* key back
        /// rather than the one they asked with.
        fn resolve(&self, key: &str) -> Option<DeviceInfo> {
            if !self.adopts {
                return None;
            }
            let canonical = info(self.id, &key.to_ascii_lowercase());
            let mut probed = lock(&self.probed);
            if !probed.iter().any(|d| d.key == canonical.key) {
                probed.push(canonical.clone());
            }
            Some(canonical)
        }
    }

    struct FakeDevice;

    impl SdrDevice for FakeDevice {
        fn capabilities(&self) -> &Capabilities {
            static CAPS: std::sync::OnceLock<Capabilities> = std::sync::OnceLock::new();
            CAPS.get_or_init(|| Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
                sample_rate_range: None,
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                extra: Vec::new(),
                ppm: false,
                duplex: Duplex::RxOnly,
                rx_streams: 1,
                tx_streams: 0,
                per_stream: StreamScope::default(),
                directional: None,
            })
        }

        fn settings(&self) -> &DeviceSettings {
            static SETTINGS: std::sync::OnceLock<DeviceSettings> = std::sync::OnceLock::new();
            SETTINGS.get_or_init(DeviceSettings::default)
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, _sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_stop(&mut self) {}
    }

    fn registry(drivers: impl IntoIterator<Item = Fake>) -> DeviceRegistry {
        let mut registry = DeviceRegistry::new();
        for driver in drivers {
            registry.register(10, Box::new(driver));
        }
        registry
    }

    #[test]
    fn open_finds_a_probed_device() {
        let registry = registry([Fake::new("mock", &["one", "two"])]);
        let (info, _) = registry.open("mock:two").expect("probed device");
        assert_eq!(info.id(), "mock:two");
    }

    #[test]
    fn a_key_no_probe_reports_is_not_found_unless_the_driver_adopts_it() {
        let registry = registry([Fake::new("mock", &["one"])]);
        assert!(matches!(
            registry.open("mock:missing"),
            Err(DeviceError::NotFound(_))
        ));
        assert!(registry.resolve("mock:missing").is_none());
    }

    /// The network-receiver path end to end: an operator names an endpoint no probe could have
    /// found, and from then on it *is* probed — which is what keeps the engine from faulting the
    /// running set it belongs to, and what lets a stored workspace bind it again.
    #[test]
    fn an_adopted_key_opens_and_then_appears_in_the_probe() {
        let registry = registry([Fake::adopting("net")]);
        assert!(registry.probe_all().is_empty());
        let (info, _) = registry.open("net:Host:1234").expect("adopted endpoint");
        assert_eq!(info.id(), "net:host:1234", "the canonical key comes back");
        assert_eq!(
            registry
                .probe_all()
                .iter()
                .map(DeviceInfo::id)
                .collect::<Vec<_>>(),
            vec!["net:host:1234".to_string()]
        );
        registry.open("net:host:1234").expect("now probed");
    }

    #[test]
    fn resolve_adopts_without_opening() {
        let registry = registry([Fake::adopting("net")]);
        let info = registry.resolve("net:HOST:1234").expect("adopted");
        assert_eq!(info.id(), "net:host:1234");
        assert_eq!(registry.probe_all().len(), 1);
        assert!(registry.resolve("other:host:1234").is_none());
        assert!(registry.resolve("no-colon").is_none());
    }

    /// A driver that adopts anything must not answer for a key another driver actually has: the
    /// probe list is consulted across every driver before any of them is offered the key.
    #[test]
    fn a_probed_device_wins_over_an_adopting_driver_with_the_same_id() {
        let registry = registry([Fake::adopting("mock"), Fake::new("mock", &["real"])]);
        let (info, _) = registry.open("mock:real").expect("the probed one");
        assert_eq!(info.label, "mock real");
    }
}
