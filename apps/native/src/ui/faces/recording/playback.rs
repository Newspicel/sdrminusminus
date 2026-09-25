use std::time::{Duration, Instant};

use sdrmm_wire::{
    PlaybackAction, PlaybackRequest,
    device::{DeviceSettings, ExtraValue},
    state::{DeviceSet, PlaybackStatus},
};
use zgui::{prelude::*, reactive::RenderEffect};

use crate::{
    store::Store,
    ui::{
        faces::device::radio::{LOOP_SETTING, Radio},
        kit_sources::{debounced, icon_button, icons, units},
        widgets::slide,
    },
};

const TICK: Duration = Duration::from_millis(200);

#[must_use]
pub fn playback_position_at(
    status: &PlaybackStatus,
    elapsed_ms: f64,
    sample_rate: f64,
    looping: bool,
) -> f64 {
    let total = status.total_samples as f64;
    let reported = (status.position_samples as f64).min(total);
    if status.paused || sample_rate <= 0.0 || status.total_samples == 0 {
        return reported;
    }
    let advanced = reported + elapsed_ms.max(0.0) / 1000.0 * sample_rate;
    if advanced < total {
        advanced
    } else if looping {
        advanced % total
    } else {
        total
    }
}

#[must_use]
pub fn samples_to_seconds(samples: f64, sample_rate: f64) -> f64 {
    if sample_rate > 0.0 {
        samples / sample_rate
    } else {
        0.0
    }
}

#[must_use]
pub fn is_looping(set: &DeviceSet) -> bool {
    set.settings
        .extra
        .iter()
        .find(|extra| extra.name == LOOP_SETTING)
        .and_then(|extra| extra.value.as_bool())
        .unwrap_or(true)
}

fn drive(store: Store, set: u32, action: PlaybackAction, position_samples: Option<u64>) {
    zgui::task::spawn_local(async move {
        let request = PlaybackRequest {
            action,
            position_samples,
        };
        let sent: anyhow::Result<PlaybackStatus> = store
            .api()
            .post(&format!("/api/devicesets/{set}/playback"), &request)
            .await;
        if let Err(error) = sent {
            store.say(format!("cannot drive the playback: {error}"));
        }
        store.refresh_state();
    });
}

