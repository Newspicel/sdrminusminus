use sdrmm_engine::Engine;
use sdrmm_wire::DeviceSettings;

use crate::store::Store;

fn radio_of(engine: &Engine, device_set: u32) -> Option<(String, DeviceSettings)> {
    engine
        .snapshot()
        .device_sets
        .into_iter()
        .find(|set| set.id == device_set)
        .map(|set| (set.device.radio(), set.settings))
}

pub(crate) fn of(engine: &Engine, store: &Store, device_set: u32) -> DeviceSettings {
    let Some((radio, _)) = radio_of(engine, device_set) else {
        return DeviceSettings::default();
    };
    store.radio_calibration(&radio).unwrap_or_else(|err| {
        tracing::warn!(%err, radio, "could not read a radio's calibration");
        DeviceSettings::default()
    })
}

pub(crate) fn remember(
    engine: &Engine,
    store: &Store,
    device_set: u32,
    calibration: &DeviceSettings,
) {
    let Some((radio, _)) = radio_of(engine, device_set) else {
        return;
    };
    store_for(store, &radio, calibration);
}

pub(crate) fn remember_live(engine: &Engine, store: &Store, device_set: u32) {
    if let Some((radio, settings)) = radio_of(engine, device_set) {
        store_for(store, &radio, &settings.calibration());
    }
}

fn store_for(store: &Store, radio: &str, calibration: &DeviceSettings) {
    if let Err(err) = store.put_radio_calibration(radio, calibration) {
        tracing::warn!(%err, radio, "could not save a radio's calibration");
    }
}
