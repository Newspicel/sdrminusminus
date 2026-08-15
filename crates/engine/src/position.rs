use sdrmm_wire::PositionFix;

use crate::{Engine, EngineError, recording, runtime::DspCommand};

impl Engine {
    pub fn update_channel_position(
        &self,
        ds: u32,
        ch: u32,
        fix: Option<PositionFix>,
    ) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let stream = state
            .channels
            .iter()
            .find(|channel| channel.id == ch)
            .map(|channel| channel.stream)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        let media = state
            .media
            .get_mut(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        media.position = fix.clone();
        state.send_dsp(stream, DspCommand::PositionChanged { id: ch, fix });
        Ok(())
    }

    pub fn update_recording_position(
        &self,
        ds: u32,
        fix: Option<PositionFix>,
    ) -> Result<(), EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let recording = state
            .recording
            .as_ref()
            .ok_or_else(|| EngineError::Recording("not recording".to_owned()))?;
        let position = recording
            .position
            .as_ref()
            .ok_or_else(|| EngineError::Recording("recording is stopping".to_owned()))?;
        position.update(fix).map_err(|error| match error {
            recording::PositionUpdateError::Full => {
                EngineError::Recording("recording queue full — disk too slow?".to_owned())
            }
            recording::PositionUpdateError::Disconnected => {
                EngineError::Recording("recording writer stopped".to_owned())
            }
        })
    }
}
