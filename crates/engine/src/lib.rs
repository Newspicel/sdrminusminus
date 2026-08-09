//! `sdrmm-engine` — the flowgraph runtime (PLAN §2, §7). Owns the authoritative device-set
//! state, hosts each device set's capture + DSP threads plus per-channel Opus encoders, and
//! pushes `StateChanged` events, spectrum snapshots, and audio packets outward. The control
//! plane (this facade) uses a mutex; the DSP plane (see [`runtime`]) is lock-free and never
//! shares mutable state with it directly — channel changes cross over via a command queue.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use sdrmm_channels::{ChannelCtx, ChannelError};
use sdrmm_device::{DeviceError, DeviceRegistry};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_wire::{
    Capabilities, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings, DeviceInfo,
    DeviceSet, DeviceSetStatus, DeviceSettings, ServerEvent, StateScope, StateSnapshot,
};
use tokio::sync::broadcast;

pub mod audio;
pub mod runtime;
pub use audio::AudioPacket;
pub use runtime::{SpectrumSnapshot, adaptive_db_window};

use crate::{
    audio::PcmBlock,
    runtime::{CaptureRuntime, ChannelHost, DspCommand},
};

/// Merge priority for the built-in virtual driver (native backends register higher, PLAN §6).
const VIRTUAL_PRIORITY: u8 = 10;
/// Soapy sits above virtual; native backends (rtlsdr/hackrf) will claim higher (PLAN §6).
#[cfg(feature = "soapy")]
const SOAPY_PRIORITY: u8 = 20;
const EVENT_CHANNEL_CAP: usize = 256;
/// Fallbacks for devices that report no tuning/rate; mirrored wherever settings are read.
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE: f64 = 2_048_000.0;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("device set {0} not found")]
    DeviceSetNotFound(u32),
    #[error("channel {0} not found in device set {1}")]
    ChannelNotFound(u32, u32),
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("audio pipeline: {0}")]
    Audio(String),
}

impl EngineError {
    /// Maps to HTTP 404 (missing device set / channel / device).
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::DeviceSetNotFound(_) | Self::ChannelNotFound(..))
            || matches!(self, Self::Device(DeviceError::NotFound(_)))
    }

    /// Maps to HTTP 400 (a well-formed request the device or channel validation rejected).
    #[must_use]
    pub fn is_bad_request(&self) -> bool {
        matches!(
            self,
            Self::Device(DeviceError::Unsupported(_) | DeviceError::AlreadyStreaming)
                | Self::Channel(_)
        )
    }
}

/// A hosted channel queued for a pipeline rebuild swap: the settings snapshot it was listed
/// under, plus the audio identity (PCM sender + shared sample position) the replacement host
/// must reuse so the audio stream and its timestamps survive the swap.
struct RebuildEntry {
    id: u32,
    settings: ChannelSettings,
    pcm_tx: broadcast::Sender<PcmBlock>,
    pcm_pos: Arc<AtomicU64>,
}

fn sample_rate_of(settings: &DeviceSettings) -> f64 {
    settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE)
}

fn descriptor_for(params: &ChannelParams) -> Result<ChannelDescriptor, EngineError> {
    let type_id = params.type_id();
    sdrmm_channels::descriptors()
        .into_iter()
        .find(|d| d.type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()).into())
}

