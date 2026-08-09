//! `sdrmm-engine` — the flowgraph runtime (PLAN §2, §7). Owns the authoritative device-set
//! state, hosts each device set's capture + DSP threads, and pushes `StateChanged` events and
//! spectrum snapshots outward. The control plane (this facade) uses a mutex; the DSP plane
//! (see [`runtime`]) is lock-free and never shares mutable state with it directly.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

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
/// Soapy sits above virtual; native backends (rtlsdr/hackrf) will claim higher (PLAN §6).
#[cfg(feature = "soapy")]
const SOAPY_PRIORITY: u8 = 20;
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
    /// Control-plane mutex around the runtime: device I/O (`apply`, `stop`) happens under it,
    /// never under the engine-wide `inner` lock, so a wedged device stalls only its own set.
    /// Never hold `inner` and this lock at once — clone the `Arc` under `inner`, drop `inner`,
    /// then lock. The DSP hot path never touches this mutex.
    runtime: Arc<Mutex<CaptureRuntime>>,
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
    /// Ids reserved by `create_device_set` but not yet inserted into `device_sets`. A fault
    /// arriving in that window is stashed in `pending_faults` instead of dropped; ids are
    /// never reused, so membership is unambiguous.
    creating: HashSet<u32>,
    pending_faults: HashMap<u32, DeviceError>,
    next_ds_id: u32,
    revision: u64,
}

/// The engine. All methods are `&self`; constructors hand back an `Arc` because the fault
/// drainer (and the optional hotplug prober) hold `Weak` back-references into it.
pub struct Engine {
    registry: DeviceRegistry,
    inner: Mutex<Inner>,
    event_tx: broadcast::Sender<ServerEvent>,
    /// Cloned into each capture runtime's fatal handler; the fault drainer holds the receiver.
    fault_tx: mpsc::Sender<(u32, DeviceError)>,
}

impl Engine {
    /// Build the engine with the built-in drivers registered (virtual always; native backends
    /// join here as their milestones land, PLAN §16).
    #[must_use]
    pub fn new() -> Arc<Self> {
        let mut registry = DeviceRegistry::new();
        registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
        #[cfg(feature = "soapy")]
        registry.register(
            SOAPY_PRIORITY,
            Box::new(sdrmm_device_soapy::SoapyDriver::new()),
        );
        Self::with_registry(registry)
    }

