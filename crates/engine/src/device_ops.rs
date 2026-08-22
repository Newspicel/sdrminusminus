use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use sdrmm_device::DeviceError;
use sdrmm_wire::{
    Capabilities, DeviceSetStatus, DeviceSettings, ServerEvent, StateScope, StreamSettings,
};

use crate::{
    ChannelMedia, DEFAULT_CENTER_HZ, DeviceSetState, Engine, EngineError, FaultGate, FrontEndPlan,
    PatchOrigin, RatePatchGuard, RebuildEntry, fault_kind, hotplug, ids_of, lock_runtime,
    planning::{
        descriptor_for, hardware_delta, plan_front_end, validate_channel, validate_streams,
    },
    runtime::CaptureRuntime,
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

impl Engine {
    pub(crate) fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        self.report_sinks(self.poll_sinks());
        self.probe_bus(known, missing_once, gate, woken)
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
                    audio_rec_faults.push((*id, *ch, error));
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
                "capture ring overrun: device samples dropped while the dsp thread was held off"
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
                    s.status == DeviceSetStatus::Running && !ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect();
            let returned = inner
                .device_sets
                .iter()
                .filter(|(_, s)| s.status == DeviceSetStatus::Error && ids.contains(&s.info.id()))
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

        let changed = known.as_ref().is_some_and(|prev| *prev != ids);
        *known = Some(ids);
        // A radio the quick search cannot name — one that answers over the network, or one whose
        // vendor module only the deep search loads — still moved on the bus, and whoever has the
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
        inner.device_sets.values().any(|s| match s.status {
            DeviceSetStatus::Running => !ids.contains(&s.info.id()),
            DeviceSetStatus::Error => true,
            DeviceSetStatus::Idle => false,
        })
    }

    fn reconnect(&self, ds: u32) {
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
                state.channels.clone(),
            )
        };
        let (device_id, stored_settings, stored_channels) = stored;

        let opened = self
            .registry
            .open(&device_id)
            .and_then(|(info, mut device)| {
                let front_end =
                    plan_front_end(device.capabilities(), &stored_settings, &stored_channels);
                device.apply(&stored_settings.to_hardware(front_end.lo_offset_hz))?;
                Ok((info, device, front_end))
            });
        let (info, device, front_end) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let capabilities = device.capabilities().clone();
        let playback = device.playback();
        let mut settings = stored_settings.clone();
        settings.merge_from(&device.settings().to_operator(front_end.lo_offset_hz));
        let rate = sample_rate_of(&settings);
        let gate = Arc::new(Mutex::new(FaultGate::Pending(None)));
        let fault_tx = self.fault_tx.clone();
        let handler_gate = gate.clone();
        let runtime = match CaptureRuntime::start(device, &settings, front_end, move |err| {
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
        let runtime = Arc::new(Mutex::new(runtime));

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
            state.info = info;
            state.capabilities = capabilities;
            state.settings = settings;
            state.front_end = front_end;
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
        self.refuse_reopen(device_id)?;
        let (info, device) = self.registry.open(device_id)?;
        if let Err(already) = self.refuse_reopen(&info.id()) {
            drop(device);
            return Err(already);
        }
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();
        let playback = device.playback();

        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.creating.insert(id);
            id
        };
        let fault_tx = self.fault_tx.clone();
        let started =
            CaptureRuntime::start(device, &settings, FrontEndPlan::default(), move |err| {
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

        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let stalls = runtime.stall_counters();
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
                    front_end: FrontEndPlan::default(),
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
                    recording: None,
                    audio_recordings: HashMap::new(),
                    baseband_recordings: HashMap::new(),
                    channel_exports: HashMap::new(),
                    network_export: None,
                    time_machine: None,
                    scanner: None,
                    hunt: None,
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    stalls,
                    playback,
                    coherent: None,
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

    fn refuse_reopen(&self, device_id: &str) -> Result<(), EngineError> {
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
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.leave_scan_session(ds);
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
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
        let removed: Vec<DeviceSetState> = {
            let mut inner = self.lock();
            if inner.device_sets.is_empty() {
                return;
            }
            inner.revision += 1;
            inner.scan_session = None;
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
        self.patch_device_from(ds, delta, PatchOrigin::Client)
    }

    /// What the radio open on this set can do, for a caller deciding what to ask of it.
    #[must_use]
    pub fn capabilities(&self, ds: u32) -> Option<Capabilities> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(|state| state.capabilities.clone())
    }

    /// Moves the LO out from under a channel that has just been added, retuned, or reshaped.
    ///
    /// The displacement is mixed back out downstream, so nothing the operator sees moves; only the
    /// front end's own DC term does.
    pub(crate) fn replace_lo(&self, ds: u32) {
        let (runtime, settings, front_end, hardware) = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            let resolved = plan_front_end(&state.capabilities, &state.settings, &state.channels);
            if resolved == state.front_end {
                return;
            }
            let restated = DeviceSettings {
                center_hz: state.settings.center_hz,
                streams: state
                    .settings
                    .streams
                    .iter()
                    .filter(|s| s.center_hz.is_some())
                    .map(|s| StreamSettings {
                        stream: s.stream,
                        center_hz: s.center_hz,
                        ..StreamSettings::default()
                    })
                    .collect(),
                ..DeviceSettings::default()
            };
            state.front_end = resolved;
            (
                state.runtime.clone(),
                state.settings.clone(),
                resolved,
                restated.to_hardware(resolved.lo_offset_hz),
            )
        };
        if let Err(e) = lock_runtime(&runtime).apply(&hardware) {
            tracing::warn!(ds, error = %e, "could not move the LO clear of a channel");
            return;
        }
        lock_runtime(&runtime).set_meta(&settings, front_end);
    }

    pub(crate) fn patch_device_from(
        &self,
        ds: u32,
        delta: DeviceSettings,
        origin: PatchOrigin,
    ) -> Result<(), EngineError> {
        let (runtime, hardware, front_end, _rate_guard) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            if origin == PatchOrigin::Client && (state.scanner.is_some() || state.hunt.is_some()) {
                return Err(EngineError::Scan(
                    "the device is being tuned by a running scan; stop the scan first".to_string(),
                ));
            }
            let mut wanted = state.settings.clone();
            wanted.merge_from(&delta);
            let front_end = plan_front_end(&state.capabilities, &wanted, &state.channels);
            let hardware = hardware_delta(
                &delta,
                &wanted,
                front_end.lo_offset_hz,
                state.front_end.lo_offset_hz,
            );
            validate_streams(&state.capabilities, &hardware)?;
            let mut rate_change = false;
            if let Some(new_rate) = delta.sample_rate
                && new_rate != sample_rate_of(&state.settings)
            {
                if state.network_export.is_some() {
                    return Err(EngineError::NetworkExport(
                        "sample rate is locked while exporting; stop the export first".to_string(),
                    ));
                }
                if state.recording.is_some() {
                    return Err(EngineError::Recording(
                        "sample rate is locked while recording; stop the recording first"
                            .to_string(),
                    ));
                }
                if state.time_machine.is_some() {
                    return Err(EngineError::Recording(
                        "sample rate is locked while the time machine holds history; disarm it \
                         first"
                            .to_string(),
                    ));
                }
                for channel in &state.channels {
                    let descriptor = descriptor_for(&channel.settings.params)?;
                    validate_channel(&descriptor, &channel.settings, new_rate)?;
                }
                rate_change = true;
            }
            let runtime = state.runtime.clone();
            let guard = rate_change.then(|| {
                state.rate_patches += 1;
                RatePatchGuard { engine: self, ds }
            });
            (runtime, hardware, front_end, guard)
        };
        let actual = {
            let mut runtime = lock_runtime(&runtime);
            runtime.apply(&hardware)?;
            runtime.device_settings(front_end.lo_offset_hz)
        };
        let (settings, rate, rebuilds, retuned) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let old_rate = sample_rate_of(&state.settings);
            let old_center = state.settings.center_hz;
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
                if let Err(e) = lock_runtime(&runtime).apply(&revert) {
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
            state.front_end = front_end;
            if let Some(coherence) = lock_runtime(&state.runtime).coherence() {
                state.capabilities.coherence = coherence;
            }
            let settings = state.settings.clone();
            let retuned = settings.center_hz != old_center || rate != old_rate;
            inner.revision += 1;
            (settings, rate, rebuilds, retuned)
        };
        lock_runtime(&runtime).set_meta(&settings, front_end);
        self.notify_coherent_meta(
            ds,
            settings.center_hz.unwrap_or(crate::DEFAULT_CENTER_HZ),
            retuned,
        );
        let mut dead: Vec<ChannelMedia> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        if origin == PatchOrigin::Client {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
        Ok(())
    }
}
