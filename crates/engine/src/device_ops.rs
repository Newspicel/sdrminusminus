use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use sdrmm_device::DeviceError;
use sdrmm_wire::{
    AgcSetting, Capabilities, DeviceSetStatus, DeviceSettings, GainValue, ServerEvent, StateScope,
    Tuning,
};

use crate::{
    ChannelMedia, DEFAULT_CENTER_HZ, DeviceSetState, Engine, EngineError, FaultGate,
    RatePatchGuard, RebuildEntry, dc_block, fault_kind, hotplug, ids_of, lock_runtime,
    planning::{plan_center, validate_streams},
    refusal,
    runtime::{CaptureRuntime, DeviceRuntime},
    sample_rate_of, teardown_set,
};

#[derive(Default)]
struct SinkPoll {
    grown: Vec<(u32, u64, u64)>,
    recording: Vec<(u32, String)>,
    audio: Vec<(u32, u32, String)>,
    baseband: Vec<(u32, u32, String)>,
    export: Vec<(u32, String)>,
    history: Vec<(u32, String)>,
    changed: Vec<u32>,
}

fn take_the_wheel(mut delta: DeviceSettings) -> DeviceSettings {
    if delta.center_hz.is_some() && delta.tuning.is_none() {
        delta.tuning = Some(Tuning::Manual);
    }
    for stream in &mut delta.streams {
        if stream.center_hz.is_some() && stream.tuning.is_none() {
            stream.tuning = Some(Tuning::Manual);
        }
    }
    delta
}

