use std::{sync::Arc, thread::JoinHandle};

use sdrmm_wire::{
    DeviceSetStatus, NetworkExportSettings, NetworkExportStatus, RecordingStatus, ServerEvent,
    StateScope,
};

use crate::{
    DEFAULT_CENTER_HZ, DeviceSetState, Engine, EngineError, NetworkExportCommit,
    NetworkExportState, channel_input_rate, check_export_request, descriptor_for,
    join_network_writer, join_recording_writer, network_export, recording,
    recording::{RecorderTap, RecordingShared},
    remove_recording_files,
    runtime::DspCommand,
    sample_rate_of,
};

pub(crate) struct ChannelBasebandRecording {
    pub(crate) file: String,
    pub(crate) stream: u32,
    pub(crate) started_at: String,
    pub(crate) tap: RecorderTap,
    pub(crate) shared: Arc<RecordingShared>,
    pub(crate) writer: JoinHandle<()>,
    pub(crate) overruns_at_start: u64,
    pub(crate) samples_seen: u64,
    pub(crate) error_seen: bool,
}

impl ChannelBasebandRecording {
    pub(crate) fn status(&self, overruns_now: u64) -> RecordingStatus {
        RecordingStatus {
            file: self.file.clone(),
            stream: self.stream,
            started_at: self.started_at.clone(),
            samples: self.shared.samples(),
            bytes: self.shared.bytes(),
            overruns: overruns_now.saturating_sub(self.overruns_at_start),
            error: self.shared.error(),
        }
    }

    pub(crate) fn finish(self, overruns_now: u64) -> RecordingStatus {
        let Self {
            file,
            stream,
            started_at,
            tap,
            shared,
            writer,
            overruns_at_start,
            ..
        } = self;
        drop(tap);
        join_recording_writer(writer);
        RecordingStatus {
            file,
            stream,
            started_at,
            samples: shared.samples(),
            bytes: shared.bytes(),
            overruns: overruns_now.saturating_sub(overruns_at_start),
            error: shared.error(),
        }
    }

