use super::*;
use crate::planning::plan_center;

fn tuned(center_hz: f64) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(center_hz),
        sample_rate: Some(2_400_000.0),
        ..DeviceSettings::default()
    }
}

fn settled(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    channels: &[ChannelInfo],
) -> f64 {
    plan_center(capabilities, settings, channels)
        .and_then(|delta| delta.center_hz)
        .unwrap_or_else(|| settings.center_hz.expect("a tuned radio"))
}

fn heard(center_hz: f64, channel: &ChannelInfo) -> bool {
    let (low, high) = sdrmm_channels::occupied_band(&channel.settings.params);
    crate::runtime::reaches(
        channel.settings.frequency_hz - center_hz,
        low,
        high,
        2_400_000.0,
    )
}

fn clears(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    channels: &[ChannelInfo],
) -> bool {
    let plan = plan_front_end(capabilities, settings, channels);
    artifact_clears_channels(plan.lo_offset_hz, settings, capabilities, channels)
}

fn resolved(settings: &DeviceSettings, delta: Option<DeviceSettings>) -> DeviceSettings {
    let mut merged = settings.clone();
    if let Some(delta) = delta {
        merged.merge_from(&delta);
    }
    merged
}

#[test]
fn a_radio_with_nothing_wired_to_it_is_left_where_it_was() {
    assert_eq!(plan_center(&tuner_caps(), &tuned(100e6), &[]), None);
}

#[test]
fn the_window_lands_over_every_decoder_that_fits_in_it() {
    let wired = [parked(1, -1e6), parked(2, 0.0), parked(3, 1e6)];
    let center_hz = settled(&tuner_caps(), &tuned(90e6), &wired);
    for channel in &wired {
        assert!(
            heard(center_hz, channel),
            "{} Hz left a decoder outside the window at {center_hz} Hz",
            channel.settings.frequency_hz
        );
    }
}

#[test]
fn a_front_end_the_engine_does_not_know_is_not_second_guessed_for_a_spike() {
    let alone = [parked(1, 0.0)];
    assert_eq!(
        settled(&tuner_caps(), &tuned(100e6), &alone),
        TEST_CENTER_HZ,
        "an unrecognised receiver was moved off a decoder to dodge a term it may not even have"
    );
}

#[test]
fn a_window_with_nowhere_left_to_park_its_artifact_steps_aside_itself() {
    let blocked: Vec<ChannelInfo> = [
        0.0, 600e3, -600e3, 450e3, -450e3, 750e3, -750e3, 300e3, -300e3,
    ]
    .into_iter()
    .enumerate()
    .map(|(at, offset_hz)| parked(at as u32 + 1, -offset_hz))
    .collect();
    let settings = DeviceSettings {
        sample_rate: Some(2_400_000.0),
        ..untouched_settings()
    };
    assert!(
        !clears(&managed_caps(), &settings, &blocked),
        "the artifact had somewhere to go without the window moving"
    );

    let center_hz = settled(&managed_caps(), &settings, &blocked);
    for channel in &blocked {
        assert!(heard(center_hz, channel), "settled on {center_hz} Hz");
    }
    assert!(
        clears(&managed_caps(), &tuned(center_hz), &blocked),
        "the window settled with its own artifact still inside a decoder at {center_hz} Hz"
    );
}

#[test]
fn a_crowd_too_wide_for_one_radio_keeps_as_many_decoders_as_the_window_holds() {
    let scattered = [
        parked(1, 0.0),
        parked(2, 500_000.0),
        parked(3, 1_000_000.0),
        parked(4, 20_000_000.0),
    ];
    let center_hz = settled(&tuner_caps(), &tuned(100e6), &scattered);
    let carried = scattered.iter().filter(|c| heard(center_hz, c)).count();
    assert_eq!(carried, 3, "settled on {center_hz} Hz");
    assert!(!heard(center_hz, &scattered[3]));
}

#[test]
fn a_radio_already_over_its_decoders_is_not_retuned_for_nothing() {
    let wired = [parked(1, -1e6), parked(2, 1e6)];
    let settled_hz = settled(&tuner_caps(), &tuned(100e6), &wired);
    assert_eq!(
        plan_center(&tuner_caps(), &tuned(settled_hz), &wired),
        None,
        "the radio moved again after it had already settled"
    );
}

#[test]
fn a_decoder_the_tuner_cannot_reach_never_drags_the_radio_out_of_its_range() {
    let unreachable = [ChannelInfo {
        settings: ChannelSettings {
            frequency_hz: 12e9,
            ..nfm_settings(0.0)
        },
        ..parked(1, 0.0)
    }];
    let center_hz = settled(&tuner_caps(), &tuned(100e6), &unreachable);
    assert!(crate::planning::tuner_reaches(&tuner_caps(), center_hz));
}

#[test]
fn a_radio_that_tunes_each_stream_apart_follows_the_decoders_on_each() {
    let capabilities = Capabilities {
        rx_streams: 2,
        per_stream: StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
        },
        ..tuner_caps()
    };
    let wired = [
        parked(1, 0.0),
        ChannelInfo {
            stream: 1,
            settings: nfm_settings(40e6),
            ..parked(2, 0.0)
        },
    ];
    let settings = tuned(100e6);
    let settled = resolved(&settings, plan_center(&capabilities, &settings, &wired));
    for channel in &wired {
        let center_hz = center_of(&settled, channel.stream, &capabilities.per_stream);
        assert!(
            heard(center_hz, channel),
            "stream {} settled on {center_hz} Hz, away from its own decoder",
            channel.stream
        );
    }
}

#[tokio::test]
async fn a_decoder_added_off_the_window_pulls_an_auto_radio_over_to_it() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .add_channel(ds, 0, nfm_settings(1_100_000.0))
        .unwrap();

    let set = &engine.snapshot().device_sets[0];
    assert!(
        !set.channels[0].out_of_band,
        "auto tuning left the decoder outside the window"
    );
    assert_eq!(
        set.channels[0].settings.frequency_hz,
        TEST_CENTER_HZ + 1_100_000.0,
        "the radio moved the decoder instead of moving itself"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_held_radio_stays_where_the_operator_put_it() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    hold_tuning(&engine, ds);
    let before = engine.snapshot().device_sets[0].settings.center_hz;
    engine.add_channel(ds, 0, nfm_settings(900_000.0)).unwrap();
    assert_eq!(engine.snapshot().device_sets[0].settings.center_hz, before);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_decoder_beyond_the_window_is_left_silent_rather_than_costing_the_others() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    for offset_hz in [0.0, 400_000.0, 20_000_000.0] {
        engine.add_channel(ds, 0, nfm_settings(offset_hz)).unwrap();
    }
    let set = &engine.snapshot().device_sets[0];
    let silent = set.channels.iter().filter(|c| c.out_of_band).count();
    assert_eq!(silent, 1, "the radio gave up a decoder it could have kept");
    assert!(set.channels[2].out_of_band);
    engine.remove_device_set(ds).unwrap();
}
