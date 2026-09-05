use sdrmm_device::DeviceError;
use sdrmm_wire::{ChannelInfo, ChannelSettings, DeviceSettings, ServerEvent, StateScope};

use crate::{
    ChannelAudioRecording, ChannelMedia, Engine, EngineError, RebuildEntry,
    planning::{descriptor_for, tuner_reaches, validate_channel, validate_streams},
    runtime::{ChannelHost, DspCommand},
    sample_rate_of,
    sinks::BasebandSinks,
};

impl Engine {
    pub(crate) fn rebuild_channel(
        &self,
        ds: u32,
        rebuild: RebuildEntry,
        rate: f64,
        dead: &mut Vec<ChannelMedia>,
    ) {
        let RebuildEntry {
            id,
            stream,
            mut settings,
            sinks,
        } = rebuild;
        let mut built_rate = rate;
        loop {
            let built = descriptor_for(&settings.params)
                .and_then(|d| validate_channel(&d, &settings, built_rate))
                .and_then(|()| {
                    ChannelHost::build(
                        built_rate,
                        &settings,
                        sinks.clone(),
                        self.decoded_sink(ds, id),
                    )
                    .map_err(EngineError::from)
                });
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            let current_rate = sample_rate_of(&state.settings);
            let Some(info) = state.channels.iter().find(|c| c.id == id) else {
                return;
            };
            if current_rate != built_rate || info.settings != settings {
                settings = info.settings.clone();
                built_rate = current_rate;
                continue;
            }
            let orphaned = state.release_baseband_sinks(id, stream);
            match built {
                Ok(mut host) => {
                    if let Some(media) = state.media.get(&id) {
                        host.position_changed(media.position.as_ref());
                    }
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    state.send_dsp(stream, DspCommand::AddChannel { id, host });
                    state.rearm_audio_recording(id, stream);
                    inner.revision += 1;
                    drop(inner);
                    self.close_baseband_sinks(ds, id, orphaned, "the channel was rebuilt");
                }
                Err(e) => {
                    tracing::error!(ds, channel = id, error = %e, "channel rebuild failed after rate change; removing channel");
                    state.channels.retain(|c| c.id != id);
                    dead.extend(state.media.remove(&id));
                    let recording = state.audio_recordings.remove(&id);
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    inner.revision += 1;
                    drop(inner);
                    if let Some(recording) = recording {
                        recording.join();
                    }
                    self.close_baseband_sinks(ds, id, orphaned, "the channel was removed");
                }
            }
            return;
        }
    }

