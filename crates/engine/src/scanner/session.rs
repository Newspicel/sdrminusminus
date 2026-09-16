use std::sync::Arc;

use sdrmm_wire::{ChannelSettings, ScanSettings, ScannerStatus, ServerEvent, StateScope};

use super::{ScanPlan, spawn};
use crate::{DeviceSetState, DeviceSetStatus, Engine, EngineError};

fn admits_a_scan(state: &DeviceSetState, ds: u32) -> Result<(), EngineError> {
    if state.scanner.is_some() {
        return Err(EngineError::Scan(format!(
            "device set {ds} is already scanning"
        )));
    }
    if state.hunt.is_some() {
        return Err(EngineError::Scan(format!(
            "device set {ds} is hunting; a sweep would carry the radio off the decoder"
        )));
    }
    if state.status != DeviceSetStatus::Running {
        return Err(EngineError::Scan(format!("device set {ds} is not running")));
    }
    Ok(())
}

fn decoder_bandwidth_hz(settings: &ChannelSettings) -> f64 {
    let (low, high) = sdrmm_channels::occupied_band(&settings.params);
    high - low
}

fn admit(
    engine: &Engine,
    ds: u32,
    settings: &mut ScanSettings,
    plan: &ScanPlan,
) -> Result<u32, EngineError> {
    let inner = engine.lock();
    let state = inner
        .device_sets
        .get(&ds)
        .ok_or(EngineError::DeviceSetNotFound(ds))?;
    admits_a_scan(state, ds)?;
    let decoder = state
        .channels
        .iter()
        .find(|c| c.id == settings.channel)
        .ok_or(EngineError::ChannelNotFound(settings.channel, ds))?;
    settings.measure_bw_hz = Some(
        settings
            .measure_bw_hz
            .unwrap_or_else(|| decoder_bandwidth_hz(&decoder.settings)),
    );
    plan.check_reach(&state.capabilities.freq_ranges)?;
    Ok(decoder.stream)
}

pub(crate) fn start(
    engine: &Arc<Engine>,
    ds: u32,
    mut settings: ScanSettings,
) -> Result<ScannerStatus, EngineError> {
    let plan = ScanPlan::build(&settings)?;
    let stream = admit(engine, ds, &mut settings, &plan)?;
    let decoder = settings.channel;
    let worker = spawn(engine, ds, plan, settings, decoder, stream)?;
    let status = worker.status();
    {
        let mut inner = engine.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            drop(inner);
            worker.stop_and_join();
            return Err(EngineError::DeviceSetNotFound(ds));
        };
        if state.scanner.is_some() {
            drop(inner);
            worker.stop_and_join();
            return Err(EngineError::Scan(format!(
                "device set {ds} is already scanning"
            )));
        }
        state.scanner = Some(worker);
        inner.revision += 1;
    }
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

pub(crate) fn stop(engine: &Engine, ds: u32) -> Result<ScannerStatus, EngineError> {
    let worker = {
        let mut inner = engine.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let worker = state
            .scanner
            .take()
            .ok_or_else(|| EngineError::Scan("no scan is running".to_string()))?;
        inner.revision += 1;
        worker
    };
    let status = worker.stop_and_join();
    engine.settle_tuning(ds);
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

pub(crate) fn skip(engine: &Engine, ds: u32) -> Result<ScannerStatus, EngineError> {
    let inner = engine.lock();
    let state = inner
        .device_sets
        .get(&ds)
        .ok_or(EngineError::DeviceSetNotFound(ds))?;
    let worker = state
        .scanner
        .as_ref()
        .ok_or_else(|| EngineError::Scan("no scan is running".to_string()))?;
    worker.skip()
}
