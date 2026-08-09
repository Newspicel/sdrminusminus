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
    pub fn open(&self, device_id: &str) -> Result<(DeviceInfo, Box<dyn SdrDevice>), DeviceError> {
        let (driver_id, key) = device_id
            .split_once(':')
            .ok_or_else(|| DeviceError::NotFound(device_id.to_string()))?;

        for (_, driver) in &self.drivers {
            if driver.id() != driver_id {
                continue;
            }
            if let Some(info) = driver.probe().into_iter().find(|d| d.key == key) {
                let device = driver.open(&info)?;
                return Ok((info, device));
            }
        }
        Err(DeviceError::NotFound(device_id.to_string()))
    }
}