    pub fn validate_configuration(
        &self,
        ds: u32,
        settings: &DeviceSettings,
        channels: &[ChannelSettings],
    ) -> Result<(), EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let rate = settings
            .sample_rate
            .unwrap_or_else(|| sample_rate_of(&state.settings));
        let caps = &state.capabilities;
        if let Some(center) = settings.center_hz
            && !tuner_reaches(caps, center)
        {
            return Err(DeviceError::Unsupported(format!(
                "{center} Hz is outside this device's tuning range"
            ))
            .into());
        }
        validate_streams(caps, settings)?;
        for channel in channels {
            let descriptor = descriptor_for(&channel.params)?;
            validate_channel(&descriptor, channel, rate)?;
        }
        Ok(())
    }

    pub fn add_channel(
        &self,
        ds: u32,
        stream: u32,
        settings: ChannelSettings,
    ) -> Result<u32, EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        let (mut device_rate, id) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.check_stream(stream)?;
            let id = state.next_channel_id;
            state.next_channel_id += 1;
            (sample_rate_of(&state.settings), id)
        };
        let created = ChannelMedia::new(sdrmm_channels::audio_channels(&settings.params))?;
        let sinks = created.sinks.clone();
        let mut media = Some(created);

        let staged = loop {
            let built = validate_channel(&descriptor, &settings, device_rate).and_then(|()| {
                ChannelHost::build(
                    device_rate,
                    &settings,
                    sinks.clone(),
                    self.decoded_sink(ds, id),
                )
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
            if let Err(e) = state.check_stream(stream) {
                break Err(e);
            }
            let current_rate = sample_rate_of(&state.settings);
            if current_rate != device_rate {
                device_rate = current_rate;
                continue;
            }
            state.channels.push(ChannelInfo {
                id,
                stream,
                settings: settings.clone(),
                audio_recording: None,
                baseband_recording: None,
                network_export: None,
            });
            if let Some(handle) = media.take() {
                state.media.insert(id, handle);
            }
            state.send_dsp(stream, DspCommand::AddChannel { id, host });
            inner.revision += 1;
            break Ok(id);
        };
        let id = match staged {
            Ok(id) => id,
            Err(e) => {
                drop(sinks);
                if let Some(handle) = media.take() {
                    handle.shutdown();
                }
                return Err(e);
            }
        };
        self.replace_lo(ds);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(id)
    }

    pub fn patch_channel(
        &self,
        ds: u32,
        ch: u32,
        settings: ChannelSettings,
    ) -> Result<(), EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        let (old, sinks, mut device_rate) = {
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
                .media
                .get(&ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            (
                info.settings.clone(),
                handle.sinks.clone(),
                sample_rate_of(&state.settings),
            )
        };
        let mut need_host = old.offset_hz != settings.offset_hz
            || old.params != settings.params
            || old.squelch_db != settings.squelch_db
            || old.squelch_auto_db != settings.squelch_auto_db
            || old.audio != settings.audio;
        let mut orphaned: Option<ChannelAudioRecording> = None;
        let mut orphaned_baseband = BasebandSinks::default();
        let staged = loop {
            if let Err(e) = validate_channel(&descriptor, &settings, device_rate) {
                break Err(e);
            }
            let host = if need_host {
                match ChannelHost::build(
                    device_rate,
                    &settings,
                    sinks.clone(),
                    self.decoded_sink(ds, ch),
                ) {
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
            if info.settings != old && host.is_none() {
                need_host = true;
                continue;
            }
            let stream = info.stream;
            let prev = std::mem::replace(&mut info.settings, settings.clone());
            if let Some(mut host) = host {
                if let Some(media) = state.media.get(&ch) {
                    host.position_changed(media.position.as_ref());
                }
                if prev.params.type_id() != settings.params.type_id() {
                    orphaned_baseband = state.release_baseband_sinks(ch, stream);
                    state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
                }
                state.send_dsp(stream, DspCommand::AddChannel { id: ch, host });
                if descriptor.has_audio {
                    state.rearm_audio_recording(ch, stream);
                } else {
                    orphaned = state.audio_recordings.remove(&ch);
                }
            }
            inner.revision += 1;
            break Ok(());
        };
        if let Some(recording) = orphaned {
            tracing::info!(
                ds,
                channel = ch,
                "channel audio recording finished: the channel no longer produces audio"
            );
            recording.join();
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        self.close_baseband_sinks(ds, ch, orphaned_baseband, "the channel was rebuilt");
        staged?;
        self.replace_lo(ds);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    pub fn remove_channel(&self, ds: u32, ch: u32) -> Result<(), EngineError> {
        let (handle, recording, baseband) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let stream = state
                .channels
                .iter()
                .find(|c| c.id == ch)
                .map(|c| c.stream)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            state.channels.retain(|c| c.id != ch);
            let handle = state.media.remove(&ch);
            let recording = state.audio_recordings.remove(&ch);
            let baseband = state.release_baseband_sinks(ch, stream);
            state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
            inner.revision += 1;
            (handle, recording, baseband)
        };
        if let Some(recording) = recording {
            recording.join();
        }
        self.close_baseband_sinks(ds, ch, baseband, "the channel was removed");
        if let Some(handle) = handle {
            handle.shutdown();
        }
        self.replace_lo(ds);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }
}
