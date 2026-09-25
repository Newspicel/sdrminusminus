pub mod binding;
pub mod controls;
pub mod edit;
pub mod modes;
pub mod picker;
pub mod settings;
pub mod swap;
pub mod tables;

use std::collections::HashMap;

use sdrmm_wire::{
    channel::{ChannelSettings, MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB, Squelch},
    decode::{BroadcastStatus, DecoderEvent},
    device::Capabilities,
    patch::NodeBody,
    state::DeviceSet,
    ws::ServerEvent,
};
use zgui::prelude::*;

use self::{
    binding::{
        ChannelBinding, iq_lanes_of, radio_is_attached, radio_refs_of, tuning_controller_of,
    },
    controls::Ctx,
    settings::{
        DEFAULT_SQUELCH_DB, DEFAULT_SQUELCH_MARGIN_DB, RadioWindow, SQUELCH_MAX_DB, SQUELCH_MIN_DB,
        SquelchMode, channel_has_audio, channel_width_hz, keeps_calls, radio_window_hz, reaches_hz,
        squelch_at, squelch_mode,
    },
    tables::SQUELCH_MODES,
};
use crate::{
    store::Store,
    ui::{
        kit_channel::{
            self, SWAP_ICON, blanker_control, button, format_hz, format_mhz, icon, level_meter,
            setting_row, slider_field, tip, toggle_row, tune_to, tuning_lock,
        },
        widgets::{dial, segments},
    },
};

pub use swap::{cycle_analog, swap_decoder};

pub const SQUELCH_STEP_DB: f32 = 2.0;

pub fn adjust_squelch(store: Store, node: &str, delta_db: f32) {
    store.edit_channel(node, move |settings| {
        settings.squelch = settings::nudged_squelch(&settings.squelch, delta_db);
    });
}

