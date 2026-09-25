use std::time::Duration;

use sdrmm_wire::{
    channel::ChannelInfo,
    device::{DeviceSettings, ExtraValue},
    patch::NodeBody,
    rest::{PlaybackAction, PlaybackRequest},
    state::{AudioRecordingStatus, DeviceSet, PlaybackStatus, RecordingStatus},
};
use zgui::prelude::*;

use crate::{store::Store, ui::widgets::slide};

pub const LOOP_SETTING: &str = "loop";
const PREFIXES: [(f64, &str); 4] = [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, "")];
const TRANSPORT_TICK: Duration = Duration::from_millis(200);
const SEEK_SETTLE: Duration = Duration::from_millis(250);

const SHEET: &str = css!(
    r#"
.btn.danger { border-color: var(--danger); color: var(--danger); }
.btn.primary { border-color: var(--accent); color: var(--accent); }
.btn:disabled { opacity: 0.45; }
.readout { flex-direction: column; gap: 3px; }
.readout__row { flex-direction: row; gap: 8px; align-items: baseline; min-width: 0; }
.readout__label { flex: 0 0 76px; color: var(--ink-faint); font-size: 10px; text-transform: uppercase; }
.readout__value { flex: 1 1 auto; min-width: 0; font-family: var(--mono); font-size: 11px; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.alert { color: var(--danger); font-size: 11px; }
.chips { flex-direction: row; flex-wrap: wrap; gap: 4px; }
.chip { flex-direction: row; gap: 4px; padding: 1px 6px; border: 1px solid var(--line); border-radius: 4px; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
.lane { flex-direction: column; gap: 6px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
.lane:last-child { border-bottom-width: 0; padding-bottom: 0; }
.lane__head { flex-direction: row; gap: 8px; align-items: center; min-width: 0; }
.lane__name { flex: 1 1 auto; min-width: 0; font-size: 10px; text-transform: uppercase; color: var(--ink-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.transport { flex-direction: row; gap: 6px; align-items: center; }
.transport__clock { flex: 0 0 auto; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
.transport .field__body { flex: 1 1 auto; min-width: 0; }
"#
);

pub fn install() {
    install_stylesheet("kit_audio", SHEET);
}

#[must_use]
pub fn si(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("? {unit}");
    }
    let (scale, prefix) = PREFIXES
        .iter()
        .copied()
        .find(|(step, _)| value.abs() >= *step)
        .unwrap_or((1.0, ""));
    let fixed = format!("{:.9}", value / scale);
    let trimmed = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.')
    } else {
        &fixed
    };
    format!("{trimmed} {prefix}{unit}")
}

#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    si(bytes as f64, "B")
}

#[must_use]
pub fn format_duration(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() / 10.0;
    if tenths < 60.0 {
        return format!("{tenths:.1} s");
    }
    format_clock(tenths.round())
}

#[must_use]
pub fn format_clock(seconds: f64) -> String {
    let whole = seconds.max(0.0).floor() as u64;
    let (hours, minutes, secs) = (whole / 3_600, (whole % 3_600) / 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[must_use]
pub fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

#[must_use]
pub fn recording_elapsed_s(status: &RecordingStatus, now_ms: i64, sample_rate: f64) -> f64 {
    if status.error.is_some() {
        return if sample_rate > 0.0 {
            status.samples as f64 / sample_rate
        } else {
            0.0
        };
    }
    status
        .started_at
        .parse::<jiff::Timestamp>()
        .map_or(0.0, |started| {
            ((now_ms - started.as_millisecond()) as f64 / 1_000.0).max(0.0)
        })
}

#[must_use]
pub fn recording_for<'a>(
    channel: &'a ChannelInfo,
    fx: &[String],
) -> Option<&'a AudioRecordingStatus> {
    channel
        .audio_recordings
        .iter()
        .find(|status| status.fx == fx)
}

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
    let advanced = reported + (elapsed_ms / 1_000.0).max(0.0) * sample_rate;
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

pub fn ticker(every: Duration) -> RwSignal<i64> {
    let now = RwSignal::new(now_ms());
    let ticking = set_interval(every, move || now.set(now_ms()));
    on_cleanup_local(move || drop(ticking));
    now
}

pub fn edit_body(store: Store, node: &str, change: impl FnOnce(&mut NodeBody)) {
    let node = node.to_owned();
    store.edit_graph(move |graph| {
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node) {
            change(&mut found.body);
        }
    });
}

pub fn node_label(store: Store, node: &str, fallback: &str) -> String {
    store
        .graph
        .get()
        .node(node)
        .and_then(|found| found.label.clone())
        .unwrap_or_else(|| fallback.to_uppercase())
}

pub fn button(
    label: impl Fn() -> String + 'static,
    danger: Signal<bool>,
    disabled: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = "btn",
            class:danger = danger,
            state:disabled = disabled,
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            on:pointer_down:stop = move |_| {},
            on:click:stop = move |_| on_press()
        ) {
            {label}
        }
    }
}

