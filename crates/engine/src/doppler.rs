use crate::{Engine, EngineError, runtime::DspCommand};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Doppler {
    pub shift_hz: f64,
    pub rate_hz_s: f64,
}

impl Engine {
    pub fn steer_channel(&self, ds: u32, ch: u32, doppler: Doppler) -> Result<(), EngineError> {
        if !doppler.shift_hz.is_finite() || !doppler.rate_hz_s.is_finite() {
            return Err(EngineError::Channel(
                sdrmm_channels::ChannelError::InvalidSettings(
                    "a Doppler correction has to be finite".to_owned(),
                ),
            ));
        }
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let stream = state
            .channels
            .iter()
            .find(|channel| channel.id == ch)
            .map(|channel| channel.stream)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        state.send_dsp(stream, DspCommand::SteerChannel { id: ch, doppler });
        Ok(())
    }

    pub fn tune_channel(&self, ds: u32, ch: u32, frequency_hz: f64) -> Result<bool, EngineError> {
        if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
            return Err(EngineError::Channel(
                sdrmm_channels::ChannelError::InvalidSettings(
                    "a channel frequency has to be positive".to_owned(),
                ),
            ));
        }
        self.scan_tune_channel(ds, ch, frequency_hz)
    }
}