pub fn toggle_squelch(store: Store, node: &str) {
    store.edit_channel(node, |settings| {
        settings.squelch = if settings.squelch.is_off() {
            Squelch::Manual {
                level_db: DEFAULT_SQUELCH_DB,
            }
        } else {
            Squelch::Off
        };
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    CycleMode(i32),
    Squelch(i8),
    ToggleSquelch,
}

#[must_use]
pub fn hotkey_of(key: &str) -> Option<Hotkey> {
    Some(match key {
        "m" => Hotkey::CycleMode(1),
        "M" => Hotkey::CycleMode(-1),
        "-" => Hotkey::Squelch(-1),
        "=" | "+" => Hotkey::Squelch(1),
        "s" => Hotkey::ToggleSquelch,
        _ => return None,
    })
}

pub fn run_hotkey(store: Store, node: &str, key: Hotkey) {
    match key {
        Hotkey::CycleMode(direction) => {
            cycle_analog(store, node, direction);
        }
        Hotkey::Squelch(sign) => adjust_squelch(store, node, SQUELCH_STEP_DB * f32::from(sign)),
        Hotkey::ToggleSquelch => toggle_squelch(store, node),
    }
}

const ANY_FREQUENCY: (f64, f64) = (0.0, 6e9);

const SHEET: &str = css!(
    r#"
.ch-face { flex-direction: column; }
.ch-top { flex-direction: column; gap: 6px; padding: 8px; border-bottom: 1px solid var(--line); }
.ch-meta { align-items: center; gap: 8px; min-height: 18px; }
.ch-badge { font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.ch-status { font-family: var(--mono); font-size: 10px; color: var(--ink-dim); overflow: hidden; }
.ch-status.warn { color: oklch(0.8 0.14 75); }
.ch-dialrow { align-items: center; gap: 4px; min-width: 0; }
.ch-dialrow.locked .digit { color: var(--ink-faint); }
.ch-tools { margin-left: auto; align-items: center; gap: 4px; flex: 0 0 auto; }
.ch-body { flex-direction: column; gap: 8px; padding: 12px; }
.ch-foot { justify-content: flex-end; gap: 8px; padding: 6px 12px; border-top: 1px solid var(--line); }
"#
);

#[must_use]
pub fn tuning_range(capabilities: &Capabilities) -> (f64, f64) {
    let ranges = &capabilities.freq_ranges;
    if ranges.is_empty() {
        return ANY_FREQUENCY;
    }
    let min = ranges
        .iter()
        .map(|range| range.min)
        .fold(f64::INFINITY, f64::min);
    let max = ranges
        .iter()
        .map(|range| range.max)
        .fold(f64::NEG_INFINITY, f64::max);
    (min, max)
}

#[must_use]
pub fn in_tuning_range(hz: f64, range: (f64, f64)) -> Option<f64> {
    (hz >= range.0 && hz <= range.1).then_some(hz)
}

#[must_use]
pub fn lane_center_hz(set: &DeviceSet, stream: u32) -> Option<f64> {
    if let Some(lane) = set.extra_lane.filter(|lane| lane.stream == stream) {
        return Some(lane.center_hz);
    }
    let own = set
        .settings
        .streams
        .iter()
        .find(|entry| entry.stream == stream)
        .and_then(|entry| entry.center_hz)
        .filter(|_| set.capabilities.per_stream.tuning);
    own.or(set.settings.center_hz)
}

#[must_use]
pub fn lane_rate_hz(set: &DeviceSet, stream: u32) -> Option<f64> {
    match set.extra_lane.filter(|lane| lane.stream == stream) {
        Some(lane) => Some(lane.sample_rate),
        None => set.settings.sample_rate,
    }
}

#[must_use]
pub fn face_status(
    live: bool,
    binding: ChannelBinding,
    unreachable: bool,
    driver: Option<String>,
    carrier: Option<String>,
) -> Option<(String, bool)> {
    if !live {
        return (binding != ChannelBinding::Unwired).then(|| (binding.status().to_owned(), false));
    }
    if let Some(driver) = driver {
        return Some((driver, false));
    }
    if unreachable {
        return Some((String::from("out of band"), true));
    }
    carrier.map(|carrier| (carrier, false))
}

#[derive(Clone, Debug, PartialEq)]
struct View {
    settings: Option<ChannelSettings>,
    live: Option<(u32, u32)>,
    binding: ChannelBinding,
    status: Option<(String, bool)>,
    width_hz: Option<f64>,
    window: Option<RadioWindow>,
    range: (f64, f64),
    locked: bool,
    controller: Option<String>,
}

fn view_of(store: Store, node: &str, tracked: Option<String>) -> View {
    let graph = store.graph.get();
    let descriptor = store.channel_descriptor(node);
    let settings = store.channel_settings(node);
    let live = store.live_channel(node);
    let lanes = iq_lanes_of(&graph, node);
    let set = store.device_set_of(node).and_then(|id| store.set_of(id));
    let references = radio_refs_of(&graph, node);
    let binding = ChannelBinding::of(
        !lanes.is_empty(),
        set.is_some(),
        !references.is_empty(),
        radio_is_attached(&references, &store.devices.get()),
    );
    let stream = live.as_ref().map_or_else(
        || lanes.first().map_or(0, |lane| lane.stream),
        |live| live.channel.stream,
    );
    let center_hz = set.as_ref().and_then(|set| lane_center_hz(set, stream));
    let span_hz = set.as_ref().and_then(|set| lane_rate_hz(set, stream));
    let window = radio_window_hz(center_hz, span_hz, descriptor.as_ref());
    let frequency = settings.as_ref().map(|settings| settings.frequency_hz);
    let unreachable = set.is_some()
        && frequency.is_some_and(|hz| {
            live.as_ref()
                .map_or_else(|| !reaches_hz(hz, window), |live| live.channel.out_of_band)
        });
    let scanned = live.as_ref().is_some_and(|live| {
        set.as_ref().is_some_and(|set| {
            set.scanners.iter().any(|scanner| {
                scanner.error.is_none() && scanner.settings.channel == live.channel.id
            })
        })
    });
    let driver = if scanned {
        Some(String::from("scanning"))
    } else {
        tracked.clone()
    };
    let carrier = (lanes.len() > 1)
        .then(|| {
            let lane = lanes
                .iter()
                .find(|lane| lane.stream == stream)
                .or(lanes.first())?;
            let label = graph
                .node(&lane.source)
                .and_then(|found| found.label.clone());
            label.or_else(|| set.as_ref().map(|set| set.device.label.clone()))
        })
        .flatten();
    let locked = matches!(graph.node(node).map(|found| &found.body), Some(NodeBody::Channel(channel)) if channel.tuning_locked);
    let controller = tuning_controller_of(&graph, node).map(|kind| tracked.unwrap_or(kind));
    View {
        width_hz: channel_width_hz(
            settings.as_ref().map(|settings| &settings.params),
            descriptor.as_ref(),
        ),
        settings,
        live: live.as_ref().map(|live| (live.device_set, live.channel.id)),
        binding,
        status: face_status(live.is_some(), binding, unreachable, driver, carrier),
        window,
        range: set
            .as_ref()
            .map_or(ANY_FREQUENCY, |set| tuning_range(&set.capabilities)),
        locked,
        controller,
    }
}

fn watch_satellites(store: Store, node: String) -> Signal<Option<String>> {
    let tracked = RwSignal::new(HashMap::<String, Option<String>>::new());
    let wanted = node.clone();
    store.on_event(move |event| {
        if let ServerEvent::SatelliteUpdate { status } = event {
            let driving = status.driving.iter().any(|driven| *driven == wanted);
            tracked.update(|held| {
                if driving {
                    held.insert(status.node.clone(), status.name.clone());
                } else {
                    held.remove(&status.node);
                }
            });
        }
    });
    Signal::derive(move || {
        tracked
            .get()
            .values()
            .next()
            .map(|name| name.clone().unwrap_or_else(|| String::from("satellite")))
    })
}

fn watch_broadcast(
    store: Store,
    live: Signal<Option<(u32, u32)>>,
) -> Signal<Option<BroadcastStatus>> {
    let heard = RwSignal::new(None::<BroadcastStatus>);
    let latest = move |held: &[sdrmm_wire::decode::DecodedRecord], at: (u32, u32)| {
        held.iter().rev().find_map(|record| match &record.event {
            DecoderEvent::Broadcast(status) if (record.device_set, record.channel) == at => {
                Some(status.clone())
            }
            _ => None,
        })
    };
    if let Some(at) = live.get_untracked() {
        heard.set(latest(&store.decoded.get_untracked(), at));
    }
    store.on_event(move |event| {
        if let ServerEvent::Decoded(record) = event
            && let DecoderEvent::Broadcast(status) = &record.event
            && live.get_untracked() == Some((record.device_set, record.channel))
        {
            heard.set(Some(status.clone()));
        }
    });
    heard.into()
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("channel-face", SHEET);
    kit_channel::install();
    let tracked = watch_satellites(store, node.clone());
    let shown = {
        let node = node.clone();
        Memo::new(move |_| view_of(store, &node, tracked.get()))
    };
    let live = Signal::derive(move || shown.get().live);
    let broadcast = watch_broadcast(store, live);
    let replacing = RwSignal::new(false);
    let has_settings = Memo::new(move |_| shown.get().settings.is_some());
    let top = {
        let node = node.clone();
        move || {
            has_settings
                .get()
                .then(|| top(store, node.clone(), shown, replacing))
        }
    };
    let body = {
        let node = node.clone();
        move || {
            has_settings
                .get()
                .then(|| controls(store, node.clone(), shown, broadcast))
        }
    };
    let replace = {
        let node = node.clone();
        move || {
            replacing.get().then(|| {
                AnyView::new(picker::replace_decoder(store, node.clone(), move || {
                    replacing.set(false)
                }))
            })
        }
    };
    let keys = {
        let node = node.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            if ev.modifiers.control() || ev.modifiers.meta() || ev.modifiers.alt() {
                return;
            }
            if let Some(key) = ev.key.as_str().and_then(hotkey_of) {
                run_hotkey(store, &node, key);
                ev.prevent_default();
            }
        }
    };
    view! {
        column(class = "ch-face", on:key_down = keys) {
            {top}
            {body}
            {footer(store, shown)}
            {replace}
        }
    }
}

fn top(store: Store, node: String, shown: Memo<View>, replacing: RwSignal<bool>) -> AnyView {
    let hz = Signal::derive(move || {
        shown
            .get()
            .settings
            .map_or(0.0, |settings| settings.frequency_hz)
    });
    let locked = Signal::derive(move || {
        let view = shown.get();
        view.locked || view.controller.is_some()
    });
    let tune = {
        let node = node.clone();
        move |value: f64| {
            let view = shown.get_untracked();
            if view.locked || view.controller.is_some() {
                return;
            }
            let value = value.clamp(view.range.0, view.range.1);
            store.edit_channel(&node, move |settings| settings.frequency_hz = value);
        }
    };
    let hint = Signal::derive(move || {
        let view = shown.get();
        match view.window {
            Some(window) => format!(
                "The radio hears {} to {}",
                format_mhz(window.low_hz),
                format_mhz(window.high_hz)
            ),
            None => format!(
                "Reaches {} to {}",
                format_mhz(view.range.0),
                format_mhz(view.range.1)
            ),
        }
    });
    let resolve = move |entered: f64| in_tuning_range(entered, shown.get_untracked().range);
    let hold = Signal::derive(move || {
        shown
            .get()
            .controller
            .map(|by| format!("Tuned by {by}. Unwire its control to tune by hand."))
    });
    let lock = {
        let node = node.clone();
        move |on: bool| {
            store.edit_node(&node, move |body| {
                if let NodeBody::Channel(channel) = body {
                    channel.tuning_locked = on;
                }
            });
        }
    };
    let level = Signal::derive(move || {
        let (set, channel) = shown.get().live?;
        store.levels.get().get(&(set, channel)).copied()
    });
    let squelch_db = Signal::derive(move || {
        shown
            .get()
            .settings
            .and_then(|settings| settings.squelch.manual_level_db())
    });
    let is_live = Signal::derive(move || shown.get().live.is_some());
    let meter = move || {
        is_live
            .get()
            .then(|| AnyView::new(level_meter(level, squelch_db)))
    };
    let dial_tune = tune.clone();
    AnyView::new(view! {
        column(class = "ch-top") {
            {meta(shown, replacing)}
            row(class = "ch-dialrow", class:locked = locked) {
                {dial(hz, dial_tune)}
                row(class = "ch-tools") {
                    {tune_to("Type a frequency to listen on", hz, hint, resolve, locked, tune)}
                    {tuning_lock(locked, "Frequency locked", "Lock frequency", hold, lock)}
                }
            }
            {meter}
        }
    })
}

fn meta(shown: Memo<View>, replacing: RwSignal<bool>) -> impl IntoView {
    let width = move || shown.get().width_hz.map(format_hz).unwrap_or_default();
    let status = move || shown.get().status.map(|(text, _)| text).unwrap_or_default();
    let warn = move || shown.get().status.is_some_and(|(_, warn)| warn);
    view! {
        row(class = "ch-meta") {
            text(class = "ch-badge", a11y:tooltip = "Channel bandwidth") {{width}}
            text(class = "ch-status", class:warn = warn) {{status}}
            spacer() {}
            {tip("Replace the decoder", AnyView::new(view! {
                control(
                    class = "kc-icon",
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    a11y:label = "Replace the decoder",
                    on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                    on:click:stop = move |_| replacing.set(true)
                ) {
                    {icon(SWAP_ICON, "kc-glyph kc-glyph--sm")}
                }
            }))}
        }
    }
}

fn controls(
    store: Store,
    node: String,
    shown: Memo<View>,
    broadcast: Signal<Option<BroadcastStatus>>,
) -> AnyView {
    let descriptor = store.channel_descriptor(&node);
    let kind = swap::current_type(&store.graph.get_untracked(), &node).unwrap_or_default();
    let audio = channel_has_audio(descriptor.as_ref());
    let ctx = Ctx {
        store,
        node: StoredValue::new(node.clone()),
        params: Signal::derive(move || shown.get().settings.map(|settings| settings.params)),
        limits: StoredValue::new(
            descriptor
                .as_ref()
                .map(|descriptor| descriptor.limits.clone())
                .unwrap_or_default(),
        ),
        broadcast,
    };
    let squelch = audio.then(|| squelch_row(store, node.clone(), shown));
    let calls = keeps_calls(descriptor.as_ref()).then(|| record_calls(store, node.clone()));
    let blanker = audio.then(|| {
        let node = node.clone();
        blanker_control(
            Signal::derive(move || {
                shown
                    .get()
                    .settings
                    .map(|settings| settings.blanker)
                    .unwrap_or_default()
            }),
            move |blanker| store.edit_channel(&node, move |settings| settings.blanker = blanker),
        )
    });
    AnyView::new(view! {
        column(class = "ch-body kc-grid") {
            {squelch}
            {modes::mode_view(ctx, &kind)}
            {calls}
            {blanker}
        }
    })
}

fn record_calls(store: Store, node: String) -> AnyView {
    let on = {
        let node = node.clone();
        Signal::derive(
            move || matches!(store.graph.get().node(&node).map(|found| &found.body), Some(NodeBody::Channel(channel)) if channel.record_calls),
        )
    };
    toggle_row(
        "Record calls",
        Some("Save each call the decoder hears as its own audio file"),
        on,
        move |record: bool| {
            store.edit_node(&node, move |body| {
                if let NodeBody::Channel(channel) = body {
                    channel.record_calls = record;
                }
            });
        },
    )
}

fn squelch_row(store: Store, node: String, shown: Memo<View>) -> AnyView {
    let squelch = Signal::derive(move || {
        shown
            .get()
            .settings
            .map(|settings| settings.squelch)
            .unwrap_or_default()
    });
    let mode = Signal::derive(move || squelch_mode(&squelch.get()));
    let held_db = RwSignal::new(DEFAULT_SQUELCH_DB);
    let level_db =
        Signal::derive(move || f64::from(squelch.get().manual_level_db().unwrap_or(held_db.get())));
    let margin_db = Signal::derive(move || {
        f64::from(
            squelch
                .get()
                .auto_margin_db()
                .unwrap_or(DEFAULT_SQUELCH_MARGIN_DB),
        )
    });
    let pick = {
        let node = node.clone();
        move |next: SquelchMode| {
            let now = mode.get_untracked();
            if next == now {
                return;
            }
            let level = level_db.get_untracked() as f32;
            if now == SquelchMode::Manual {
                held_db.set(level);
            }
            let margin = margin_db.get_untracked() as f32;
            store.edit_channel(&node, move |settings| {
                settings.squelch = squelch_at(next, level, margin)
            });
        }
    };
    let slider = move || {
        let node = node.clone();
        match mode.get() {
            SquelchMode::Off => None,
            SquelchMode::Manual => Some(setting_row(
                "Level",
                None,
                slider_field(
                    level_db,
                    (f64::from(SQUELCH_MIN_DB), f64::from(SQUELCH_MAX_DB), 1.0),
                    Signal::stored(false),
                    |value| format!("{value:.0} dB"),
                    move |value| {
                        store.edit_channel(&node, move |settings| {
                            settings.squelch = Squelch::Manual {
                                level_db: value as f32,
                            };
                        });
                    },
                ),
            )),
            SquelchMode::Auto => Some(setting_row(
                "Margin",
                Some("How far above the measured noise floor the channel opens"),
                slider_field(
                    margin_db,
                    (
                        f64::from(MIN_SQUELCH_AUTO_MARGIN_DB),
                        f64::from(MAX_SQUELCH_AUTO_MARGIN_DB),
                        1.0,
                    ),
                    Signal::stored(false),
                    |value| format!("+{value:.0} dB"),
                    move |value| {
                        store.edit_channel(&node, move |settings| {
                            settings.squelch = Squelch::Auto {
                                margin_db: value as f32,
                            };
                        });
                    },
                ),
            )),
        }
    };
    AnyView::new(view! {
        column(class = "kc-grid") {
            {setting_row(
                "Squelch",
                Some("Mute the channel until a signal is strong enough"),
                segments(SQUELCH_MODES.to_vec(), mode, pick),
            )}
            {slider}
        }
    })
}

fn footer(store: Store, shown: Memo<View>) -> impl IntoView {
    let action = move || {
        let view = shown.get();
        if view.live.is_some() {
            return None;
        }
        let label = view.binding.action()?;
        Some(view! {
            row(class = "ch-foot") {
                {tip(view.binding.hint(), AnyView::new(button(
                    label,
                    "kc-btn kc-btn--primary",
                    Signal::stored(false),
                    move || {
                        zgui::task::spawn_local(async move { store.apply().await });
                    },
                )))}
            }
        })
    };
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_reads_the_binding_before_anything_live() {
        assert_eq!(
            face_status(false, ChannelBinding::Unwired, false, None, None),
            None
        );
        assert_eq!(
            face_status(false, ChannelBinding::RadioClosed, true, None, None),
            Some((String::from("radio closed"), false))
        );
        assert_eq!(
            face_status(
                true,
                ChannelBinding::NotStarted,
                true,
                Some(String::from("scanning")),
                None
            ),
            Some((String::from("scanning"), false))
        );
        assert_eq!(
            face_status(
                true,
                ChannelBinding::NotStarted,
                true,
                None,
                Some(String::from("RTL"))
            ),
            Some((String::from("out of band"), true))
        );
        assert_eq!(
            face_status(
                true,
                ChannelBinding::NotStarted,
                false,
                None,
                Some(String::from("RTL"))
            ),
            Some((String::from("RTL"), false))
        );
        assert_eq!(
            face_status(true, ChannelBinding::NotStarted, false, None, None),
            None
        );
    }

    #[test]
    fn the_tuning_range_spans_every_range_the_radio_reaches() {
        let capabilities: Capabilities = serde_json::from_value(serde_json::json!({
            "freq_ranges": [{ "min": 24e6, "max": 1.7e9 }, { "min": 500e3, "max": 28e6 }],
            "sample_rates": [], "gains": [], "antennas": [], "bandwidths": []
        }))
        .expect("capabilities");
        assert_eq!(tuning_range(&capabilities), (500e3, 1.7e9));
        let none: Capabilities = serde_json::from_value(serde_json::json!({
            "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": []
        }))
        .expect("capabilities");
        assert_eq!(tuning_range(&none), ANY_FREQUENCY);
        assert_eq!(in_tuning_range(100e6, (0.0, 6e9)), Some(100e6));
        assert_eq!(in_tuning_range(7e9, (0.0, 6e9)), None);
    }
}
