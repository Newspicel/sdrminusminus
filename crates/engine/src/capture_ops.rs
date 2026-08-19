use std::{path::PathBuf, sync::atomic::Ordering};

use sdrmm_wire::{
    AudioRecordingStatus, DeviceSetStatus, NetworkExportSettings, NetworkExportStatus, ServerEvent,
    StateScope,
};

use crate::{
    ChannelAudioRecording, DEFAULT_CENTER_HZ, Engine, EngineError, FinalizedRecording,
    NetworkExportCommit, NetworkExportState, RecordingState, audio_recording, check_export_request,
    join_network_writer, join_recording_writer, network_export, planning::descriptor_for,
    recording, remove_recording_files, runtime::DspCommand, sample_rate_of,
};

impl Engine {
    pub fn start_recording(&self, ds: u32, stream: u32) -> Result<(), EngineError> {
        loop {
            let (rate, center, hw) = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                state.check_stream(stream)?;
                if state.recording.is_some() {
                    return Err(EngineError::Recording("already recording".to_string()));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::Recording(
                        "device set is not running".to_string(),
                    ));
                }
                let center = state
                    .settings
                    .for_stream(stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ);
                (
                    sample_rate_of(&state.settings),
                    center,
                    state.info.label.clone(),
                )
            };
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
                &format!("rec_{ds}"),
                stream,
                started_at,
                rate,
                center,
                &hw,
            )?;
            let stem = sigmf.stem().to_path_buf();
            let (tap, position, messages, shared) = recording::create_tap();
            let writer = recording::spawn_writer(sigmf, messages, shared.clone())?;

            let (aborted, patch_in_flight) = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && state.recording.is_none()
                            && state.rate_patches == 0
                            && state.check_stream(stream).is_ok()
                            && sample_rate_of(&state.settings) == rate =>
                    {
                        state.recording = Some(RecordingState {
                            file,
                            stream,
                            started_at: started_at.to_string(),
                            stem: stem.clone(),
                            shared,
                            position: Some(position.clone()),
                            writer,
                            overruns_at_start: state.overruns_total(),
                            samples_seen: 0,
                            error_seen: false,
                        });
                        state.send_dsp(stream, DspCommand::StartRecording { tap });
                        inner.revision += 1;
                        (None, false)
                    }
                    Some(state) if state.rate_patches > 0 => (Some((tap, writer)), true),
                    _ => (Some((tap, writer)), false),
                }
            };
            let Some((tap, writer)) = aborted else {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(ds),
                });
                return Ok(());
            };
            drop(tap);
            drop(position);
            join_recording_writer(writer);
            remove_recording_files(&stem);
            if patch_in_flight {
                return Err(EngineError::Recording(
                    "a sample-rate change is in flight; retry once it completes".to_string(),
                ));
            }
        }
    }

    pub fn stop_recording(&self, ds: u32) -> Result<FinalizedRecording, EngineError> {
        let (recording, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(recording) = state.recording.take() else {
                return Err(EngineError::Recording("not recording".to_string()));
            };
            state.send_dsp(recording.stream, DspCommand::StopRecording);
            let overruns = state.overruns.clone();
            inner.revision += 1;
            (recording, overruns)
        };
        let RecordingState {
            stem,
            stream,
            started_at,
            shared,
            mut position,
            writer,
            overruns_at_start,
            ..
        } = recording;
        drop(position.take());
        join_recording_writer(writer);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        let overruns_now: u64 = overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum();
        Ok(FinalizedRecording {
            stem,
            stream,
            started_at,
            samples: shared.samples(),
            bytes: shared.bytes(),
            overruns: overruns_now - overruns_at_start,
            error: shared.error(),
        })
    }

    #[must_use]
    pub fn audio_recordings_dir(&self) -> Option<PathBuf> {
        self.recordings_dir
            .as_deref()
            .map(audio_recording::audio_dir)
    }

    pub fn start_channel_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<AudioRecordingStatus, EngineError> {
        loop {
            let (stream, channels) = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                let channel = state
                    .channels
                    .iter()
                    .find(|c| c.id == ch)
                    .ok_or(EngineError::ChannelNotFound(ch, ds))?;
                if state.audio_recordings.contains_key(&ch) {
                    return Err(EngineError::Recording(
                        "this channel is already recording".to_string(),
                    ));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::Recording(
                        "device set is not running".to_string(),
                    ));
                }
                if !descriptor_for(&channel.settings.params)?.has_audio {
                    return Err(EngineError::Recording(format!(
                        "`{}` channels produce no audio to record",
                        channel.settings.params.type_id()
                    )));
                }
                (
                    channel.stream,
                    sdrmm_channels::audio_channels(&channel.settings.params),
                )
            };
            let Some(dir) = self.audio_recordings_dir() else {
                return Err(EngineError::Recording(
                    "no recordings directory configured".to_string(),
                ));
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| EngineError::RecordingIo(format!("create {}: {e}", dir.display())))?;
            let started_at = jiff::Timestamp::now();
            let writer = audio_recording::create_writer(
                &dir,
                ds,
                ch,
                started_at,
                sdrmm_channels::AUDIO_RATE,
                channels,
            )?;
            let path = writer.path().to_path_buf();
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            let (tap, blocks, shared) = audio_recording::create_tap();
            let thread = audio_recording::spawn_writer(writer, blocks, shared.clone())?;

            let committed = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && !state.audio_recordings.contains_key(&ch)
                            && state.channels.iter().any(|c| {
                                c.id == ch
                                    && c.stream == stream
                                    && sdrmm_channels::audio_channels(&c.settings.params)
                                        == channels
                            }) =>
                    {
                        let recording = ChannelAudioRecording {
                            file,
                            stream,
                            started_at: started_at.to_string(),
                            channels,
                            tap: tap.clone(),
                            shared,
                            writer: thread,
                            frames_seen: 0,
                            error_seen: false,
                        };
                        let status = recording.status();
                        state.audio_recordings.insert(ch, recording);
                        state.send_dsp(stream, DspCommand::StartChannelRecording { id: ch, tap });
                        inner.revision += 1;
                        Ok(status)
                    }
                    _ => Err((tap, thread, path)),
                }
            };
            match committed {
                Ok(status) => {
                    self.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(ds),
                    });
                    return Ok(status);
                }
                Err((tap, thread, path)) => {
                    drop(tap);
                    if thread.join().is_err() {
                        tracing::error!("audio recording writer thread panicked");
                    }
                    if let Err(e) = std::fs::remove_file(&path)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(path = %path.display(), error = %e, "aborted audio recording left a file behind");
                    }
                }
            }
        }
    }

    pub fn stop_channel_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<AudioRecordingStatus, EngineError> {
        let recording = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(recording) = state.audio_recordings.remove(&ch) else {
                return Err(EngineError::Recording(
                    "this channel is not recording".to_string(),
                ));
            };
            state.send_dsp(
                recording.stream,
                DspCommand::StopChannelRecording { id: ch },
            );
            inner.revision += 1;
            recording
        };
        let (file, started_at, channels, shared) = (
            recording.file.clone(),
            recording.started_at.clone(),
            recording.channels,
            recording.shared.clone(),
        );
        recording.join();
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Recordings,
        });
        Ok(AudioRecordingStatus {
            file,
            started_at,
            channels,
            frames: shared.frames(),
            bytes: shared.bytes(),
            error: shared.error(),
        })
    }

    pub fn start_network_export(
        &self,
        ds: u32,
        node: String,
        stream: u32,
        settings: NetworkExportSettings,
    ) -> Result<NetworkExportStatus, EngineError> {
        check_export_request(&node, &settings)?;
        loop {
            let rate = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                state.check_stream(stream)?;
                if state.network_export.is_some() {
                    return Err(EngineError::NetworkExport(
                        "another network export is already active".to_owned(),
                    ));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::NetworkExport(
                        "device set is not running".to_owned(),
                    ));
                }
                sample_rate_of(&state.settings)
            };
            let (tap, shared, writer) = network_export::start(&settings)?;
            let commit = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && state.network_export.is_none()
                            && state.rate_patches == 0
                            && state.check_stream(stream).is_ok()
                            && sample_rate_of(&state.settings) == rate =>
                    {
                        let center = state
                            .settings
                            .for_stream(stream, &state.capabilities.per_stream)
                            .center_hz
                            .unwrap_or(DEFAULT_CENTER_HZ);
                        let export = NetworkExportState {
                            node: node.clone(),
                            stream,
                            settings: settings.clone(),
                            sample_rate: rate.round() as u64,
                            center_hz: center.round() as i64,
                            shared,
                            writer: Some(writer),
                            overruns_at_start: state.overruns_total(),
                            samples_seen: 0,
                            error_seen: false,
                        };
                        let status = export.status(state.overruns_total());
                        state.network_export = Some(export);
                        state.send_dsp(stream, DspCommand::StartNetworkExport { tap });
                        inner.revision += 1;
                        NetworkExportCommit::Started(status)
                    }
                    Some(state) if state.rate_patches > 0 => NetworkExportCommit::Aborted {
                        tap,
                        writer,
                        patch_in_flight: true,
                    },
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
                    return Ok(status);
                }
                NetworkExportCommit::Aborted {
                    tap,
                    writer,
                    patch_in_flight,
                } => {
                    drop(tap);
                    join_network_writer(writer);
                    if patch_in_flight {
                        return Err(EngineError::NetworkExport(
                            "a sample-rate change is in flight; retry once it completes".to_owned(),
                        ));
                    }
                }
            }
        }
    }

    pub fn stop_network_export(
        &self,
        ds: u32,
        node: &str,
    ) -> Result<NetworkExportStatus, EngineError> {
        let (export, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(active) = state.network_export.as_ref() else {
                return Err(EngineError::NetworkExport(
                    "network export is not active".to_owned(),
                ));
            };
            if active.node != node {
                return Err(EngineError::NetworkExport(format!(
                    "network export belongs to node `{}`",
                    active.node
                )));
            }
            let Some(export) = state.network_export.take() else {
                return Err(EngineError::NetworkExport(
                    "network export vanished while stopping".to_owned(),
                ));
            };
            state.send_dsp(export.stream, DspCommand::StopNetworkExport);
            let overruns = state.overruns.clone();
            inner.revision += 1;
            (export, overruns)
        };
        let overruns_now: u64 = overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum();
        let mut export = export;
        export.join();
        let status = export.status(overruns_now);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }
}
