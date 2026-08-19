use std::ffi::c_int;

use sdrmm_device::DeviceError;
use sdrmm_wire::{Capabilities, DeviceSettings, ExtraSetting, GainStage, GainValue};

use crate::{
    api::{Cr8Api, DevHandle},
    caps, ffi,
};

/// One call the library has to be told to make, worked out before any of them are made so a
/// settings table that is wrong in its last field changes nothing at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Tune { channels: c_int, freq_hz: f64 },
    Lna { channels: c_int, gain: i32 },
    Mixer { channels: c_int, gain: i32 },
    Vga { channels: c_int, gain: i32 },
    Clock(c_int),
}

impl Step {
    pub fn run(self, api: &dyn Cr8Api, dev: DevHandle) -> Result<(), DeviceError> {
        match self {
            Self::Tune { channels, freq_hz } => api.set_freq(dev, channels, freq_hz, true),
            Self::Lna { channels, gain } => api.set_lna_gain(dev, channels, gain),
            Self::Mixer { channels, gain } => api.set_mixer_gain(dev, channels, gain),
            Self::Vga { channels, gain } => api.set_vga_gain(dev, channels, gain),
            Self::Clock(clock) => api.set_clock(dev, clock),
        }
    }
}

fn snapped(stage: &GainStage, value: f64) -> i32 {
    stage.snap(value).round() as i32
}

fn gain_steps(
    channels: c_int,
    gains: &[GainValue],
    stages: &[GainStage],
) -> Result<Vec<Step>, DeviceError> {
    let mut steps = Vec::new();
    for wanted in gains {
        let stage = stages
            .iter()
            .find(|stage| stage.name.eq_ignore_ascii_case(&wanted.stage))
            .ok_or_else(|| {
                DeviceError::Unsupported(format!("the CR-8 has no {} gain stage", wanted.stage))
            })?;
        let gain = snapped(stage, wanted.value_db);
        steps.push(match stage.name.as_str() {
            "LNA" => Step::Lna { channels, gain },
            "Mixer" => Step::Mixer { channels, gain },
            _ => Step::Vga { channels, gain },
        });
    }
    Ok(steps)
}

fn clock_step(
    settings: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<Vec<Step>, DeviceError> {
    let Some(value) = settings
        .extra
        .iter()
        .find(|extra| extra.name == caps::CLOCK_SETTING)
    else {
        return Ok(Vec::new());
    };
    let known = capabilities.extra.iter().any(
        |setting| matches!(setting, ExtraSetting::Enum { name, .. } if name == caps::CLOCK_SETTING),
    );
    if !known {
        return Ok(Vec::new());
    }
    match value.value.as_str() {
        Some(caps::CLOCK_INTERNAL) => Ok(vec![Step::Clock(ffi::CLOCK_INTERNAL)]),
        Some(caps::CLOCK_EXTERNAL) => Ok(vec![Step::Clock(ffi::CLOCK_EXTERNAL)]),
        other => Err(DeviceError::Unsupported(format!(
            "clock source {}",
            other.unwrap_or("(not a name)")
        ))),
    }
}

/// What a settings table means for a CR-8, in the order it has to be done.
///
/// Every channel is tuned in one coherent call: eight receivers on one frequency is what the
/// radio is for, and tuning them apart would throw away the phase alignment the library just did.
pub fn plan(
    settings: &DeviceSettings,
    current: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<Vec<Step>, DeviceError> {
    let mut steps = clock_step(settings, capabilities)?;
    if let Some(rate) = settings.sample_rate
        && (rate - ffi::SAMPLE_RATE_HZ).abs() > 1.0
    {
        return Err(DeviceError::Unsupported(format!(
            "the CR-8 runs at {} MS/s and nothing else, not {} MS/s",
            ffi::SAMPLE_RATE_HZ / 1e6,
            rate / 1e6
        )));
    }
    let centre = settings.center_hz.or(current.center_hz);
    if let Some(freq_hz) = centre
        && settings.center_hz.is_some()
    {
        steps.push(Step::Tune {
            channels: ffi::CHAN_ALL,
            freq_hz,
        });
    }
    steps.extend(gain_steps(
        ffi::CHAN_ALL,
        &settings.gains,
        &capabilities.gains,
    )?);
    for stream in &settings.streams {
        let channels = crate::channel_mask(stream.stream as usize);
        if channels == 0 {
            return Err(DeviceError::Unsupported(format!(
                "the CR-8 has {} channels, not stream {}",
                ffi::CHANNEL_COUNT,
                stream.stream
            )));
        }
        steps.extend(gain_steps(channels, &stream.gains, &capabilities.gains)?);
    }
    Ok(steps)
}
