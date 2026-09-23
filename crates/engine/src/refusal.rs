use sdrmm_device::DeviceError;
use sdrmm_wire::{DeviceSettings, SettingsRefused};

pub(crate) fn refused(hardware: &DeviceSettings, error: &DeviceError) -> SettingsRefused {
    SettingsRefused {
        settings: setting_names(hardware),
        error: error.to_string(),
    }
}

fn setting_names(settings: &DeviceSettings) -> Vec<String> {
    let streams = &settings.streams;
    let flagged = [
        (
            "frequency",
            settings.center_hz.is_some() || streams.iter().any(|s| s.center_hz.is_some()),
        ),
        ("sample rate", settings.sample_rate.is_some()),
        ("bandwidth", settings.bandwidth.is_some()),
        (
            "gain",
            !settings.gains.is_empty() || streams.iter().any(|s| !s.gains.is_empty()),
        ),
        ("AGC", settings.agc.is_some()),
        (
            "antenna",
            settings.antenna.is_some() || streams.iter().any(|s| s.antenna.is_some()),
        ),
        ("PPM", settings.ppm.is_some()),
        ("bias tee", settings.bias_tee.is_some()),
    ];
    flagged
        .into_iter()
        .filter(|(_, set)| *set)
        .map(|(name, _)| name.to_string())
        .chain(settings.extra.iter().map(|extra| extra.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{ExtraValue, StreamSettings};

    use super::*;

    #[test]
    fn a_retune_is_named_as_the_frequency() {
        let tune = DeviceSettings {
            center_hz: Some(110e6),
            ..DeviceSettings::default()
        };
        let refused = refused(&tune, &DeviceError::Io("endpoint stalled".into()));
        assert_eq!(refused.settings, ["frequency"]);
        assert_eq!(refused.error, "device I/O error: endpoint stalled");
    }

    #[test]
    fn every_setting_in_the_patch_is_named_once() {
        let patch = DeviceSettings {
            sample_rate: Some(2.4e6),
            ppm: Some(1.0),
            extra: vec![ExtraValue {
                name: "direct_sampling".into(),
                value: "q".into(),
            }],
            streams: vec![
                StreamSettings {
                    stream: 0,
                    center_hz: Some(1e8),
                    ..StreamSettings::default()
                },
                StreamSettings {
                    stream: 1,
                    center_hz: Some(2e8),
                    ..StreamSettings::default()
                },
            ],
            ..DeviceSettings::default()
        };
        assert_eq!(
            setting_names(&patch),
            ["frequency", "sample rate", "PPM", "direct_sampling"]
        );
    }
}
