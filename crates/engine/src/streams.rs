use std::sync::{Arc, atomic::Ordering};

use sdrmm_channels::ChannelError;
use sdrmm_device::DeviceError;
use sdrmm_wire::{
    ChannelDescriptor, ChannelLevel, ChannelSettings, DeviceSetStatus, DeviceSettings,
    HuntSettings, HuntStatus, PlaybackRequest, PlaybackStatus, ScanSession, ScanSessionStatus,
    ScanSettings, ScannerStatus, ServerEvent, StateScope,
};
use tokio::sync::broadcast;

use crate::{
    AudioPacket, Engine, EngineError, IqBlock, PatchOrigin, PcmBlock, SpectrumSnapshot,
    SymbolBlock, VideoPacket, hunt, lock_runtime, planning::descriptor_for, sample_rate_of,
    scanner, scanner::session::SessionState,
};

impl Engine {
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
        sdrmm_channels::descriptors()
    }

    pub fn start_scan(
        self: &Arc<Self>,
        ds: u32,
        settings: ScanSettings,
    ) -> Result<ScannerStatus, EngineError> {
        let mut session = scanner::session::start(self, &[ds], settings)?;
        session
            .members
            .pop()
            .map(|member| member.status)
            .ok_or(EngineError::DeviceSetNotFound(ds))
    }

    /// Sweeps one plan with several radios at once, each taking a share of the targets.
    pub fn start_scan_session(
        self: &Arc<Self>,
        device_sets: &[u32],
        settings: ScanSettings,
    ) -> Result<ScanSessionStatus, EngineError> {
        scanner::session::start(self, device_sets, settings)
    }

    pub fn stop_scan(&self, ds: u32) -> Result<ScannerStatus, EngineError> {
        scanner::session::stop_one(self, ds)
    }

    pub fn stop_scan_session(&self) -> Result<ScanSessionStatus, EngineError> {
        scanner::session::stop_all(self)
    }

    /// Parks the radio on one frequency and streams how strong it is, fast enough to walk with.
    pub fn start_hunt(
        self: &Arc<Self>,
        ds: u32,
        settings: HuntSettings,
    ) -> Result<HuntStatus, EngineError> {
        hunt::start(self, ds, settings)
    }

    pub fn stop_hunt(&self, ds: u32) -> Result<HuntStatus, EngineError> {
        hunt::stop(self, ds)
    }

    #[must_use]
    pub fn scan_session(&self) -> Option<ScanSession> {
        self.lock().scan_session.as_ref().map(SessionState::project)
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

    pub(crate) fn scan_retune(
        &self,
        ds: u32,
        center_hz: f64,
    ) -> Result<broadcast::Receiver<SpectrumSnapshot>, EngineError> {
        self.patch_device_from(
            ds,
            DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
            PatchOrigin::Scan,
        )?;
        self.subscribe_spectrum(ds, 0)
    }

    pub(crate) fn scan_park_channel(
        &self,
        ds: u32,
        ch: u32,
        offset_hz: f64,
    ) -> Result<(), EngineError> {
        let settings = {
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
            ChannelSettings {
                offset_hz,
                ..info.settings.clone()
            }
        };
        self.patch_channel(ds, ch, settings)
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