pub fn transport(radio: Radio) -> impl IntoView {
    let store = radio.store;
    let status = Memo::new(move |_| radio.read(|set| set.playback).flatten());
    let id = Memo::new(move |_| radio.read(|set| set.id));
    let rate = Signal::derive(move || {
        radio
            .read(|set| set.settings.sample_rate.unwrap_or(0.0))
            .unwrap_or(0.0)
    });
    let looping = Signal::derive(move || radio.read(is_looping).unwrap_or(true));
    let paused = Signal::derive(move || status.get().is_none_or(|status| status.paused));
    let anchor = RwSignal::new((0u64, Instant::now()));
    let follow = RenderEffect::new(move |_| {
        if let Some(status) = status.get() {
            anchor.set((status.position_samples, Instant::now()));
        }
    });
    on_cleanup_local(move || drop(follow));
    let now = RwSignal::new(Instant::now());
    let ticker = set_interval(TICK, move || {
        if !paused.get_untracked() {
            now.set(Instant::now());
        }
    });
    let ticker = StoredValue::new_local(Some(ticker));
    on_cleanup_local(move || drop(ticker.try_update_value(Option::take)));
    let live = Signal::derive(move || {
        let Some(mut status) = status.get() else {
            return 0.0;
        };
        let (position, at) = anchor.get();
        status.position_samples = position;
        let elapsed = now.get().saturating_duration_since(at).as_secs_f64() * 1000.0;
        playback_position_at(&status, elapsed, rate.get(), looping.get())
    });
    let total = Signal::derive(move || {
        status
            .get()
            .map_or(1.0, |status| status.total_samples.max(1) as f64)
    });
    let seek = move |target: f64| {
        if let Some(set) = id.get_untracked() {
            drive(
                store,
                set,
                PlaybackAction::Seek,
                Some(target.round().max(0.0) as u64),
            );
        }
    };
    let (scrub, change) = debounced(seek);
    let position = Signal::derive(move || scrub.get().unwrap_or_else(|| live.get()));
    let act = move |action: PlaybackAction| {
        if let Some(set) = id.get_untracked() {
            drive(store, set, action, None);
        }
    };
    let clock = move |at: f64| {
        let rate = rate.get_untracked();
        format!(
            "{} / {}",
            units::clock(samples_to_seconds(at, rate)),
            units::clock(samples_to_seconds(total.get_untracked(), rate))
        )
    };
    let playing = Signal::derive(move || !paused.get());
    view! {
        row(class = "kit-transport") {
            {move || {
                let label = if paused.get() { "Play" } else { "Pause" };
                let svg = if paused.get() { icons::PLAY } else { icons::PAUSE };
                icon_button(svg, label, playing, Signal::stored(false), move || act(if paused.get_untracked() { PlaybackAction::Play } else { PlaybackAction::Pause }))
            }}
            {icon_button(icons::STOP, "Stop", Signal::stored(false), Signal::stored(false), move || act(PlaybackAction::Stop))}
            {icon_button(icons::LOOP, "Loop", looping, Signal::stored(false), move || {
                radio.patch(DeviceSettings {
                    extra: vec![ExtraValue { name: LOOP_SETTING.to_owned(), value: serde_json::Value::Bool(!looping.get_untracked()) }],
                    ..DeviceSettings::default()
                });
            })}
            {move || slide(position, 0.0, total.get(), clock, change.clone())}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn status(position_samples: u64, total_samples: u64, paused: bool) -> PlaybackStatus {
        PlaybackStatus {
            position_samples,
            total_samples,
            paused,
        }
    }

    fn set(extra: serde_json::Value) -> DeviceSet {
        serde_json::from_value(json!({
            "id": 1, "status": "running",
            "device": { "driver": "virtual", "key": "file:rec_1", "label": "rec_1" },
            "settings": { "sample_rate": 48_000.0, "extra": extra },
            "capabilities": { "antennas": [], "bandwidths": [], "freq_ranges": [], "gains": [], "sample_rates": [] },
            "channels": []
        }))
        .expect("device set")
    }

    #[test]
    fn the_position_runs_on_the_clock_between_snapshots() {
        assert_eq!(
            playback_position_at(&status(1_000, 48_000, false), 500.0, 48_000.0, true),
            25_000.0
        );
        assert_eq!(
            playback_position_at(&status(1_000, 48_000, true), 60_000.0, 48_000.0, true),
            1_000.0
        );
        assert_eq!(
            playback_position_at(&status(0, 48_000, false), 1_500.0, 48_000.0, true),
            24_000.0
        );
        assert_eq!(
            playback_position_at(&status(0, 48_000, false), 5_000.0, 48_000.0, false),
            48_000.0
        );
    }

    #[test]
    fn the_position_never_runs_backwards_or_past_the_recording() {
        assert_eq!(
            playback_position_at(&status(100, 48_000, false), -5_000.0, 48_000.0, true),
            100.0
        );
        assert_eq!(
            playback_position_at(&status(100, 48_000, false), 5_000.0, 0.0, true),
            100.0
        );
        assert_eq!(
            playback_position_at(&status(0, 0, false), 5_000.0, 48_000.0, true),
            0.0
        );
        assert_eq!(
            playback_position_at(&status(99_999, 48_000, true), 0.0, 48_000.0, false),
            48_000.0
        );
    }

    #[test]
    fn samples_convert_at_the_rate_of_the_set() {
        assert_eq!(samples_to_seconds(96_000.0, 48_000.0), 2.0);
        assert_eq!(samples_to_seconds(96_000.0, 0.0), 0.0);
    }

    #[test]
    fn looping_reads_the_loop_extra_and_defaults_to_on() {
        assert!(!is_looping(&set(
            json!([{ "name": "loop", "value": false }])
        )));
        assert!(is_looping(&set(json!([{ "name": "loop", "value": true }]))));
        assert!(is_looping(&set(json!([]))));
        assert!(is_looping(&set(
            json!([{ "name": "loop", "value": "yes" }])
        )));
    }
}