pub fn readout(rows: Vec<(&'static str, AnyView)>) -> impl IntoView {
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(label, value)| {
            view! {
                row(class = "readout__row") {
                    text(class = "readout__label") {{label}}
                    box(class = "readout__value") {{value}}
                }
            }
        })
        .collect();
    view! { column(class = "readout") {{rows}} }
}

pub fn alert(message: impl Fn() -> Option<String> + Send + Sync + 'static) -> impl IntoView {
    let message = Signal::derive(message);
    view! {
        if move || message.get().is_some() {
            text(class = "alert") {{move || message.get().unwrap_or_default()}}
        }
    }
}

pub fn is_recording(store: Store, node: String) -> Signal<bool> {
    Signal::derive(move || {
        store.graph.get().node(&node).is_some_and(|found| {
            matches!(
                &found.body,
                NodeBody::Recorder(body) | NodeBody::AudioRecorder(body) | NodeBody::BasebandRecorder(body)
                    if body.recording
            )
        })
    })
}

pub fn recorder_switch(store: Store, node: String) -> impl IntoView {
    let recording = is_recording(store, node.clone());
    button(
        move || {
            String::from(if recording.get() {
                "Stop"
            } else {
                "\u{25cf} Record"
            })
        },
        recording,
        Signal::stored(false),
        move || {
            let next = !recording.get_untracked();
            edit_body(store, &node, |body| {
                if let NodeBody::Recorder(recorder)
                | NodeBody::AudioRecorder(recorder)
                | NodeBody::BasebandRecorder(recorder) = body
                {
                    recorder.recording = next;
                }
            });
        },
    )
}

fn drive_playback(store: Store, set: u32, action: PlaybackAction, position: Option<u64>) {
    let request = PlaybackRequest {
        action,
        position_samples: position,
    };
    zgui::task::spawn_local(async move {
        let path = format!("/api/devicesets/{set}/playback");
        if let Err(error) = store.api().post::<_, PlaybackStatus>(&path, &request).await {
            store.say(format!("cannot drive playback: {error}"));
        }
        store.refresh_state();
    });
}