/// A channel's occupied band must fit inside the device passband and its DDC must decimate
/// (never interpolate); non-finite knobs are rejected before they can reach the DSP plane.
/// The band comes from the configured params ([`sdrmm_channels::occupied_band`]), not the
/// descriptor nominal — the nominal would pass configs (wide NFM, SSB's one-sided band)
/// whose real occupancy sticks out past Nyquist and silently truncates the audio.
fn validate_channel(
    descriptor: &ChannelDescriptor,
    settings: &ChannelSettings,
    device_rate: f64,
) -> Result<(), EngineError> {
    if !settings.offset_hz.is_finite() {
        return Err(ChannelError::InvalidSettings(format!(
            "offset_hz must be finite, got {}",
            settings.offset_hz
        ))
        .into());
    }
    if let Some(db) = settings.squelch_db
        && !db.is_finite()
    {
        return Err(
            ChannelError::InvalidSettings(format!("squelch_db must be finite, got {db}")).into(),
        );
    }
    if device_rate < descriptor.input_rate_hz {
        return Err(ChannelError::InvalidSettings(format!(
            "{} needs a device rate of at least {} Hz, device runs at {device_rate} Hz",
            descriptor.type_id, descriptor.input_rate_hz
        ))
        .into());
    }
    let (low, high) = sdrmm_channels::occupied_band(&settings.params);
    let band_low = settings.offset_hz + low;
    let band_high = settings.offset_hz + high;
    let nyquist = device_rate / 2.0;
    if band_low < -nyquist || band_high > nyquist {
        return Err(ChannelError::InvalidSettings(format!(
            "channel band [{band_low}, {band_high}] Hz exceeds the ±{nyquist} Hz device passband"
        ))
        .into());
    }
    Ok(())
}

/// A channel's audio identity: the PCM fan-in and Opus fan-out survive pipeline rebuilds
/// (params type change, device rate change), so audio subscribers never notice a swap.
struct ChannelAudio {
    pcm_tx: broadcast::Sender<PcmBlock>,
    /// 48 kHz-domain position of the channel's next PCM sample, shared into every host
    /// built for this channel so packet timestamps stay continuous across rebuilds.
    pcm_pos: Arc<AtomicU64>,
    audio_tx: broadcast::Sender<AudioPacket>,
    encoder: Option<std::thread::JoinHandle<()>>,
}

impl ChannelAudio {
    fn new() -> Result<Self, EngineError> {
        let (pcm_tx, pcm_rx) = broadcast::channel(audio::PCM_CHANNEL_CAP);
        let (audio_tx, _) = broadcast::channel(audio::AUDIO_CHANNEL_CAP);
        let encoder = audio::spawn_encoder(pcm_rx, audio_tx.clone())?;
        Ok(Self {
            pcm_tx,
            pcm_pos: Arc::new(AtomicU64::new(0)),
            audio_tx,
            encoder: Some(encoder),
        })
    }

