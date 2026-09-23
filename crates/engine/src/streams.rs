use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
};

use sdrmm_channels::{ChannelError, ClickProfile};
use sdrmm_device::DeviceError;
use sdrmm_wire::{
    AudioProcessing, AudioRoute, ChannelDescriptor, ChannelInfo, ChannelLevel, DeviceSetStatus,
    HuntSettings, HuntStatus, PlaybackRequest, PlaybackStatus, ScanSettings, ScannerStatus,
    ServerEvent, StateScope,
};
use tokio::sync::broadcast;

use crate::{
    AudioPacket, DspCommand, Engine, EngineError, IqBlock, PcmBlock, SpectrumSnapshot, SymbolBlock,
    VideoPacket,
    audio_fx::{AudioFxHub, FxSource},
    hunt, lock_runtime,
    planning::{descriptor_for, plan_center},
    sample_rate_of, scanner,
};

impl Engine {
    pub fn pipeline_health(&self) -> Vec<sdrmm_wire::PipelineQueue> {
        let (runtimes, mut queues): (Vec<_>, Vec<_>) = {
            let inner = self.lock();
            let runtimes = inner
                .device_sets
                .iter()
                .map(|(id, state)| (*id, state.runtime.clone()))
                .collect();
            let queues = inner
                .device_sets
                .iter()
                .flat_map(|(device_set, state)| {
                    state.channels.iter().filter_map(|channel| {
                        let media = state.media.get(&channel.id)?;
                        Some(sdrmm_wire::PipelineQueue {
                            device_set: *device_set,
                            stream: channel.stream,
                            channel: Some(channel.id),
                            stage: sdrmm_wire::PipelineStage::Channel,
                            health: media.sinks.publication.snapshot(),
                        })
                    })
                })
                .collect();
            (runtimes, queues)
        };
        for (id, runtime) in runtimes {
            queues.extend(lock_runtime(&runtime).queue_health(id));
        }
        queues
    }

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
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.audio_tx.subscribe())
    }

    pub fn subscribe_pcm(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<PcmBlock>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.pcm_tx.subscribe())
    }

    pub fn set_audio_fx(&self, node: &str, settings: AudioProcessing) -> Result<(), EngineError> {
        settings
            .validate()
            .map_err(|reason| EngineError::from(ChannelError::InvalidSettings(reason)))?;
        self.fx_hub().set(node, settings);
        Ok(())
    }

    pub fn retain_audio_fx(&self, nodes: &HashSet<String>) {
        self.fx_hub().retain(nodes);
    }

    pub fn subscribe_route_audio(
        &self,
        route: &AudioRoute,
    ) -> Result<broadcast::Receiver<AudioPacket>, EngineError> {
        if route.fx.is_empty() {
            return self.subscribe_audio(route.device_set, route.channel);
        }
        check_route(route)?;
        self.fx_hub()
            .subscribe_audio(route, || self.fx_source(route))
    }

    pub fn subscribe_route_pcm(
        &self,
        route: &AudioRoute,
    ) -> Result<broadcast::Receiver<PcmBlock>, EngineError> {
        if route.fx.is_empty() {
            return self.subscribe_pcm(route.device_set, route.channel);
        }
        check_route(route)?;
        self.fx_hub().subscribe_pcm(route, || self.fx_source(route))
    }

    pub(crate) fn fx_hub(&self) -> std::sync::MutexGuard<'_, AudioFxHub> {
        self.audio_fx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn fx_source(&self, route: &AudioRoute) -> Result<FxSource, EngineError> {
        let inner = self.lock();
        let (ds, ch) = (route.device_set, route.channel);
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let info = state
            .channels
            .iter()
            .find(|c| c.id == ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        if !descriptor_for(&info.settings.params)?.has_audio {
            return Err(EngineError::Audio(format!(
                "`{}` channels produce no audio for audio FX",
                info.settings.params.type_id()
            )));
        }
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(FxSource {
            pcm: handle.sinks.pcm_tx.subscribe(),
            channels: sdrmm_channels::audio_channels(&info.settings.params),
            profile: ClickProfile::for_params(&info.settings.params),
            capacity: crate::audio::pcm_channel_cap(sample_rate_of(&state.settings)),
        })
    }

    pub fn subscribe_video(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<VideoPacket>, EngineError> {
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
        let descriptor = descriptor_for(&info.settings.params)?;
        if !descriptor.has_video {
            return Err(ChannelError::InvalidSettings(format!(
                "{} produces no video",
                descriptor.name
            ))
            .into());
        }
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.video_tx.subscribe())
    }

    #[must_use]
    pub fn channel_levels(&self, ds: u32) -> Vec<ChannelLevel> {
        let inner = self.lock();
        let Some(state) = inner.device_sets.get(&ds) else {
            return Vec::new();
        };
        state
            .channels
            .iter()
            .filter_map(|channel| {
                let media = state.media.get(&channel.id)?;
                Some(ChannelLevel {
                    channel: channel.id,
                    level_db: f32::from_bits(media.sinks.level_db.load(Ordering::Relaxed)),
                    peak_db: f32::from_bits(media.sinks.peak_db.load(Ordering::Relaxed)),
                    squelch_db: Some(f32::from_bits(
                        media.sinks.squelch_db.load(Ordering::Relaxed),
                    ))
                    .filter(|db| db.is_finite()),
                })
            })
            .collect()
    }

    #[must_use]
    pub fn device_sets_with_channels(&self) -> Vec<u32> {
        let inner = self.lock();
        inner
            .device_sets
            .iter()
            .filter(|(_, state)| !state.channels.is_empty())
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn subscribe_iq(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<IqBlock>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.iq_tx.subscribe())
    }

    pub fn subscribe_symbols(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<SymbolBlock>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.symbol_tx.subscribe())
    }

    #[must_use]
    pub fn channel_types(&self) -> Vec<ChannelDescriptor> {
        crate::channel_types()
    }

    pub fn start_scan(
        self: &Arc<Self>,
        ds: u32,
        settings: ScanSettings,
    ) -> Result<ScannerStatus, EngineError> {
        let _edit = sdrmm_device::lock(&self.array_edits);
        self.check_array_scan(ds)?;
        scanner::session::start(self, ds, settings)
    }

    pub fn stop_scan(&self, ds: u32, channel: u32) -> Result<ScannerStatus, EngineError> {
        scanner::session::stop(self, ds, channel)
    }

    pub fn skip_scan(&self, ds: u32, channel: u32) -> Result<ScannerStatus, EngineError> {
        scanner::session::skip(self, ds, channel)
    }

    /// Parks the radio on one frequency and streams how strong it is, fast enough to walk with.
    pub fn start_hunt(
        self: &Arc<Self>,
        ds: u32,
        settings: HuntSettings,
    ) -> Result<HuntStatus, EngineError> {
        let _edit = sdrmm_device::lock(&self.array_edits);
        self.check_array_scan(ds)?;
        hunt::start(self, ds, settings)
    }

    pub fn stop_hunt(&self, ds: u32, channel: u32) -> Result<HuntStatus, EngineError> {
        hunt::stop(self, ds, channel)
    }

    pub fn control_playback(
        &self,
        ds: u32,
        request: &PlaybackRequest,
    ) -> Result<PlaybackStatus, EngineError> {
        let status = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let playback = state.playback.as_deref().ok_or_else(|| {
                EngineError::Device(DeviceError::Unsupported(
                    "this device is a radio, not a recording: there is nothing to seek in a \
                     signal that is still arriving"
                        .to_string(),
                ))
            })?;
            playback.control(request);
            let status = playback.status();
            inner.revision += 1;
            status
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }

    #[must_use]
    pub fn sweeps_in_firmware(&self, ds: u32) -> bool {
        self.lock()
            .device_sets
            .get(&ds)
            .is_some_and(|state| state.capabilities.hardware_sweep)
    }

    pub(crate) fn scan_sample_rate(&self, ds: u32) -> Option<f64> {
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        (state.status == DeviceSetStatus::Running).then(|| sample_rate_of(&state.settings))
    }

    pub(crate) fn scan_reaches(&self, ds: u32, ch: u32, hz: f64) -> Result<bool, EngineError> {
        let arrayed = !self.arrays_using(ds).is_empty();
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
        let mut moved = channel.settings.clone();
        moved.frequency_hz = hz;
        if state.hears(channel.stream, &moved) {
            return Ok(true);
        }
        let scope = state.capabilities.per_stream;
        let follows = !arrayed
            && state.tunes_freely()
            && state
                .settings
                .for_stream(channel.stream, &scope)
                .tunes_itself();
        if !follows {
            return Ok(false);
        }
        let channels: Vec<ChannelInfo> = state
            .channels
            .iter()
            .map(|c| {
                if c.id == ch {
                    ChannelInfo {
                        settings: moved.clone(),
                        ..c.clone()
                    }
                } else {
                    c.clone()
                }
            })
            .collect();
        let mut settled = state.settings.clone();
        if let Some(delta) = plan_center(&state.capabilities, &settled, &channels) {
            settled.merge_from(&delta);
        }
        Ok(state.hears_with(&settled, channel.stream, &moved))
    }

    pub(crate) fn decoder_of(&self, ds: u32, ch: u32) -> Option<hunt::Decoder> {
        let inner = self.lock();
        inner
            .device_sets
            .get(&ds)?
            .channels
            .iter()
            .find(|channel| channel.id == ch)
            .map(hunt::Decoder::of)
    }

    pub(crate) fn scan_tune_channel(
        &self,
        ds: u32,
        ch: u32,
        frequency_hz: f64,
    ) -> Result<bool, EngineError> {
        {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let info = state
                .channels
                .iter_mut()
                .find(|c| c.id == ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            if info.settings.frequency_hz == frequency_hz {
                return Ok(false);
            }
            info.settings.frequency_hz = frequency_hz;
            let stream = info.stream;
            state.send_dsp(
                stream,
                DspCommand::RetuneChannel {
                    id: ch,
                    frequency_hz,
                },
            );
            inner.revision += 1;
        }
        let moved = self.settle_tuning(ds);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(moved)
    }

    pub fn subscribe_spectrum(
        &self,
        ds: u32,
        stream: u32,
    ) -> Result<broadcast::Receiver<SpectrumSnapshot>, EngineError> {
        let (runtime, streams) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            (state.runtime.clone(), state.rx_streams())
        };
        lock_runtime(&runtime)
            .subscribe(stream)
            .ok_or(EngineError::StreamOutOfRange { stream, streams })
    }
}

fn check_route(route: &AudioRoute) -> Result<(), EngineError> {
    route
        .validate()
        .map_err(|reason| EngineError::from(ChannelError::InvalidSettings(reason)))
}