    /// Build the engine over a caller-supplied registry, so tests can register mock drivers.
    #[must_use]
    pub fn with_registry(registry: DeviceRegistry) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        let (fault_tx, fault_rx) = mpsc::channel();
        let engine = Arc::new(Self {
            registry,
            inner: Mutex::new(Inner::default()),
            event_tx,
            fault_tx,
        });
        engine.spawn_fault_drainer(fault_rx);
        engine
    }

    /// Serialize capture-thread faults into state changes. Holds only a `Weak` so it never
    /// keeps a dropped engine alive; it exits once every fault sender is gone (engine dropped,
    /// all capture threads joined).
    fn spawn_fault_drainer(self: &Arc<Self>, fault_rx: mpsc::Receiver<(u32, DeviceError)>) {
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-fault".to_string())
            .spawn(move || {
                while let Ok((ds, err)) = fault_rx.recv() {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.mark_device_fault(ds, err);
                }
            });
        if let Err(e) = spawned {
            // Without the drainer, device faults would be lost; say so loudly.
            tracing::error!("failed to spawn fault drainer: {e}");
        }
    }

    /// A capture thread died (PLAN §16 M1 hotplug robustness): keep the set listed but flag it
    /// so clients render the failure. Joins nothing — `remove_device_set` may concurrently be
    /// joining the very capture thread that raised this fault, and it needs the lock released
    /// (its take-out-then-drop pattern) while we only mutate under it.
    fn mark_device_fault(&self, ds: u32, err: DeviceError) {
        let mut inner = self.lock();
        if let Some(state) = inner.device_sets.get_mut(&ds) {
            state.status = DeviceSetStatus::Error;
            state.error = Some(err.to_string());
            inner.revision += 1;
            drop(inner);
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        } else if inner.creating.contains(&ds) {
            // The capture died before `create_device_set` could insert the set; stash the
            // fault so the insert applies it instead of leaving a dead set Running.
            inner.pending_faults.insert(ds, err);
        } else {
            drop(inner);
            // The fault raced a removal; the set is already gone, so log instead of dropping
            // the error on the floor.
            tracing::warn!(ds, error = %err, "fault for removed device set");
        }
    }

    /// Poll for attach/detach every `interval` (PLAN §16 M1): a changed probe result pushes
    /// `StateChanged{Devices}` so clients refetch `GET /api/devices`. Opt-in per binary — unit
    /// tests must never race a background prober. Holds a `Weak`; the thread exits at the
    /// first wake after the engine is dropped.
    pub fn start_hotplug_prober(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-hotplug".to_string())
            .spawn(move || {
                let mut known = None;
                let mut missing_once = HashSet::new();
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.hotplug_tick(&mut known, &mut missing_once);
                    drop(engine);
                    std::thread::sleep(interval);
                }
            })?;
        Ok(())
    }

    /// One prober step, split from the thread so tests drive it without sleeping: probe, diff
    /// against `known`, emit on change. The first probe is the baseline and never emits.
    /// `probe_all` output is id-sorted, so plain `Vec` equality is order-insensitive.
    ///
    /// The probe also guards running sets against silent unplug: Soapy backends cannot detect
    /// it from inside the capture thread (their hardware-key reads do no live I/O), so a
    /// running set whose device is absent from the probe on two consecutive ticks — one miss
    /// may be a transient enumerate hiccup — is faulted via `mark_device_fault`.
    /// `missing_once` carries the single-miss ids between ticks.
    fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
    ) -> bool {
        let ids: Vec<String> = self
            .registry
            .probe_all()
            .iter()
            .map(DeviceInfo::id)
            .collect();

        let absent: HashSet<u32> = {
            let inner = self.lock();
            inner
                .device_sets
                .iter()
                .filter(|(_, s)| {
                    s.status == DeviceSetStatus::Running && !ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for ds in absent.intersection(missing_once) {
            self.mark_device_fault(
                *ds,
                DeviceError::Io("device disappeared from probe".to_string()),
            );
        }
        *missing_once = absent;

        let changed = known.as_ref().is_some_and(|prev| *prev != ids);
        *known = Some(ids);
        if changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Devices,
            });
        }
        changed
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
        let (info, device) = self.registry.open(device_id)?;
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();
        let center = settings.center_hz.unwrap_or(100_000_000.0);
        let rate = settings.sample_rate.unwrap_or(2_048_000.0);

        // Reserve the id before the runtime exists: the fatal handler must name its device
        // set, and `creating` membership lets `mark_device_fault` stash a fault raised before
        // the insert below instead of dropping it.
        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.creating.insert(id);
            id
        };
        let fault_tx = self.fault_tx.clone();
        let started = CaptureRuntime::start(device, center, rate, move |err| {
            // Unbounded send: the dying capture thread never blocks on the control plane.
            let _ = fault_tx.send((id, err));
        });
        let runtime = match started {
            Ok(runtime) => runtime,
            Err(e) => {
                let mut inner = self.lock();
                inner.creating.remove(&id);
                inner.pending_faults.remove(&id);
                return Err(e.into());
            }
        };

        let faulted = {
            let mut inner = self.lock();
            inner.creating.remove(&id);
            let pending = inner.pending_faults.remove(&id);
            inner.device_sets.insert(
                id,
                DeviceSetState {
                    info,
                    capabilities,
                    settings,
                    status: if pending.is_some() {
                        DeviceSetStatus::Error
                    } else {
                        DeviceSetStatus::Running
                    },
                    channels: Vec::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    runtime: Arc::new(Mutex::new(runtime)),
                },
            );
            inner.revision += 1;
            pending.is_some()
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if faulted {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id),
            });
        }
        Ok(id)
    }

    /// Close a device set and stop its threads (PLAN §5 DELETE devicesets).
    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
        // Take ownership out of the map, then stop (joining threads) OUTSIDE the engine lock;
        // the per-set lock lets a concurrent `patch_device` finish its device I/O first.
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        lock_runtime(&removed.runtime).stop();
        drop(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        Ok(())
    }

    /// Apply a device settings delta (PLAN §5 PATCH device). The device I/O runs under the
    /// per-set lock only; `inner` is re-taken afterwards to merge, so a wedged device never
    /// blocks `snapshot` or other sets.
    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        let runtime = {
            let inner = self.lock();
            inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?
                .runtime
                .clone()
        };
        lock_runtime(&runtime).apply(&delta)?;
        let (center, rate) = {
            let mut inner = self.lock();
            // The set may have been removed while `apply` ran; its runtime was stopped by
            // `remove_device_set`, so just report the removal.
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.settings.merge_from(&delta);
            let center = state.settings.center_hz.unwrap_or(100_000_000.0);
            let rate = state.settings.sample_rate.unwrap_or(2_048_000.0);
            inner.revision += 1;
            (center, rate)
        };
        lock_runtime(&runtime).set_meta(center, rate);
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
        let runtime = {
            let inner = self.lock();
            inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?
                .runtime
                .clone()
        };
        let subscription = lock_runtime(&runtime).subscribe();
        Ok(subscription)
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