    /// Joins the encoder thread. The caller must already have arranged for the DSP-side PCM
    /// sender clone to drop (`RemoveChannel` sent, or the runtime stopped) or this blocks.
    fn shutdown(mut self) {
        let encoder = self.encoder.take();
        drop(self);
        if let Some(handle) = encoder
            && handle.join().is_err()
        {
            tracing::error!("opus encoder thread panicked");
        }
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
    /// Audio plumbing per channel id, kept beside the projected `channels` so rebuild swaps
    /// can preserve a channel's streams while replacing its DSP pipeline.
    audio: HashMap<u32, ChannelAudio>,
    next_channel_id: u32,
    error: Option<String>,
    /// Clone of the runtime's DSP command queue. Channel commands go through this while
    /// holding the engine `inner` lock with this set's entry present — that ordering is what
    /// keeps DSP-plane channel membership consistent with control-plane state (a removal or
    /// swap can never interleave into a stale rebuild and re-add a deleted channel, which
    /// would strand a live PCM sender and hang the encoder join). `mpsc` sends never block,
    /// so sending under `inner` is safe; the never-hold-both rule below concerns only the
    /// `runtime` mutex, which these sends never touch.
    cmd_tx: mpsc::Sender<DspCommand>,
    /// Capture-ring drop counter shared with the runtime; readable without its lock so
    /// snapshots never wait on a wedged device.
    overruns: Arc<AtomicU64>,
    /// Overrun count already surfaced to clients; the hotplug tick diffs against it.
    overruns_seen: u64,
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
            overruns: self.overruns.load(Ordering::Relaxed),
            error: self.error.clone(),
        }
    }

    /// Queue a DSP command. Callers hold `inner` with this set still listed, so the DSP
    /// thread is alive (`remove_device_set` unlists the set before stopping it); a closed
    /// queue here is an engine bug and is surfaced rather than swallowed.
    fn send_dsp(&self, cmd: DspCommand) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::error!("dsp command queue closed while its device set is still listed");
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
    ///
    /// Capture-ring overrun growth surfaces here too, rather than from the DSP thread: the
    /// prober cadence rate-limits both the warn and the `StateChanged` fan-out to one per
    /// set per tick, and clients then read the cumulative count from `DeviceSet.overruns`.
    fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
    ) -> bool {
        let grown: Vec<(u32, u64)> = {
            let mut inner = self.lock();
            let grown: Vec<(u32, u64)> = inner
                .device_sets
                .iter_mut()
                .filter_map(|(id, s)| {
                    let now = s.overruns.load(Ordering::Relaxed);
                    let delta = now - s.overruns_seen;
                    s.overruns_seen = now;
                    (delta > 0).then_some((*id, delta))
                })
                .collect();
            if !grown.is_empty() {
                inner.revision += 1;
            }
            grown
        };
        for (ds, dropped) in grown {
            tracing::warn!(ds, dropped, "capture ring overrun: device samples dropped");
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }

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

    /// Emit a `StateChanged` for state the engine does not own (presets, bookmarks): the WS
    /// hub forwards engine events only, so server-side stores invalidate through here.
    pub fn emit_scope(&self, scope: StateScope) {
        self.emit(ServerEvent::StateChanged { scope });
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
        let center = settings.center_hz.unwrap_or(DEFAULT_CENTER_HZ);
        let rate = sample_rate_of(&settings);

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

        let cmd_tx = runtime.command_sender();
        let overruns = runtime.overruns_counter();
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
                    audio: HashMap::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    cmd_tx,
                    overruns,
                    overruns_seen: 0,
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
        let mut removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        // Stopping joins the DSP thread, dropping every hosted channel and with it the last
        // DSP-side PCM sender — only then can the encoder joins below complete.
        lock_runtime(&removed.runtime).stop();
        for (_, handle) in removed.audio.drain() {
            handle.shutdown();
        }
        drop(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        Ok(())
    }

    /// Apply a device settings delta (PLAN §5 PATCH device). The device I/O runs under the
    /// per-set lock only; `inner` is re-taken afterwards to merge, so a wedged device never
    /// blocks `snapshot` or other sets. A sample-rate change rebuilds every hosted channel
    /// pipeline at the new rate (ids and audio streams preserved); center-frequency changes
    /// need nothing — channel offsets are center-relative.
    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        let runtime = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            // Refuse a rate the hosted channels cannot run at, before any device I/O —
            // rejecting up front beats stranding a channel after the device already retuned.
            if let Some(new_rate) = delta.sample_rate
                && new_rate != sample_rate_of(&state.settings)
            {
                for channel in &state.channels {
                    let descriptor = descriptor_for(&channel.settings.params)?;
                    validate_channel(&descriptor, &channel.settings, new_rate)?;
                }
            }
            state.runtime.clone()
        };
        lock_runtime(&runtime).apply(&delta)?;
        let (center, rate, rebuilds) = {
            let mut inner = self.lock();
            // The set may have been removed while `apply` ran; its runtime was stopped by
            // `remove_device_set`, so just report the removal.
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let old_rate = sample_rate_of(&state.settings);
            state.settings.merge_from(&delta);
            let center = state.settings.center_hz.unwrap_or(DEFAULT_CENTER_HZ);
            let rate = sample_rate_of(&state.settings);
            let rebuilds: Vec<RebuildEntry> = if rate == old_rate {
                Vec::new()
            } else {
                state
                    .channels
                    .iter()
                    .filter_map(|c| {
                        state.audio.get(&c.id).map(|a| RebuildEntry {
                            id: c.id,
                            settings: c.settings.clone(),
                            pcm_tx: a.pcm_tx.clone(),
                            pcm_pos: a.pcm_pos.clone(),
                        })
                    })
                    .collect()
            };
            inner.revision += 1;
            (center, rate, rebuilds)
        };
        lock_runtime(&runtime).set_meta(center, rate);
        let mut dead: Vec<ChannelAudio> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        // Encoder joins happen outside `inner`; the RemoveChannel each dead entry already
        // queued guarantees the DSP-side PCM sender drops, so these cannot hang.
        for handle in dead {
            handle.shutdown();
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Swap one hosted channel onto the post-patch device rate. The host is built outside
    /// any lock; `inner` is then re-taken to verify the channel still exists with exactly
    /// the settings (and rate) the host was built for — retrying against the fresher state
    /// otherwise — and the Remove+Add pair is queued under `inner`. A concurrently removed
    /// channel is skipped: the unused host just drops (taking its PCM sender clone with it),
    /// so the removal's encoder join stays sound. A channel that no longer validates at the
    /// current rate is dropped with a loud error; its audio teardown lands in `dead` for the
    /// caller to join outside the lock.
    fn rebuild_channel(
        &self,
        ds: u32,
        rebuild: RebuildEntry,
        rate: f64,
        dead: &mut Vec<ChannelAudio>,
    ) {
        let RebuildEntry {
            id,
            mut settings,
            pcm_tx,
            pcm_pos,
        } = rebuild;
        let mut built_rate = rate;
        loop {
            let built = descriptor_for(&settings.params)
                .and_then(|d| validate_channel(&d, &settings, built_rate))
                .and_then(|()| {
                    ChannelHost::build(built_rate, &settings, pcm_tx.clone(), pcm_pos.clone())
                        .map_err(EngineError::from)
                });
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                // The set was removed while building; that removal owns all teardown.
                return;
            };
            let current_rate = sample_rate_of(&state.settings);
            let Some(info) = state.channels.iter().find(|c| c.id == id) else {
                // The channel was removed while building — nothing to swap.
                return;
            };
            if current_rate != built_rate || info.settings != settings {
                // The snapshot went stale (another patch landed); rebuild against it.
                settings = info.settings.clone();
                built_rate = current_rate;
                continue;
            }
            match built {
                Ok(host) => {
                    state.send_dsp(DspCommand::RemoveChannel { id });
                    state.send_dsp(DspCommand::AddChannel { id, host });
                }
                Err(e) => {
                    // The rate was pre-validated against every channel before the device
                    // I/O, so only a racing settings change lands here; drop the channel
                    // rather than leave a stale-rate pipeline running.
                    tracing::error!(ds, channel = id, error = %e, "channel rebuild failed after rate change; removing channel");
                    state.channels.retain(|c| c.id != id);
                    dead.extend(state.audio.remove(&id));
                    state.send_dsp(DspCommand::RemoveChannel { id });
                    inner.revision += 1;
                }
            }
            return;
        }
    }

    /// Add a channel to a device set (PLAN §5 POST channels): validate and build the whole
    /// DDC → demod pipeline control-side, then hand it to the DSP thread via the command
    /// queue. Construction failures surface here as bad requests.
    pub fn add_channel(&self, ds: u32, settings: ChannelSettings) -> Result<u32, EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        let mut device_rate = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            sample_rate_of(&state.settings)
        };
        let created = ChannelAudio::new()?;
        let pcm_tx = created.pcm_tx.clone();
        let pcm_pos = created.pcm_pos.clone();
        let mut audio = Some(created);

        // A rate patch racing between build and insert would leave a wrong-rate DDC, so the
        // rate is re-checked under the lock and the pipeline rebuilt if it moved. The
        // AddChannel goes out in the same critical section as the state commit, so no
        // concurrent removal or swap can interleave between them.
        let staged = loop {
            let built = validate_channel(&descriptor, &settings, device_rate).and_then(|()| {
                ChannelHost::build(device_rate, &settings, pcm_tx.clone(), pcm_pos.clone())
                    .map_err(EngineError::from)
            });
            let host = match built {
                Ok(host) => host,
                Err(e) => break Err(e),
            };
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                break Err(EngineError::DeviceSetNotFound(ds));
            };
            let current_rate = sample_rate_of(&state.settings);
            if current_rate != device_rate {
                device_rate = current_rate;
                continue;
            }
            let id = state.next_channel_id;
            state.next_channel_id += 1;
            state.channels.push(ChannelInfo {
                id,
                settings: settings.clone(),
            });
            if let Some(handle) = audio.take() {
                state.audio.insert(id, handle);
            }
            state.send_dsp(DspCommand::AddChannel { id, host });
            inner.revision += 1;
            break Ok(id);
        };
        let id = match staged {
            Ok(id) => id,
            Err(e) => {
                // The local sender clone must go first or the encoder join would wait on it.
                drop(pcm_tx);
                if let Some(handle) = audio.take() {
                    handle.shutdown();
                }
                return Err(e);
            }
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(id)
    }

    /// Apply a channel settings delta (PLAN §5 PATCH channels). Retunes and param tweaks
    /// reach the live pipeline as commands (no rebuild); a params *type* change swaps in a
    /// freshly built pipeline while the channel keeps its id and audio stream. Commands are
    /// queued under `inner` in the same critical section as the state commit, so what the
    /// DSP thread hosts always converges to the last-committed settings.
    pub fn patch_channel(
        &self,
        ds: u32,
        ch: u32,
        settings: ChannelSettings,
    ) -> Result<(), EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        let (old, pcm_tx, pcm_pos, mut device_rate) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let info = state
                .channels
                .iter()
                .find(|c| c.id == ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            let handle = state
                .audio
                .get(&ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            (
                info.settings.clone(),
                handle.pcm_tx.clone(),
                handle.pcm_pos.clone(),
                sample_rate_of(&state.settings),
            )
        };
        let mut need_host = old.params.type_id() != settings.params.type_id();
        if !need_host {
            // Construct-and-discard so invalid params surface here as a bad request instead
            // of failing later on the DSP thread.
            drop(sdrmm_channels::create(
                ChannelCtx {
                    input_rate: descriptor.input_rate_hz,
                },
                &settings,
            )?);
        }
        let staged = loop {
            if let Err(e) = validate_channel(&descriptor, &settings, device_rate) {
                break Err(e);
            }
            let host = if need_host {
                match ChannelHost::build(device_rate, &settings, pcm_tx.clone(), pcm_pos.clone()) {
                    Ok(host) => Some(host),
                    Err(e) => break Err(e.into()),
                }
            } else {
                None
            };
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                break Err(EngineError::DeviceSetNotFound(ds));
            };
            let current_rate = sample_rate_of(&state.settings);
            if current_rate != device_rate {
                device_rate = current_rate;
                continue;
            }
            let Some(info) = state.channels.iter_mut().find(|c| c.id == ch) else {
                break Err(EngineError::ChannelNotFound(ch, ds));
            };
            // Diff against what is committed *now*, not the entry snapshot: a concurrent
            // patch may have landed in between, and skipping the command then would leave
            // the DSP pipeline on its settings while state shows ours.
            if info.settings.params.type_id() != settings.params.type_id() && host.is_none() {
                need_host = true;
                continue;
            }
            let prev = std::mem::replace(&mut info.settings, settings.clone());
            match host {
                Some(host) => {
                    state.send_dsp(DspCommand::RemoveChannel { id: ch });
                    state.send_dsp(DspCommand::AddChannel { id: ch, host });
                }
                None => {
                    if prev.offset_hz != settings.offset_hz {
                        state.send_dsp(DspCommand::Retune {
                            id: ch,
                            offset_hz: settings.offset_hz,
                        });
                    }
                    if prev.params != settings.params || prev.squelch_db != settings.squelch_db {
                        state.send_dsp(DspCommand::ApplySettings {
                            id: ch,
                            settings: settings.clone(),
                        });
                    }
                }
            }
            inner.revision += 1;
            break Ok(());
        };
        staged?;
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Remove a channel from a device set (PLAN §5 DELETE channels), tearing down its DSP
    /// pipeline and joining its encoder thread.
    pub fn remove_channel(&self, ds: u32, ch: u32) -> Result<(), EngineError> {
        let handle = {
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
            let handle = state.audio.remove(&ch);
            // Queued under `inner` in the same critical section as the state removal: every
            // rebuild swap re-checks membership under `inner` before queueing, so nothing
            // can re-add the host after this — the DSP-side PCM sender is guaranteed to
            // drop and the encoder join below cannot hang.
            state.send_dsp(DspCommand::RemoveChannel { id: ch });
            inner.revision += 1;
            handle
        };
        if let Some(handle) = handle {
            handle.shutdown();
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Subscribe to a channel's Opus packet stream (PLAN §5 SubscribeAudio).
    pub fn subscribe_audio(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<AudioPacket>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .audio
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.audio_tx.subscribe())
    }

    /// Every compiled-in channel type (PLAN §5 GET channel types). The registry lives in
    /// `sdrmm-channels`; the server reaches it through here and never depends on it directly.
    #[must_use]
    pub fn channel_types(&self) -> Vec<ChannelDescriptor> {
        sdrmm_channels::descriptors()
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
    use sdrmm_wire::{ChannelSettings, NfmParams, Sideband, SsbParams};

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

    /// Driver whose device floods the capture ring in a single oversized push before the
    /// DSP thread can drain, guaranteeing a deterministic overrun count.
    struct FloodingDriver;

    impl DeviceDriver for FloodingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("flood", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(FloodingDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    struct FloodingDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for FloodingDevice {
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
            // 2× the ring in one push: at most RING_CAPACITY fits, the rest must be counted.
            let block = vec![Complex::new(0.0f32, 0.0); crate::runtime::RING_CAPACITY * 2];
            sink.push(&block);
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

    /// Hermetic engine: virtual driver only. `Engine::new()` registers the Soapy driver, whose
    /// probe enumerates live system modules — forbidden in tests (PLAN §14: no hardware in CI).
    fn virtual_engine() -> Arc<Engine> {
        let mut registry = DeviceRegistry::new();
        registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
        Engine::with_registry(registry)
    }

    #[tokio::test]
    async fn probes_virtual_device() {
        let engine = virtual_engine();
        assert!(
            engine
                .probe_devices()
                .iter()
                .any(|d| d.id() == "virtual:siggen")
        );
    }

    #[tokio::test]
    async fn spectrum_flows_with_a_visible_tone() {
        let engine = virtual_engine();
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
        let engine = virtual_engine();
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

    fn nfm_settings(offset_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
        }
    }

    #[tokio::test]
    async fn channel_crud_updates_state() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let ch = engine.add_channel(ds, nfm_settings(0.0)).unwrap();
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
        engine.remove_channel(ds, ch).unwrap();
        assert!(engine.snapshot().device_sets[0].channels.is_empty());
        assert!(engine.remove_channel(ds, 999).is_err());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn add_channel_rejects_out_of_passband_offset() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        // Default rate 2.048 Msps → ±1.024 MHz passband.
        let err = engine
            .add_channel(ds, nfm_settings(1_100_000.0))
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(engine.snapshot().device_sets[0].channels.is_empty());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn patch_channel_rejects_missing_channel() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let err = engine.patch_channel(ds, 7, nfm_settings(0.0)).unwrap_err();
        assert!(err.is_not_found(), "expected not found, got {err}");
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn rate_change_stranding_a_channel_is_rejected_before_device_io() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.add_channel(ds, nfm_settings(900_000.0)).unwrap();
        // At 250 ksps the ±125 kHz passband cannot contain a channel at +900 kHz.
        let err = engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(250_000.0),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        // The rejected patch must not have reached the device.
        assert_eq!(
            engine.snapshot().device_sets[0].settings.sample_rate,
            Some(2_048_000.0)
        );
        engine.remove_device_set(ds).unwrap();
    }

    /// The engine used to send a rate-change rebuild's Remove+Add from a stale snapshot
    /// outside `inner`: a concurrent DELETE could interleave, its channel got re-added on
    /// the DSP thread as a zombie holding a live PCM sender, and the DELETE's encoder join
    /// hung forever. Commands now go out under `inner` with membership re-checked, so this
    /// loop must never wedge.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rate_rebuild_and_remove_never_strands_a_channel() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        for i in 0..40u32 {
            let ch = engine.add_channel(ds, nfm_settings(100_000.0)).unwrap();
            let rate = if i % 2 == 0 { 2_400_000.0 } else { 2_048_000.0 };
            let patch = {
                let engine = engine.clone();
                tokio::task::spawn_blocking(move || {
                    engine.patch_device(
                        ds,
                        DeviceSettings {
                            sample_rate: Some(rate),
                            ..Default::default()
                        },
                    )
                })
            };
            let remove = {
                let engine = engine.clone();
                tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch))
            };
            let (patch, remove) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(patch, remove)
            })
            .await
            .unwrap_or_else(|_| panic!("iteration {i}: patch_device/remove_channel deadlocked"));
            patch.expect("join").expect("patch ok");
            remove.expect("join").expect("remove ok");
            assert!(
                engine.snapshot().device_sets[0].channels.is_empty(),
                "iteration {i}: channel survived its removal"
            );
        }
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn ring_overrun_surfaces_in_state_and_emits_event() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(FloodingDriver));
        let engine = Engine::with_registry(registry);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:flood").unwrap();

        let snap = engine.snapshot();
        assert!(
            snap.device_sets[0].overruns >= crate::runtime::RING_CAPACITY as u64,
            "flooded ring must report drops, got {}",
            snap.device_sets[0].overruns
        );

        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        wait_for_deviceset_event(&mut events, ds).await;

        // No further growth: the next tick must stay quiet instead of re-announcing.
        let mut quiet = engine.subscribe_events();
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert!(
            matches!(quiet.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "tick without overrun growth must not emit"
        );
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn validate_honors_configured_bandwidth_and_sideband() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(250_000.0),
                    ..Default::default()
                },
            )
            .unwrap();

        let usb = |offset_hz: f64| ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 10_000.0,
                agc: true,
            }),
        };
        let wide_nfm = |offset_hz: f64| ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams {
                bandwidth_hz: 25_000.0,
            }),
        };

        // USB at +120 kHz occupies +120.1…+130 kHz — past the +125 kHz Nyquist edge even
        // though the descriptor-nominal ±1.5 kHz check would pass it.
        let err = engine.add_channel(ds, usb(120_000.0)).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        // A 25 kHz NFM at +118 kHz reaches +130.5 kHz.
        let err = engine.add_channel(ds, wide_nfm(118_000.0)).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(engine.snapshot().device_sets[0].channels.is_empty());

        // The same configs fit once their occupied band stays inside the passband — the
        // check must not become a blunt nominal-width rejection.
        engine.add_channel(ds, usb(-124_000.0)).unwrap();
        engine.add_channel(ds, wide_nfm(112_000.0)).unwrap();
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn patch_retunes_without_error() {
        let engine = virtual_engine();
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