pub fn transport(store: Store, set: u32) -> impl IntoView {
    install();
    let device = Signal::derive(move || store.set_of(set));
    let status = Signal::derive(move || device.get().and_then(|device| device.playback));
    let rate = Signal::derive(move || {
        device
            .get()
            .and_then(|device| device.settings.sample_rate)
            .unwrap_or(0.0)
    });
    let looping = Signal::derive(move || device.get().is_none_or(|device| is_looping(&device)));
    let now = ticker(TRANSPORT_TICK);
    let anchor = RwSignal::new((0u64, now_ms()));
    let scrub = RwSignal::new(None::<f64>);
    let settle = StoredValue::new_local(None::<zgui::view::TimeoutHandle>);
    let clock = Timers::current();
    let anchoring = zgui::reactive::RenderEffect::new(move |_| {
        if let Some(status) = status.get() {
            anchor.set((status.position_samples, now_ms()));
        }
    });
    on_cleanup_local(move || drop(anchoring));
    let live = Signal::derive(move || {
        let Some(status) = status.get() else {
            return 0.0;
        };
        let (position, at) = anchor.get();
        let moved = PlaybackStatus {
            position_samples: position,
            ..status
        };
        playback_position_at(&moved, (now.get() - at) as f64, rate.get(), looping.get())
    });
    let position = Signal::derive(move || scrub.get().unwrap_or_else(|| live.get()));
    let total = Signal::derive(move || {
        status
            .get()
            .map_or(1.0, |status| status.total_samples.max(1) as f64)
    });
    let paused = Signal::derive(move || status.get().is_none_or(|status| status.paused));
    let seek = move |target: f64| {
        scrub.set(Some(target));
        let Some(clock) = clock.clone() else {
            drive_playback(
                store,
                set,
                PlaybackAction::Seek,
                Some(target.max(0.0) as u64),
            );
            scrub.set(None);
            return;
        };
        let handle = clock.set_timeout(SEEK_SETTLE, move || {
            drive_playback(
                store,
                set,
                PlaybackAction::Seek,
                Some(target.max(0.0) as u64),
            );
            scrub.set(None);
        });
        settle.set_value(Some(handle));
    };
    let read = move |at: f64| {
        format!(
            "{} / {}",
            format_clock(samples_to_seconds(at, rate.get_untracked())),
            format_clock(samples_to_seconds(
                total.get_untracked(),
                rate.get_untracked()
            ))
        )
    };
    view! {
        row(class = "transport") {
            {button(
                move || String::from(if paused.get() { "Play" } else { "Pause" }),
                Signal::stored(false),
                Signal::stored(false),
                move || {
                    let action = if paused.get_untracked() { PlaybackAction::Play } else { PlaybackAction::Pause };
                    drive_playback(store, set, action, None);
                },
            )}
            {button(
                || String::from("Stop"),
                Signal::stored(false),
                Signal::stored(false),
                move || drive_playback(store, set, PlaybackAction::Stop, None),
            )}
            {button(
                move || String::from(if looping.get() { "Loop on" } else { "Loop off" }),
                Signal::stored(false),
                Signal::stored(false),
                move || {
                    let settings = DeviceSettings {
                        extra: vec![ExtraValue {
                            name: LOOP_SETTING.to_owned(),
                            value: serde_json::Value::Bool(!looping.get_untracked()),
                        }],
                        ..DeviceSettings::default()
                    };
                    store.set_device(set, settings);
                },
            )}
            {move || {
                let total = total.get();
                AnyView::new(slide(position, 0.0, total, read, seek.clone()))
            }}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(position: u64, paused: bool) -> PlaybackStatus {
        PlaybackStatus {
            position_samples: position,
            total_samples: 48_000,
            paused,
        }
    }

    fn recording(started_at: &str, samples: u64, error: Option<&str>) -> RecordingStatus {
        RecordingStatus {
            file: String::from("a.sigmf"),
            stream: 0,
            started_at: started_at.to_owned(),
            samples,
            bytes: 0,
            overruns: 0,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn sizes_read_in_the_unit_they_belong_in() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1_536), "1.536 kB");
        assert_eq!(format_bytes(12_000_000), "12 MB");
        assert_eq!(si(f64::NAN, "B"), "? B");
    }

    #[test]
    fn durations_read_seconds_then_a_clock() {
        assert_eq!(format_duration(9.94), "9.9 s");
        assert_eq!(format_duration(75.0), "1:15");
        assert_eq!(format_duration(3_725.0), "1:02:05");
    }

    #[test]
    fn a_clock_is_fixed_width_grows_hours_only_when_needed_and_never_reads_negative() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(9.9), "0:09");
        assert_eq!(format_clock(64.0), "1:04");
        assert_eq!(format_clock(599.0), "9:59");
        assert_eq!(format_clock(3_600.0), "1:00:00");
        assert_eq!(format_clock(3_725.0), "1:02:05");
        assert_eq!(format_clock(-5.0), "0:00");
    }

    #[test]
    fn playback_advances_on_the_clock_and_holds_while_paused() {
        let at = playback_position_at(&playback(1_000, false), 500.0, 48_000.0, true);
        assert!((at - 25_000.0).abs() < f64::EPSILON);
        let held = playback_position_at(&playback(1_000, true), 60_000.0, 48_000.0, true);
        assert!((held - 1_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn playback_wraps_with_the_loop_and_holds_at_the_end_without() {
        let looped = playback_position_at(&playback(0, false), 1_500.0, 48_000.0, true);
        assert!((looped - 24_000.0).abs() < f64::EPSILON);
        let ended = playback_position_at(&playback(0, false), 5_000.0, 48_000.0, false);
        assert!((ended - 48_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn playback_never_runs_backwards_or_past_the_recording() {
        let back = playback_position_at(&playback(100, false), -5_000.0, 48_000.0, true);
        assert!((back - 100.0).abs() < f64::EPSILON);
        let no_rate = playback_position_at(&playback(100, false), 5_000.0, 0.0, true);
        assert!((no_rate - 100.0).abs() < f64::EPSILON);
        let beyond = playback_position_at(&playback(99_999, true), 0.0, 48_000.0, false);
        assert!((beyond - 48_000.0).abs() < f64::EPSILON);
        let empty = PlaybackStatus {
            position_samples: 0,
            total_samples: 0,
            paused: false,
        };
        assert!(playback_position_at(&empty, 5_000.0, 48_000.0, true).abs() < f64::EPSILON);
        assert!((samples_to_seconds(96_000.0, 48_000.0) - 2.0).abs() < f64::EPSILON);
        assert!(samples_to_seconds(96_000.0, 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_recording_counts_the_wall_clock_until_it_faults() {
        let now = "2026-09-23T12:00:10Z"
            .parse::<jiff::Timestamp>()
            .map(|at| at.as_millisecond())
            .unwrap_or_default();
        let running = recording("2026-09-23T12:00:00Z", 0, None);
        assert!((recording_elapsed_s(&running, now, 48_000.0) - 10.0).abs() < 1e-9);
        let faulted = recording("2026-09-23T12:00:00Z", 96_000, Some("disk full"));
        assert!((recording_elapsed_s(&faulted, now, 48_000.0) - 2.0).abs() < 1e-9);
        assert!(recording_elapsed_s(&recording("garbage", 0, None), now, 48_000.0).abs() < 1e-9);
    }

    #[test]
    fn an_audio_recording_is_found_by_its_exact_route() {
        let status = |file: &str, fx: &[&str]| AudioRecordingStatus {
            fx: fx.iter().map(|node| (*node).to_owned()).collect(),
            file: file.to_owned(),
            started_at: String::from("2026-09-23T12:00:00Z"),
            channels: 1,
            frames: 0,
            bytes: 0,
            error: None,
        };
        let mut channel: ChannelInfo = serde_json::from_value(serde_json::json!({
            "id": 1,
            "settings": { "frequency_hz": 1.0, "params": { "type": "nfm", "settings": {} } }
        }))
        .expect("a channel");
        channel.audio_recordings = vec![status("raw.wav", &[]), status("clean.wav", &["a", "b"])];
        let route = |fx: &[&str]| fx.iter().map(|node| (*node).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            recording_for(&channel, &route(&[])).map(|s| s.file.as_str()),
            Some("raw.wav")
        );
        assert_eq!(
            recording_for(&channel, &route(&["a", "b"])).map(|s| s.file.as_str()),
            Some("clean.wav")
        );
        assert!(recording_for(&channel, &route(&["b", "a"])).is_none());
        channel.audio_recordings.clear();
        assert!(recording_for(&channel, &route(&[])).is_none());
    }
}