impl Engine {
    pub(crate) fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        self.report_sinks(self.poll_sinks());
        self.read_agc_gains();
        self.recover_lost_sync();
        self.probe_bus(known, missing_once, gate, woken)
    }

    fn read_agc_gains(&self) {
        let running: Vec<(u32, bool, Arc<DeviceRuntime>)> = self
            .lock()
            .device_sets
            .iter()
            .filter(|(_, state)| state.status == DeviceSetStatus::Running)
            .map(|(id, state)| (*id, state.runs_agc(), state.runtime.clone()))
            .collect();
        for (ds, agc, runtime) in running {
            let gains = if agc {
                match lock_runtime(&runtime).agc_gains() {
                    Ok(gains) => gains,
                    Err(error) => {
                        tracing::warn!(ds, %error, "reading back the AGC gain failed");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let changed = self.lock().device_sets.get_mut(&ds).is_some_and(|state| {
                let changed = state.agc_gains != gains;
                state.agc_gains = gains;
                changed
            });
            if changed {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(ds),
                });
            }
        }
    }

    fn poll_sinks(&self) -> SinkPoll {
        let mut inner = self.lock();
        let mut grown: Vec<(u32, u64, u64)> = Vec::new();
        let mut rec_faults: Vec<(u32, String)> = Vec::new();
        let mut audio_rec_faults: Vec<(u32, u32, String)> = Vec::new();
        let mut export_faults: Vec<(u32, String)> = Vec::new();
        let mut baseband_faults: Vec<(u32, u32, String)> = Vec::new();
        let mut history_faults: Vec<(u32, String)> = Vec::new();
        let mut changed: Vec<u32> = Vec::new();
        for (id, s) in inner.device_sets.iter_mut() {
            let now = s.overruns_total();
            let delta = now - s.overruns_seen;
            s.overruns_seen = now;
            let mut dirty = delta > 0;
            let clipping = s.take_clipping();
            if clipping != s.clipping {
                s.clipping = clipping;
                dirty = true;
            }
            if delta > 0 {
                grown.push((*id, delta, s.take_worst_stall_ms()));
            }
            if let Some(rec) = &mut s.recording {
                let samples = rec.shared.samples();
                if samples != rec.samples_seen {
                    rec.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = rec.shared.error()
                    && !rec.error_seen
                {
                    rec.error_seen = true;
                    rec_faults.push((*id, error));
                    dirty = true;
                }
            }
            for (ch, recording) in &mut s.audio_recordings {
                let frames = recording.shared.frames();
                if frames != recording.frames_seen {
                    recording.frames_seen = frames;
                    dirty = true;
                }
                if let Some(error) = recording.shared.error()
                    && !recording.error_seen
                {
                    recording.error_seen = true;
                    audio_rec_faults.push((*id, ch.channel, error));
                    dirty = true;
                }
            }
            for (ch, recording) in &mut s.baseband_recordings {
                let samples = recording.shared.samples();
                if samples != recording.samples_seen {
                    recording.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = recording.shared.error()
                    && !recording.error_seen
                {
                    recording.error_seen = true;
                    baseband_faults.push((*id, *ch, error));
                    dirty = true;
                }
            }
            for (ch, export) in &mut s.channel_exports {
                let clients = export.shared.clients();
                if clients != export.clients_seen {
                    export.clients_seen = clients;
                    dirty = true;
                }
                let samples = export.shared.samples();
                if samples != export.samples_seen {
                    export.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = export.shared.error()
                    && !export.error_seen
                {
                    export.error_seen = true;
                    baseband_faults.push((*id, *ch, error));
                    dirty = true;
                }
            }
            if let Some(history) = &mut s.time_machine {
                let held = history.handle.shared().held();
                if held != history.held_seen {
                    history.held_seen = held;
                    dirty = true;
                }
                if let Some(error) = history.handle.shared().error()
                    && !history.error_seen
                {
                    history.error_seen = true;
                    history_faults.push((*id, error));
                    dirty = true;
                }
                if history.capture.is_some() && !history.handle.shared().capturing() {
                    history.capture = None;
                    dirty = true;
                }
            }
            if let Some(export) = &mut s.network_export {
                let clients = export.shared.clients();
                if clients != export.clients_seen {
                    export.clients_seen = clients;
                    dirty = true;
                }
                let samples = export.shared.samples();
                if samples != export.samples_seen {
                    export.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = export.shared.error()
                    && !export.error_seen
                {
                    export.error_seen = true;
                    export_faults.push((*id, error));
                    dirty = true;
                }
            }
            if dirty {
                changed.push(*id);
            }
        }
        if !changed.is_empty() {
            inner.revision += 1;
        }
        SinkPoll {
            grown,
            recording: rec_faults,
            audio: audio_rec_faults,
            baseband: baseband_faults,
            export: export_faults,
            history: history_faults,
            changed,
        }
    }

    fn report_sinks(&self, poll: SinkPoll) {
        for (ds, dropped, stalled_ms) in poll.grown {
            tracing::warn!(
                ds,
                dropped,
                stalled_ms,
                "capture loss: reported device gaps, full queues, or stale samples"
            );
        }
        for (ds, error) in poll.recording {
            tracing::warn!(ds, error = %error, "recording fault");
        }
        for (ds, channel, error) in poll.audio {
            tracing::warn!(ds, channel, error = %error, "audio recording fault");
        }
        for (ds, error) in poll.export {
            tracing::warn!(ds, error = %error, "network export fault");
        }
        for (ds, channel, error) in poll.baseband {
            tracing::warn!(ds, channel, error = %error, "channel baseband sink fault");
        }
        for (ds, error) in poll.history {
            tracing::warn!(ds, error = %error, "time machine fault");
        }
        for ds in poll.changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
    }

    fn probe_bus(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        let Some(reason) = gate.should_probe(sdrmm_device::usb::fingerprint(), woken) else {
            return false;
        };
        if reason == hotplug::Probe::BusChanged {
            self.lock_discovery().expire();
        }

        let mut ids = ids_of(&self.registry.probe_all());
        if self.wants_a_deeper_look(&ids) {
            ids = ids_of(&self.registry.probe_all_deep());
        }

        let (absent, returned): (HashSet<u32>, Vec<u32>) = {
            let inner = self.lock();
            let absent = inner
                .device_sets
                .iter()
                .filter(|(_, s)| {
                    s.array.is_none()
                        && s.status == DeviceSetStatus::Running
                        && !ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect();
            let returned = inner
                .device_sets
                .iter()
                .filter(|(_, s)| {
                    s.array.is_none()
                        && s.status == DeviceSetStatus::Error
                        && ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect();
            (absent, returned)
        };
        for ds in absent.intersection(missing_once) {
            self.mark_device_fault(
                *ds,
                DeviceError::Io("device disappeared from probe".to_string()),
            );
        }
        *missing_once = absent;
        for ds in returned {
            self.reconnect(ds);
        }

        self.recover_arrays();

        let changed = known.as_ref().is_some_and(|prev| *prev != ids);
        *known = Some(ids);
        // A radio the quick search cannot name, one that answers over the network, or one whose
        // vendor module only the deep search loads, still moved on the bus, and whoever has the
        // device list open is the one who should find out.
        if changed || reason == hotplug::Probe::BusChanged {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Devices,
            });
        }
        changed
    }

    /// Whether the cheap search left a question only a full one can answer: a radio that is
    /// streaming but nothing found, or a faulted one that may have come back. Both are worth
    /// seconds; a healthy machine never gets here.
    fn wants_a_deeper_look(&self, ids: &[String]) -> bool {
        let inner = self.lock();
        inner
            .device_sets
            .values()
            .filter(|s| s.array.is_none())
            .any(|s| match s.status {
                DeviceSetStatus::Running => !ids.contains(&s.info.id()),
                DeviceSetStatus::Error => true,
                DeviceSetStatus::Idle => false,
            })
    }

    pub(crate) fn reconnect(&self, ds: u32) {
        let _edit = sdrmm_device::lock(&self.array_edits);
        let stored = {
            let inner = self.lock();
            let Some(state) = inner.device_sets.get(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error {
                return;
            }
            (
                state.info.id(),
                state.settings.clone(),
                state.array.clone(),
                state.info.clone(),
            )
        };
        let (device_id, stored_settings, array, stored_info) = stored;

        let opened = if let Some(binding) = &array {
            self.reopen_array(binding)
                .map(|(device, binding)| (stored_info, device, Some(binding)))
        } else {
            self.registry
                .open(&device_id)
                .map(|(info, device)| (info, device, None))
        }
        .and_then(|(info, mut device, array)| {
            if array.is_none() {
                device.apply(&stored_settings.to_hardware())?;
            }
            Ok((info, device, array))
        });
        let (info, device, array) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let capabilities = device.capabilities().shifted_by(stored_settings.offset());
        let playback = device.playback();
        let mut settings = stored_settings.clone();
        settings.merge_from(&DeviceSettings::from_hardware(
            device.settings().clone(),
            stored_settings.offset_hz,
        ));
        let rate = sample_rate_of(&settings);
        let blocking = dc_block(&capabilities, &settings);
        let gate = Arc::new(Mutex::new(FaultGate::Pending(None)));
        let fault_tx = self.fault_tx.clone();
        let handler_gate = gate.clone();
        let runtime = match CaptureRuntime::start(device, &settings, blocking, move |err| {
            let mut gate = handler_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &mut *gate {
                FaultGate::Pending(slot) => *slot = Some(err),
                FaultGate::Armed => {
                    let _ = fault_tx.send((ds, err));
                }
            }
        }) {
            Ok(runtime) => runtime,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let stalls = runtime.stall_counters();
        let clip_meters = runtime.clip_meters();
        let runtime = Arc::new(DeviceRuntime::new(runtime));

        let (old_runtime, rebuilds, early_fault) = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            };
            if state.status != DeviceSetStatus::Error {
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            }
            let early_fault = match std::mem::replace(
                &mut *gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                FaultGate::Armed,
            ) {
                FaultGate::Pending(slot) => slot,
                FaultGate::Armed => None,
            };
            let old_runtime = std::mem::replace(&mut state.runtime, runtime);
            state.cmd_txs = cmd_txs;
            state.overruns = overruns;
            state.overruns_seen = 0;
            state.stalls = stalls;
            state.clip_meters = clip_meters;
            state.clipping.clear();
            state.array = array.clone();
            state.info = info;
            state.capabilities = capabilities;
            state.settings = settings;
            state.status = DeviceSetStatus::Running;
            state.error = None;
            state.playback = playback;
            let rebuilds: Vec<RebuildEntry> = state
                .channels
                .iter()
                .filter_map(|c| {
                    state.media.get(&c.id).map(|m| RebuildEntry {
                        id: c.id,
                        stream: c.stream,
                        settings: c.settings.clone(),
                        sinks: m.sinks.clone(),
                    })
                })
                .collect();
            inner.revision += 1;
            (old_runtime, rebuilds, early_fault)
        };
        lock_runtime(&old_runtime).stop();
        drop(old_runtime);

        if let Some(binding) = &array
            && let Err(error) = self.connect_array_inputs(ds, binding)
        {
            self.mark_device_fault(ds, DeviceError::Io(error.to_string()));
            return;
        }
        let mut dead: Vec<ChannelMedia> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        if let Some(err) = early_fault {
            tracing::warn!(ds, error = %err, "reconnected capture died immediately");
            self.mark_device_fault(ds, err);
            return;
        }
        tracing::info!(ds, device = %device_id, "device set reconnected after replug");
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

    fn note_reconnect_failure(&self, ds: u32, reason: &str) {
        let message = format!("device present but not reopenable: {reason}");
        let changed = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error || state.error.as_deref() == Some(&message) {
                false
            } else {
                state.error = Some(message);
                inner.revision += 1;
                true
            }
        };
        if changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
    }

    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        if let Some(key) = device_id.strip_prefix("array:") {
            return self.create_array_set(key);
        }
        self.refuse_reopen(device_id)?;
        let (info, device) = self.registry.open(device_id)?;
        self.create_opened_set(info, device, None)
    }

    pub(crate) fn create_opened_set(
        &self,
        info: sdrmm_wire::DeviceInfo,
        device: Box<dyn sdrmm_device::SdrDevice>,
        array: Option<crate::arrays::ArrayBinding>,
    ) -> Result<u32, EngineError> {
        if let Err(already) = self.refuse_reopen(&info.id()) {
            drop(device);
            return Err(already);
        }
        let capabilities = device.capabilities().clone();
        let settings = DeviceSettings {
            tuning: Some(Tuning::default()),
            ..device.settings().clone()
        };
        let playback = device.playback();

        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.creating.insert(id);
            id
        };
        let fault_tx = self.fault_tx.clone();
        let started = CaptureRuntime::start(
            device,
            &settings,
            dc_block(&capabilities, &settings),
            move |err| {
                let _ = fault_tx.send((id, err));
            },
        );
        let runtime = match started {
            Ok(runtime) => runtime,
            Err(e) => {
                let mut inner = self.lock();
                inner.creating.remove(&id);
                inner.pending_faults.remove(&id);
                return Err(e.into());
            }
        };

        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let stalls = runtime.stall_counters();
        let clip_meters = runtime.clip_meters();
        let faulted = {
            let mut inner = self.lock();
            inner.creating.remove(&id);
            let pending = inner.pending_faults.remove(&id);
            inner.device_sets.insert(
                id,
                DeviceSetState {
                    array,
                    info,
                    capabilities,
                    settings,
                    status: if pending.is_some() {
                        DeviceSetStatus::Error
                    } else {
                        DeviceSetStatus::Running
                    },
                    channels: Vec::new(),
                    media: HashMap::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    fault: pending.as_ref().map(fault_kind),
                    refused: None,
                    recording: None,
                    audio_recordings: HashMap::new(),
                    baseband_recordings: HashMap::new(),
                    channel_exports: HashMap::new(),
                    network_export: None,
                    time_machine: None,
                    scanners: HashMap::new(),
                    hunts: HashMap::new(),
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    stalls,
                    clip_meters,
                    clipping: Vec::new(),
                    agc_gains: Vec::new(),
                    playback,
                    coherent: None,
                    runtime: Arc::new(DeviceRuntime::new(runtime)),
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

    pub(crate) fn refuse_reopen(&self, device_id: &str) -> Result<(), EngineError> {
        let inner = self.lock();
        match inner
            .device_sets
            .iter()
            .find(|(_, set)| set.info.id() == device_id)
        {
            Some((id, _)) => Err(EngineError::DeviceAlreadyOpen(device_id.to_owned(), *id)),
            None => Ok(()),
        }
    }

    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
        let _edit = sdrmm_device::lock(&self.array_edits);
        self.remove_set(ds)
    }

    pub(crate) fn remove_set(&self, ds: u32) -> Result<(), EngineError> {
        for array in self.arrays_using(ds) {
            self.remove_set(array)?;
        }
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        self.detach_array(ds, removed.array.as_ref());
        let finalized = teardown_set(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        let _edit = sdrmm_device::lock(&self.array_edits);
        let removed: Vec<DeviceSetState> = {
            let mut inner = self.lock();
            if inner.device_sets.is_empty() {
                return;
            }
            inner.revision += 1;
            std::mem::take(&mut inner.device_sets)
                .into_values()
                .collect()
        };
        let mut finalized = false;
        for set in removed {
            finalized |= teardown_set(set);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
    }

    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        let _edit = sdrmm_device::lock(&self.array_edits);
        if self
            .lock()
            .device_sets
            .get(&ds)
            .is_some_and(|state| state.array.is_some())
        {
            self.patch_array(ds, take_the_wheel(delta))?;
            self.settle_tuning(ds);
            return Ok(());
        }
        let mut delta = delta;
        if let Some((array, forward)) = self.split_member_patch(ds, &mut delta) {
            self.patch_array(array, take_the_wheel(forward))?;
            self.settle_tuning(array);
        }
        if delta != DeviceSettings::default() {
            self.patch_device_from(ds, take_the_wheel(delta))?;
        }
        self.settle_tuning(ds);
        Ok(())
    }

    fn runtime_of(&self, ds: u32) -> Option<Arc<DeviceRuntime>> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(|state| state.runtime.clone())
    }

    /// What the radio open on this set can do by itself, for a caller deciding what to ask of
    /// it. Frequencies are the radio's own; the snapshot shows them through the converter.
    #[must_use]
    pub fn capabilities(&self, ds: u32) -> Option<Capabilities> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(DeviceSetState::hardware_capabilities)
    }

    pub(crate) fn settle_tuning(&self, ds: u32) -> bool {
        if let Some((array, _)) = self.array_of(ds) {
            return self.settle_tuning(array);
        }
        let Some(delta) = self.auto_center(ds) else {
            return false;
        };
        let arrayed = self
            .lock()
            .device_sets
            .get(&ds)
            .is_some_and(|state| state.array.is_some());
        let moved = if arrayed {
            self.patch_array(ds, delta)
        } else {
            self.patch_device_from(ds, delta)
        };
        match moved {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(ds, error = %e, "auto tuning could not move the radio");
                false
            }
        }
    }

    fn auto_center(&self, ds: u32) -> Option<DeviceSettings> {
        if !self.arrays_using(ds).is_empty() {
            return None;
        }
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        if !state.tunes_freely() {
            return None;
        }
        let stitched = state.stitched();
        if stitched
            .as_ref()
            .is_some_and(|stitched| stitched.mode == sdrmm_wire::StitchMode::Auto)
        {
            return state.plan_stitch();
        }
        let group = if stitched.is_some() {
            Vec::new()
        } else {
            state.coherent_lanes()
        };
        let channels = if state.array.is_some() {
            crate::arrays::array_channels(&inner.device_sets, ds, None)
        } else {
            state.channels.clone()
        };
        plan_center(&state.capabilities, &state.settings, &channels, &group)
    }

    pub(crate) fn patch_device_from(
        &self,
        ds: u32,
        mut delta: DeviceSettings,
    ) -> Result<(), EngineError> {
        let serialized = self
            .runtime_of(ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let _patching = serialized.patching();
        if serialized.sweeping() {
            return Err(EngineError::Scan(
                "the radio is sweeping in firmware; stop the scan first".to_string(),
            ));
        }
        let (runtime, hardware, _rate_guard) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.tune_group_together(&mut delta);
            state.settings.carry_offset(&mut delta);
            let (hardware, rate_change) = state.validate_patch(&delta)?;
            let runtime = state.runtime.clone();
            let guard = rate_change.then(|| {
                state.rate_patches += 1;
                RatePatchGuard { engine: self, ds }
            });
            (runtime, hardware, guard)
        };
        let applied = runtime.apply(&hardware);
        self.note_refusal(ds, &hardware, applied.as_ref().err());
        let actual = applied?.map(|actual| DeviceSettings::from_hardware(actual, delta.offset_hz));
        let (settings, blocking, rate, rate_changed, rebuilds, retuned) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let old_rate = sample_rate_of(&state.settings);
            let old_centers = state.coherent_centers();
            let old_front_end = front_end(&state.settings);
            let locked_by_export = state.network_export.is_some();
            let owner = if locked_by_export {
                Some(("exporting", "stop the export first"))
            } else if state.recording.is_some() {
                Some(("recording", "stop the recording first"))
            } else if state.time_machine.is_some() {
                Some(("holding history", "disarm the time machine first"))
            } else {
                None
            };
            if let Some((owner, remedy)) = owner
                && delta.sample_rate.is_some_and(|r| r != old_rate)
            {
                drop(inner);
                let revert = DeviceSettings {
                    sample_rate: Some(old_rate),
                    ..DeviceSettings::default()
                };
                if let Err(e) = runtime.apply(&revert) {
                    let message = format!(
                        "sample rate is locked while {owner}, and reverting the device to \
                         {old_rate} Hz failed: {e}"
                    );
                    return Err(if locked_by_export {
                        EngineError::NetworkExport(message)
                    } else {
                        EngineError::Recording(message)
                    });
                }
                let message = format!("sample rate is locked while {owner}; {remedy}");
                return Err(if locked_by_export {
                    EngineError::NetworkExport(message)
                } else {
                    EngineError::Recording(message)
                });
            }
            state.settings.merge_from(&delta);
            if let Some(actual) = &actual {
                state.settings.merge_from(actual);
            }
            let export_center = state.network_export.as_ref().map(|export| {
                state
                    .settings
                    .for_stream(export.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(export), Some(center_hz)) = (state.network_export.as_mut(), export_center)
            {
                export.center_hz = center_hz;
            }
            let history_center = state.time_machine.as_ref().map(|history| {
                state
                    .settings
                    .for_stream(history.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(history), Some(center_hz)) = (state.time_machine.as_mut(), history_center)
            {
                history.center_hz = center_hz;
            }
            let rate = sample_rate_of(&state.settings);
            let rebuilds: Vec<RebuildEntry> = if rate == old_rate {
                Vec::new()
            } else {
                state
                    .channels
                    .iter()
                    .filter_map(|c| {
                        state.media.get(&c.id).map(|m| RebuildEntry {
                            id: c.id,
                            stream: c.stream,
                            settings: c.settings.clone(),
                            sinks: m.sinks.clone(),
                        })
                    })
                    .collect()
            };
            if let Some(current) = lock_runtime(&state.runtime).capabilities() {
                state.capabilities = current.shifted_by(state.settings.offset());
            }
            let settings = state.settings.clone();
            let blocking = dc_block(&state.capabilities, &settings);
            let centers = state.coherent_centers();
            let retuned =
                centers != old_centers || rate != old_rate || front_end(&settings) != old_front_end;
            inner.revision += 1;
            (
                settings,
                blocking,
                rate,
                rate != old_rate,
                rebuilds,
                retuned,
            )
        };
        lock_runtime(&runtime).set_meta(&settings, blocking);
        if !rate_changed {
            self.sync_extra_lane(ds);
            self.notify_coherent_meta(ds, retuned);
        } else if let Err(error) = self.restart_coherent(ds) {
            self.mark_device_fault(ds, DeviceError::Io(format!("coherent restart: {error}")));
        }
        let mut dead: Vec<ChannelMedia> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    fn note_refusal(&self, ds: u32, hardware: &DeviceSettings, error: Option<&DeviceError>) {
        let refused = error.map(|error| refusal::refused(hardware, error));
        {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.refused == refused {
                return;
            }
            state.refused = refused;
            inner.revision += 1;
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

    pub(crate) fn guard_device_patches<'a>(
        &self,
        patches: impl IntoIterator<Item = (u32, &'a DeviceSettings)>,
    ) -> Result<Vec<RatePatchGuard<'_>>, EngineError> {
        let mut inner = self.lock();
        let mut changing = Vec::new();
        for (ds, delta) in patches {
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let (_, rate_change) = state.validate_patch(delta)?;
            if rate_change {
                changing.push(ds);
            }
        }
        let mut guards = Vec::with_capacity(changing.len());
        for (ds, state) in &mut inner.device_sets {
            if changing.contains(ds) {
                state.rate_patches += 1;
                guards.push(RatePatchGuard {
                    engine: self,
                    ds: *ds,
                });
            }
        }
        Ok(guards)
    }
}

impl DeviceSetState {
    pub(crate) fn tunes_freely(&self) -> bool {
        self.recording.is_none() && !self.runtime.sweeping()
    }

    fn validate_patch(
        &self,
        delta: &DeviceSettings,
    ) -> Result<(DeviceSettings, bool), EngineError> {
        let hardware = delta.to_hardware();
        validate_streams(&self.hardware_capabilities(), &hardware)?;
        let rate_change = delta
            .sample_rate
            .is_some_and(|rate| rate != sample_rate_of(&self.settings));
        if rate_change {
            self.validate_rate_change()?;
        }
        Ok((hardware, rate_change))
    }

    fn validate_rate_change(&self) -> Result<(), EngineError> {
        if self.network_export.is_some() {
            return Err(EngineError::NetworkExport(
                "sample rate is locked while exporting; stop the export first".to_string(),
            ));
        }
        if self.recording.is_some() {
            return Err(EngineError::Recording(
                "sample rate is locked while recording; stop the recording first".to_string(),
            ));
        }
        if self.time_machine.is_some() {
            return Err(EngineError::Recording(
                "sample rate is locked while the time machine holds history; disarm it first"
                    .to_string(),
            ));
        }
        Ok(())
    }
}
fn front_end(
    settings: &DeviceSettings,
) -> (Vec<GainValue>, Option<AgcSetting>, Vec<Vec<GainValue>>) {
    (
        settings.gains.clone(),
        settings.agc.clone(),
        settings
            .streams
            .iter()
            .map(|stream| stream.gains.clone())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{GainKind, StreamSettings};

    use super::*;

    fn lane_gain(db: f64) -> DeviceSettings {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream: 2,
                gains: vec![GainValue::new(GainKind::Tuner, db)],
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn one_lanes_gain_moving_changes_the_front_end() {
        assert_ne!(front_end(&lane_gain(12.5)), front_end(&lane_gain(29.7)));
    }

    #[test]
    fn switching_agc_changes_the_front_end() {
        let on = DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        };
        assert_ne!(front_end(&on), front_end(&DeviceSettings::default()));
    }

    #[test]
    fn a_retune_alone_leaves_the_front_end_as_it_was() {
        let tuned = DeviceSettings {
            center_hz: Some(433.92e6),
            ..lane_gain(12.5)
        };
        assert_eq!(front_end(&tuned), front_end(&lane_gain(12.5)));
    }
}
