use sdrmm_wire::{
    device::{Capabilities, DeviceSettings, StreamSettings, Tuning},
    state::DeviceSet,
};

const WINDOW_SHARE: f64 = 0.45;

#[must_use]
pub fn reachable_hz(capabilities: &Capabilities, hz: f64) -> f64 {
    let ranges = &capabilities.freq_ranges;
    if ranges.is_empty() {
        return hz.max(0.0);
    }
    ranges
        .iter()
        .map(|range| hz.clamp(range.min, range.max.max(range.min)))
        .min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        .unwrap_or(hz)
}

#[must_use]
pub fn stream_settings(set: &DeviceSet, stream: u32) -> DeviceSettings {
    set.settings
        .for_stream(stream, &set.capabilities.per_stream)
}

#[must_use]
pub fn auto_tuning(set: &DeviceSet, stream: u32) -> bool {
    stream_settings(set, stream).tuning.unwrap_or_default() == Tuning::Auto
}

#[must_use]
pub fn center_hz(set: &DeviceSet, stream: u32) -> Option<f64> {
    stream_settings(set, stream).center_hz
}

#[must_use]
pub fn tune_delta(capabilities: &Capabilities, stream: u32, hz: f64) -> DeviceSettings {
    let center_hz = Some(reachable_hz(capabilities, hz));
    if capabilities.per_stream.tuning {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                center_hz,
                tuning: Some(Tuning::Manual),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    } else {
        DeviceSettings {
            center_hz,
            tuning: Some(Tuning::Manual),
            ..DeviceSettings::default()
        }
    }
}

#[must_use]
pub fn radio_pull(set: &DeviceSet, hz: f64) -> Option<DeviceSettings> {
    if auto_tuning(set, 0) {
        return None;
    }
    let center = center_hz(set, 0)?;
    let rate = set.settings.sample_rate?;
    let reach = rate * WINDOW_SHARE;
    ((hz - center).abs() > reach).then(|| tune_delta(&set.capabilities, 0, hz))
}

#[must_use]
pub fn same_mode(mode: Option<&str>, channel_type: Option<&str>) -> bool {
    match mode.filter(|mode| !mode.is_empty()) {
        None => true,
        Some(mode) => channel_type.is_some_and(|kind| kind.eq_ignore_ascii_case(mode)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(settings: serde_json::Value, ranges: serde_json::Value) -> DeviceSet {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "device": { "driver": "rtlsdr", "key": "0", "label": "RTL-SDR" },
            "capabilities": {
                "freq_ranges": ranges, "sample_rates": [], "gains": [],
                "antennas": [], "bandwidths": [], "duplex": "rx_only"
            },
            "settings": settings,
            "status": "running",
            "channels": [],
        }))
        .expect("a device set")
    }

    #[test]
    fn a_tune_holds_the_radio_to_its_nearest_range_and_goes_manual() {
        let radio = set(
            serde_json::json!({}),
            serde_json::json!([{ "min": 24e6, "max": 1766e6 }]),
        );
        let delta = tune_delta(&radio.capabilities, 0, 10e6);
        assert_eq!(delta.center_hz, Some(24e6));
        assert_eq!(delta.tuning, Some(Tuning::Manual));
        assert!(auto_tuning(&radio, 0));
    }

    #[test]
    fn a_manual_radio_follows_a_channel_that_leaves_its_window() {
        let manual = set(
            serde_json::json!({ "center_hz": 100e6, "sample_rate": 2e6, "tuning": "manual" }),
            serde_json::json!([]),
        );
        assert!(radio_pull(&manual, 100.5e6).is_none());
        assert_eq!(
            radio_pull(&manual, 105e6).and_then(|delta| delta.center_hz),
            Some(105e6)
        );
        let auto = set(
            serde_json::json!({ "center_hz": 100e6, "sample_rate": 2e6 }),
            serde_json::json!([]),
        );
        assert!(radio_pull(&auto, 105e6).is_none());
    }

    #[test]
    fn a_bookmark_without_a_mode_suits_any_channel() {
        assert!(same_mode(None, Some("nfm")));
        assert!(same_mode(Some(""), None));
        assert!(same_mode(Some("NFM"), Some("nfm")));
        assert!(!same_mode(Some("am"), Some("nfm")));
        assert!(!same_mode(Some("am"), None));
    }
}