    pub(crate) fn join(self) {
        let _ = self.finish(0);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BasebandPlan {
    stream: u32,
    sample_rate: f64,
    center_hz: f64,
    hardware: String,
}

#[derive(Default)]
pub(crate) struct BasebandSinks {
    pub(crate) recording: Option<ChannelBasebandRecording>,
    pub(crate) export: Option<NetworkExportState>,
}

impl BasebandSinks {
    pub(crate) fn is_empty(&self) -> bool {
        self.recording.is_none() && self.export.is_none()
    }

    pub(crate) fn join(self) {
        if let Some(recording) = self.recording {
            recording.join();
        }
        if let Some(mut export) = self.export {
            export.join();
        }
    }
}

impl DeviceSetState {
    fn baseband_plan(&self, ds: u32, ch: u32) -> Result<BasebandPlan, EngineError> {
        if self.status != DeviceSetStatus::Running {
            return Err(EngineError::Recording(
                "device set is not running".to_string(),
            ));
        }
        let channel = self
            .channels
            .iter()
            .find(|c| c.id == ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        let descriptor = descriptor_for(&channel.settings.params)?;
        let center = self
            .settings
            .for_stream(channel.stream, &self.capabilities.per_stream)
            .center_hz
            .unwrap_or(DEFAULT_CENTER_HZ);
        Ok(BasebandPlan {
            stream: channel.stream,
            sample_rate: channel_input_rate(&descriptor, sample_rate_of(&self.settings)),
            center_hz: center + channel.settings.offset_hz,
            hardware: self.info.label.clone(),
        })
    }

    pub(crate) fn release_baseband_sinks(&mut self, ch: u32, stream: u32) -> BasebandSinks {
        let recording = self.baseband_recordings.remove(&ch);
        if recording.is_some() {
            self.send_dsp(stream, DspCommand::StopBasebandRecording { id: ch });
        }
        let export = self.channel_exports.remove(&ch);
        if export.is_some() {
            self.send_dsp(stream, DspCommand::StopBasebandExport { id: ch });
        }
        BasebandSinks { recording, export }
    }
}

impl Engine {
    pub(crate) fn close_baseband_sinks(&self, ds: u32, ch: u32, sinks: BasebandSinks, why: &str) {
        if sinks.is_empty() {
            return;
        }
        let wrote_file = sinks.recording.is_some();
        tracing::info!(ds, channel = ch, "baseband sinks stopped: {why}");
        sinks.join();
        if wrote_file {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
    }
    fn channel_baseband(&self, ds: u32, ch: u32) -> Result<BasebandPlan, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        state.baseband_plan(ds, ch)
    }

    pub fn start_channel_baseband_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<RecordingStatus, EngineError> {
        loop {
            let plan = self.channel_baseband(ds, ch)?;
            {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                if state.baseband_recordings.contains_key(&ch) {
                    return Err(EngineError::Recording(
                        "this channel's baseband is already recording".to_string(),
                    ));
                }
            }
            let Some(dir) = self.recordings_dir.clone() else {
                return Err(EngineError::Recording(
                    "no recordings directory configured".to_string(),
                ));
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| EngineError::RecordingIo(format!("create {}: {e}", dir.display())))?;
            let started_at = jiff::Timestamp::now();
            let (sigmf, file) = recording::create_writer(
                &dir,
                &format!("bb_{ds}_{ch}"),
                plan.stream,
                started_at,
                plan.sample_rate,
                plan.center_hz,
                &plan.hardware,
            )?;
            let stem = sigmf.stem().to_path_buf();
            let (tap, position, messages, shared) = recording::create_tap();
            drop(position);
            let writer = recording::spawn_writer(sigmf, messages, shared.clone())?;

            let committed = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if !state.baseband_recordings.contains_key(&ch)
                            && state.baseband_plan(ds, ch).ok().as_ref() == Some(&plan) =>
                    {
                        let recording = ChannelBasebandRecording {
                            file,
                            stream: plan.stream,
                            started_at: started_at.to_string(),
                            tap: tap.clone(),
                            shared,
                            writer,
                            overruns_at_start: state.overruns_total(),
                            samples_seen: 0,
                            error_seen: false,
                        };
                        let status = recording.status(state.overruns_total());
                        state.baseband_recordings.insert(ch, recording);
                        state.send_dsp(
                            plan.stream,
                            DspCommand::StartBasebandRecording { id: ch, tap },
                        );
                        inner.revision += 1;
                        Ok(status)
                    }
                    _ => Err((tap, writer)),
                }
            };
            match committed {
                Ok(status) => {
                    self.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(ds),
                    });
                    return Ok(status);
                }
                Err((tap, writer)) => {
                    drop(tap);
                    join_recording_writer(writer);
                    remove_recording_files(&stem);
                }
            }
        }
    }

    pub fn stop_channel_baseband_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<RecordingStatus, EngineError> {
        let (recording, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(recording) = state.baseband_recordings.remove(&ch) else {
                return Err(EngineError::Recording(
                    "this channel's baseband is not recording".to_string(),
                ));
            };
            state.send_dsp(
                recording.stream,
                DspCommand::StopBasebandRecording { id: ch },
            );
            let overruns = state.overruns_total();
            inner.revision += 1;
            (recording, overruns)
        };
        let status = recording.finish(overruns);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Recordings,
        });
        Ok(status)
    }

    pub fn start_channel_network_export(
        &self,
        ds: u32,
        ch: u32,
        node: String,
        settings: NetworkExportSettings,
    ) -> Result<NetworkExportStatus, EngineError> {
        check_export_request(&node, &settings)?;
        let plan = self.channel_baseband(ds, ch)?;
        {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            if state.channel_exports.contains_key(&ch) {
                return Err(EngineError::NetworkExport(
                    "this channel is already being exported".to_owned(),
                ));
            }
        }
        let (tap, shared, writer) = network_export::start(&settings)?;
        let commit = {
            let mut inner = self.lock();
            match inner.device_sets.get_mut(&ds) {
                Some(state)
                    if !state.channel_exports.contains_key(&ch)
                        && state.baseband_plan(ds, ch).ok().as_ref() == Some(&plan) =>
                {
                    let export = NetworkExportState {
                        node: node.clone(),
                        stream: plan.stream,
                        settings: settings.clone(),
                        sample_rate: plan.sample_rate.round() as u64,
                        center_hz: plan.center_hz.round() as i64,
                        shared,
                        writer: Some(writer),
                        overruns_at_start: state.overruns_total(),
                        samples_seen: 0,
                        error_seen: false,
                    };
                    let status = export.status(state.overruns_total());
                    state.channel_exports.insert(ch, export);
                    state.send_dsp(plan.stream, DspCommand::StartBasebandExport { id: ch, tap });
                    inner.revision += 1;
                    NetworkExportCommit::Started(status)
                }
                _ => NetworkExportCommit::Aborted {
                    tap,
                    writer,
                    patch_in_flight: false,
                },
            }
        };
        match commit {
            NetworkExportCommit::Started(status) => {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(ds),
                });
                Ok(status)
            }
            NetworkExportCommit::Aborted { tap, writer, .. } => {
                drop(tap);
                join_network_writer(writer);
                Err(EngineError::NetworkExport(
                    "the channel went away before its export started".to_owned(),
                ))
            }
        }
    }

    pub fn stop_channel_network_export(
        &self,
        ds: u32,
        ch: u32,
        node: &str,
    ) -> Result<NetworkExportStatus, EngineError> {
        let (mut export, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(active) = state.channel_exports.get(&ch) else {
                return Err(EngineError::NetworkExport(
                    "this channel is not being exported".to_owned(),
                ));
            };
            if active.node != node {
                return Err(EngineError::NetworkExport(format!(
                    "this channel's export belongs to node `{}`",
                    active.node
                )));
            }
            let Some(export) = state.channel_exports.remove(&ch) else {
                return Err(EngineError::NetworkExport(
                    "the export vanished while stopping".to_owned(),
                ));
            };
            state.send_dsp(export.stream, DspCommand::StopBasebandExport { id: ch });
            let overruns = state.overruns_total();
            inner.revision += 1;
            (export, overruns)
        };
        export.join();
        let status = export.status(overruns);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }
}
