//! `sdrmm-engine` — the flowgraph runtime (PLAN §2, §7). Owns the authoritative device-set
//! state, hosts each device set's capture + DSP threads, and pushes `StateChanged` events and
//! spectrum snapshots outward. The control plane (this facade) uses a mutex; the DSP plane
//! (see [`runtime`]) is lock-free and never shares mutable state with it directly.

use std::{collections::BTreeMap, sync::Mutex};

use sdrmm_device::{DeviceError, DeviceRegistry};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_wire::{
    Capabilities, ChannelInfo, ChannelSettings, DeviceInfo, DeviceSet, DeviceSetStatus,
    DeviceSettings, ServerEvent, StateScope, StateSnapshot,
};
use tokio::sync::broadcast;

pub mod runtime;
pub use runtime::{SpectrumSnapshot, adaptive_db_window};

use crate::runtime::CaptureRuntime;

/// Merge priority for the built-in virtual driver (native backends register higher, PLAN §6).
const VIRTUAL_PRIORITY: u8 = 10;
const EVENT_CHANNEL_CAP: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("device set {0} not found")]
    DeviceSetNotFound(u32),
    #[error("channel {0} not found in device set {1}")]
    ChannelNotFound(u32, u32),
    #[error(transparent)]
    Device(#[from] DeviceError),
}

impl EngineError {
    /// Maps to HTTP 404 (missing device set / channel / device).
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::DeviceSetNotFound(_) | Self::ChannelNotFound(..))
            || matches!(self, Self::Device(DeviceError::NotFound(_)))
    }

    /// Maps to HTTP 400 (a well-formed request the device rejected).
    #[must_use]
    pub fn is_bad_request(&self) -> bool {
        matches!(
            self,
            Self::Device(DeviceError::Unsupported(_) | DeviceError::AlreadyStreaming)
        )
    }
}

/// Per-device-set control-plane state plus its running capture (PLAN §7). The runtime owns the
/// device and DSP thread; the rest is the serializable projection sent to clients.
struct DeviceSetState {
    info: DeviceInfo,
    capabilities: Capabilities,
    settings: DeviceSettings,
    status: DeviceSetStatus,
    channels: Vec<ChannelInfo>,
    next_channel_id: u32,
    error: Option<String>,
    runtime: CaptureRuntime,
}

impl DeviceSetState {
    fn project(&self, id: u32) -> DeviceSet {
        DeviceSet {
            id,
            device: self.info.clone(),
            capabilities: self.capabilities.clone(),
            settings: self.settings.clone(),
            status: self.status,
            channels: self.channels.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Default)]
struct Inner {
    device_sets: BTreeMap<u32, DeviceSetState>,
    next_ds_id: u32,
    revision: u64,
}

/// The engine. Cheap to share behind an `Arc`; all methods are `&self`.
pub struct Engine {
    registry: DeviceRegistry,
    inner: Mutex<Inner>,
    event_tx: broadcast::Sender<ServerEvent>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Build the engine with the built-in drivers registered (M0: virtual only).
    #[must_use]
    pub fn new() -> Self {
        let mut registry = DeviceRegistry::new();
        registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        Self {
            registry,
            inner: Mutex::new(Inner::default()),
            event_tx,
        }
    }

    /// Subscribe to the low-rate `ServerEvent` stream (state changes, errors).
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    /// Discovered devices across all drivers (PLAN §5 `GET /api/devices`).
    #[must_use]
    pub fn probe_devices(&self) -> Vec<DeviceInfo> {
        self.registry.probe_all()
    }

    /// Full authoritative snapshot (PLAN §5 `GET /api/state`).
    #[must_use]
    pub fn snapshot(&self) -> StateSnapshot {
        let inner = self.lock();
        StateSnapshot {
            device_sets: inner
                .device_sets
                .iter()
                .map(|(id, s)| s.project(*id))
                .collect(),
            revision: inner.revision,
        }
    }

    /// Open a device into a new device set and start streaming (PLAN §5 POST devicesets).
    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        let device = self.registry.open(device_id)?;
        let info = DeviceInfo {
            driver: device_id
                .split_once(':')
                .map(|(d, _)| d)
                .unwrap_or("")
                .to_string(),
            key: device_id
                .split_once(':')
                .map(|(_, k)| k)
                .unwrap_or(device_id)
                .to_string(),
            label: device_id.to_string(),
            serial: None,
        };
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();
        let center = settings.center_hz.unwrap_or(100_000_000.0);
        let rate = settings.sample_rate.unwrap_or(2_048_000.0);