fn lock_runtime(runtime: &Mutex<CaptureRuntime>) -> std::sync::MutexGuard<'_, CaptureRuntime> {
    runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    use num_complex::Complex;
    use sdrmm_device::{DeviceDriver, DeviceRegistry, RxSink, SdrDevice};
    use sdrmm_wire::ChannelSettings;

    use super::*;

    fn mock_info(key: &str, serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            driver: "mock".to_string(),
            key: key.to_string(),
            label: format!("Mock {key}"),
            serial: serial.map(str::to_string),
        }
    }

    fn empty_capabilities() -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            tx_capable: false,
        }
    }

    /// Driver whose device streams a few blocks and then dies with an I/O error.
    struct DyingDriver;

    impl DeviceDriver for DyingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("dying", Some("MOCK-1"))]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(DyingDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
                worker: None,
            }))
        }
    }

    struct DyingDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        worker: Option<JoinHandle<()>>,
    }

    impl SdrDevice for DyingDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, mut sink: RxSink) -> Result<(), DeviceError> {
            self.worker = Some(std::thread::spawn(move || {
                let block = [Complex::new(0.0f32, 0.0); 256];
                for _ in 0..3 {
                    sink.push(&block);
                }
                sink.fail(DeviceError::Io("mock stream died".to_string()));
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Driver whose device reports a fatal error synchronously inside `rx_start`, so the fault
    /// is on the drainer's queue before `create_device_set` can insert the set — the
    /// stash-then-apply window made deterministic.
    struct InstantFailDriver;

    impl DeviceDriver for InstantFailDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("instafail", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(InstantFailDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    struct InstantFailDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for InstantFailDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, mut sink: RxSink) -> Result<(), DeviceError> {
            sink.fail(DeviceError::Io("died at start".to_string()));
            Ok(())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose probe result can be emptied mid-test, simulating an unplug the capture
    /// thread never notices (the Soapy case).
    struct VanishingDriver {
        present: Arc<AtomicBool>,
    }

    impl DeviceDriver for VanishingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            if self.present.load(Ordering::SeqCst) {
                vec![mock_info("vanish", None)]
            } else {
                Vec::new()
            }
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(SilentDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    /// Device that streams nothing and never raises a fault on its own.
    struct SilentDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for SilentDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, _sink: RxSink) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose probe result grows after the first call, simulating an attach.
    struct FlappingDriver {
        probes: AtomicUsize,
    }

    impl DeviceDriver for FlappingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            let n = self.probes.fetch_add(1, Ordering::SeqCst);
            let mut out = vec![mock_info("a", None)];
            if n >= 1 {
                out.push(mock_info("b", None));
            }
            out
        }

        fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Err(DeviceError::NotFound(info.id()))
        }
    }

    async fn wait_for_deviceset_event(events: &mut broadcast::Receiver<ServerEvent>, ds: u32) {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
                .await
                .expect("event within timeout")
                .expect("event");
            if matches!(
                ev,
                ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(id)
                } if id == ds
            ) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn device_fault_surfaces_and_removal_completes() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(DyingDriver));
        let engine = Engine::with_registry(registry);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:dying").unwrap();

        wait_for_deviceset_event(&mut events, ds).await;

        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
        assert!(
            snap.device_sets[0]
                .error
                .as_deref()
                .unwrap()
                .contains("mock stream died"),
            "fault message must surface: {:?}",
            snap.device_sets[0].error
        );
        // registry.open must have carried the probed info through, not a synthesized one.
        assert_eq!(snap.device_sets[0].device.label, "Mock dying");
        assert_eq!(snap.device_sets[0].device.serial.as_deref(), Some("MOCK-1"));

        let removal = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || engine.remove_device_set(ds))
        };
        tokio::time::timeout(Duration::from_secs(5), removal)
            .await
            .expect("removal must not hang on a dead capture thread")
            .expect("join")
            .expect("remove ok");
    }

    #[tokio::test]
    async fn fault_raised_before_insert_still_surfaces() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(InstantFailDriver));
        let engine = Engine::with_registry(registry);
        let ds = engine.create_device_set("mock:instafail").unwrap();

        // The fault was sent before the insert; whether the drainer processed it before the
        // insert (stashed in pending_faults) or after (marked directly), the set must converge
        // to Error instead of staying Running forever.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snap = engine.snapshot();
            let set = snap
                .device_sets
                .iter()
                .find(|s| s.id == ds)
                .expect("faulted set must stay listed");
            if set.status == DeviceSetStatus::Error {
                assert!(
                    set.error
                        .as_deref()
                        .expect("error message")
                        .contains("died at start"),
                    "fault message must surface: {:?}",
                    set.error
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "set stuck in {:?} without surfacing the fault",
                set.status
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn probe_disappearance_faults_running_set_after_two_misses() {
        let present = Arc::new(AtomicBool::new(true));
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(VanishingDriver {
                present: present.clone(),
            }),
        );
        let engine = Engine::with_registry(registry);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:vanish").unwrap();

        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert_eq!(
            engine.snapshot().device_sets[0].status,
            DeviceSetStatus::Running,
            "present device must not be faulted"
        );

        present.store(false, Ordering::SeqCst);
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert_eq!(
            engine.snapshot().device_sets[0].status,
            DeviceSetStatus::Running,
            "one missed probe may be a transient enumerate hiccup"
        );

        engine.hotplug_tick(&mut known, &mut missing_once);
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
        assert!(
            snap.device_sets[0]
                .error
                .as_deref()
                .unwrap()
                .contains("disappeared from probe"),
            "unplug reason must surface: {:?}",
            snap.device_sets[0].error
        );
        wait_for_deviceset_event(&mut events, ds).await;
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn hotplug_tick_emits_only_on_probe_change() {
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(FlappingDriver {
                probes: AtomicUsize::new(0),
            }),
        );
        let engine = Engine::with_registry(registry);
        let mut events = engine.subscribe_events();

        let mut known = None;
        let mut missing_once = HashSet::new();
        assert!(
            !engine.hotplug_tick(&mut known, &mut missing_once),
            "first probe is baseline"
        );
        assert!(
            engine.hotplug_tick(&mut known, &mut missing_once),
            "attach must be detected"
        );

        let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        assert!(matches!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::Devices
            }
        ));

        assert!(
            !engine.hotplug_tick(&mut known, &mut missing_once),
            "steady state stays quiet"
        );
    }

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