        let runtime = CaptureRuntime::start(device, center, rate)?;

        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.device_sets.insert(
                id,
                DeviceSetState {
                    info,
                    capabilities,
                    settings,
                    status: DeviceSetStatus::Running,
                    channels: Vec::new(),
                    next_channel_id: 1,
                    error: None,
                    runtime,
                },
            );
            inner.revision += 1;
            id
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        Ok(id)
    }

    /// Close a device set and stop its threads (PLAN §5 DELETE devicesets).
    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
        // Take ownership out of the map, then drop it (joining threads) OUTSIDE the lock.
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        drop(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        Ok(())
    }

    /// Apply a device settings delta (PLAN §5 PATCH device).
    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.runtime.apply(&delta)?;
            merge_settings(&mut state.settings, &delta);
            let center = state.settings.center_hz.unwrap_or(100_000_000.0);
            let rate = state.settings.sample_rate.unwrap_or(2_048_000.0);
            state.runtime.set_meta(center, rate);
            inner.revision += 1;
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Add a channel to a device set (PLAN §5 POST channels). At M0 the channel is inert state
    /// only — no demod exists yet (PLAN §16) — but the CRUD and events are exercised.
    pub fn add_channel(&self, ds: u32, settings: ChannelSettings) -> Result<u32, EngineError> {
        let id = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let id = state.next_channel_id;
            state.next_channel_id += 1;
            state.channels.push(ChannelInfo { id, settings });
            inner.revision += 1;
            id
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(id)
    }

    /// Remove a channel from a device set (PLAN §5 DELETE channels).
    pub fn remove_channel(&self, ds: u32, ch: u32) -> Result<(), EngineError> {
        {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let before = state.channels.len();
            state.channels.retain(|c| c.id != ch);
            if state.channels.len() == before {
                return Err(EngineError::ChannelNotFound(ch, ds));
            }
            inner.revision += 1;
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Subscribe to a device set's spectrum stream (PLAN §5 SubscribeSpectrum).
    pub fn subscribe_spectrum(
        &self,
        ds: u32,
    ) -> Result<broadcast::Receiver<SpectrumSnapshot>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        Ok(state.runtime.subscribe())
    }

    fn emit(&self, event: ServerEvent) {
        let _ = self.event_tx.send(event);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Overlay the non-empty fields of `delta` onto `current` (PLAN §5: absent fields unchanged).
fn merge_settings(current: &mut DeviceSettings, delta: &DeviceSettings) {
    if delta.center_hz.is_some() {
        current.center_hz = delta.center_hz;
    }
    if delta.sample_rate.is_some() {
        current.sample_rate = delta.sample_rate;
    }
    if delta.ppm.is_some() {
        current.ppm = delta.ppm;
    }
    if delta.antenna.is_some() {
        current.antenna = delta.antenna.clone();
    }
    if !delta.gains.is_empty() {
        current.gains = delta.gains.clone();
    }
    if !delta.extra.is_empty() {
        current.extra = delta.extra.clone();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sdrmm_wire::ChannelSettings;

    use super::*;

    #[tokio::test]
    async fn probes_virtual_device() {
        let engine = Engine::new();
        assert!(
            engine
                .probe_devices()
                .iter()
                .any(|d| d.id() == "virtual:siggen")
        );
    }

    #[tokio::test]
    async fn spectrum_flows_with_a_visible_tone() {
        let engine = Engine::new();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let mut rx = engine.subscribe_spectrum(ds).unwrap();

        let snap = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("spectrum within timeout")
            .expect("snapshot");
        assert_eq!(snap.db.len(), 4096);

        let mut sorted = snap.db.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let peak = *sorted.last().unwrap();
        assert!(
            peak - median > 20.0,
            "expected tone peak above floor (peak {peak}, median {median})"
        );

        engine.remove_device_set(ds).unwrap();
        assert!(engine.snapshot().device_sets.is_empty());
    }

    #[tokio::test]
    async fn create_emits_state_changed() {
        let engine = Engine::new();
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("virtual:siggen").unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        assert!(matches!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::All
            }
        ));
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn channel_crud_updates_state() {
        let engine = Engine::new();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let ch = engine
            .add_channel(
                ds,
                ChannelSettings {
                    type_id: "nfm".into(),
                    offset_hz: 0.0,
                    params: Default::default(),
                },
            )
            .unwrap();
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
        engine.remove_channel(ds, ch).unwrap();
        assert!(engine.snapshot().device_sets[0].channels.is_empty());
        assert!(engine.remove_channel(ds, 999).is_err());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn patch_retunes_without_error() {
        let engine = Engine::new();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(88_500_000.0),
                    sample_rate: Some(2_400_000.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].settings.center_hz, Some(88_500_000.0));
        assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
        engine.remove_device_set(ds).unwrap();
    }
}
