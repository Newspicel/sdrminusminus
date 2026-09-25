use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use sdrmm_wire::{
    audio::{MAX_BLANKER_THRESHOLD, MIN_BLANKER_THRESHOLD, NoiseBlankerSettings},
    state::ChannelLevel,
};
use zgui::{prelude::*, reactive::RenderEffect, view::TimeoutHandle};
use zgui_ui::prelude::*;

use crate::ui::{
    faces::channel::settings::{NumberLimit, SQUELCH_MAX_DB, SQUELCH_MIN_DB},
    widgets::{Menus, check, slide, toggled},
};

pub const LEVEL_FLOOR_DB: f32 = -140.0;
const DEBOUNCE: Duration = Duration::from_millis(150);
const LEVEL_SEGMENTS: usize = 12;

static NEXT_POPOVER: AtomicU32 = AtomicU32::new(1 << 24);

pub const KEYBOARD_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M10 8h.01"/><path d="M12 12h.01"/><path d="M14 8h.01"/><path d="M16 12h.01"/><path d="M18 8h.01"/><path d="M6 8h.01"/><path d="M7 16h10"/><path d="M8 12h.01"/><rect width="20" height="16" x="2" y="4" rx="2"/></svg>"#;
pub const LOCK_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>"#;
pub const UNLOCK_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/></svg>"#;
pub const CHEVRON_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#;
pub const CLOSE_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#;
pub const SWAP_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="m16 3 4 4-4 4"/><path d="M20 7H4"/><path d="m8 21-4-4 4-4"/><path d="M4 17h16"/></svg>"#;

const SHEET: &str = css!(
    r#"
.kc-grid { flex-direction: column; gap: 8px; }
.kc-row { align-items: center; gap: 10px; min-height: 24px; }
.kc-name {
    flex: 0 0 auto;
    width: 92px;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    overflow: hidden;
}
.kc-name--tip { text-decoration: underline dotted; text-decoration-color: var(--line-strong); }
.kc-cell { flex: 1 1 auto; min-width: 0; align-items: center; gap: 8px; flex-wrap: wrap; }
.kc-group { flex-direction: column; gap: 8px; padding-top: 8px; border-top: 1px solid var(--line); }
.kc-group:first-child { border-top-width: 0; padding-top: 0; }
.kc-group__head { align-items: center; justify-content: space-between; min-height: 20px; }
.kc-note { color: var(--ink-dim); font-size: 11px; }
.kc-warn { color: oklch(0.8 0.14 75); font-size: 11px; }
.kc-danger { color: var(--danger); }
.kc-accent { color: var(--accent); }
.kc-faint { color: var(--ink-faint); }
.kc-dim { color: var(--ink-dim); }

.kc-num { position: relative; flex: 0 1 auto; width: 132px; min-width: 64px; }
.kc-num--wide { width: 176px; }
.kc-num--narrow { width: 72px; }
.kc-num .native-input {
    height: 26px; width: 100%; min-width: 0; padding: 3px 8px;
    font-family: var(--mono); font-size: 11px; line-height: 18px;
    background-color: var(--panel-2); color: var(--ink);
    border: 1px solid var(--line); border-radius: 5px;
}
.kc-num .native-input:focus-visible { border-color: var(--accent); outline: 1px solid var(--accent); }
.kc-num.invalid .native-input { border-color: var(--danger); }
.kc-num__unit {
    position: absolute; right: 8px; top: 4px;
    font-family: var(--mono); font-size: 10px; color: var(--ink-faint);
    pointer-events: none;
}

.kc-btn {
    padding: 3px 10px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--panel-2);
    color: var(--ink);
    font-size: 11px;
    flex: 0 0 auto;
}
.kc-btn:hover { border-color: var(--line-strong); background-color: var(--panel-3); }
.kc-btn:disabled { opacity: 0.45; }
.kc-btn--sm { padding: 0 6px; font-size: 10px; }
.kc-btn--primary { border-color: var(--accent); background-color: var(--accent); color: var(--bg); font-weight: 600; }
.kc-btn--primary:hover { background-color: var(--accent); border-color: var(--accent); }
.kc-btn--danger:hover { border-color: var(--danger); color: var(--danger); }
.kc-btn--on { border-color: var(--accent); background-color: var(--accent); color: var(--bg); }
.kc-btn--mono { font-family: var(--mono); width: 44px; text-align: center; padding: 3px 0; }

.kc-icon {
    width: 26px; height: 26px; flex: 0 0 auto;
    border-radius: 5px; border: 1px solid transparent;
    color: var(--ink-dim);
    align-items: center; justify-content: center;
    display: flex;
}
.kc-icon:hover { background-color: var(--panel-2); color: var(--ink); }
.kc-icon.on { background-color: color-mix(in oklab, var(--accent) 15%, transparent); color: var(--accent); }
.kc-icon:disabled { opacity: 0.45; }
.kc-icon--held { background-color: color-mix(in oklab, var(--accent) 15%, transparent); color: var(--accent); }
.kc-glyph { width: 16px; height: 16px; }
.kc-glyph--sm { width: 12px; height: 12px; }

.kc-pop { position: relative; flex: 0 0 auto; }
.kc-pop__panel {
    position: absolute; right: 0; top: 30px; z-index: 70;
    width: 256px;
    flex-direction: column; gap: 8px; padding: 10px;
    border: 1px solid var(--line-strong); border-radius: 6px;
    background-color: var(--panel-3);
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
}
.kc-pop__panel--left { left: 0; right: auto; }
.kc-pop__form { align-items: center; gap: 8px; }

.kc-meter { flex: 1 1 auto; align-items: center; gap: 8px; }
.kc-meter__bar { position: relative; flex: 1 1 auto; height: 6px; border-radius: 2px; background-color: var(--panel-2); }
.kc-meter__fill { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 2px; background-color: var(--accent); }
.kc-meter__fill.closed { background-color: var(--accent-dim); }
.kc-meter__gap { position: absolute; top: 0; bottom: 0; width: 1px; background-color: var(--panel); }
.kc-meter__peak { position: absolute; top: 0; bottom: 0; width: 1px; background-color: var(--ink-dim); }
.kc-meter__gate { position: absolute; top: -2px; bottom: -2px; width: 2px; background-color: var(--ink); }
.kc-meter__read { flex: 0 0 auto; width: 64px; text-align: right; font-family: var(--mono); font-size: 11px; color: var(--ink-faint); }
.kc-meter__read.heard { color: var(--ink); }

.kc-chip {
    align-items: center; height: 22px; padding: 0 7px;
    border: 1px solid var(--line); border-radius: 4px;
    background-color: var(--panel-2);
    font-family: var(--mono); font-size: 11px; color: var(--ink);
}
.kc-chips { flex-wrap: wrap; gap: 4px; }
.kc-slider { flex: 1 1 auto; min-width: 0; align-items: center; gap: 8px; }
.kc-slider.off { opacity: 0.45; }
.kc-readout { flex-direction: column; gap: 3px; }
.kc-readout__row { gap: 10px; font-size: 11px; }
.kc-readout__name { flex: 0 0 auto; width: 92px; font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.kc-readout__value { flex: 1 1 auto; font-family: var(--mono); color: var(--ink); min-width: 0; }
"#
);

pub fn install() {
    install_stylesheet("kit-channel", SHEET);
}

#[must_use]
pub fn format_hz(hz: f64) -> String {
    si(hz, "Hz")
}

#[must_use]
pub fn si(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("? {unit}");
    }
    let magnitude = value.abs();
    let (scale, prefix) = if magnitude >= 1e9 {
        (1e9, "G")
    } else if magnitude >= 1e6 {
        (1e6, "M")
    } else if magnitude >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    format!(
        "{} {prefix}{unit}",
        trim_number(&format!("{:.9}", value / scale))
    )
}

fn trim_number(text: &str) -> String {
    if !text.contains('.') {
        return text.to_owned();
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[must_use]
pub fn format_mhz(hz: f64) -> String {
    format!("{:.4} MHz", hz / 1e6)
}

#[must_use]
pub fn fraction_digits(step: Option<f64>) -> usize {
    let Some(step) = step.filter(|step| step.is_finite() && *step != 0.0) else {
        return 6;
    };
    let text = format!("{:e}", step.abs());
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text.as_str(), "0"));
    let decimals = mantissa.split_once('.').map_or(0, |(_, rest)| rest.len()) as i64;
    let exponent = exponent.parse::<i64>().unwrap_or(0);
    (decimals - exponent).clamp(0, 20) as usize
}

#[must_use]
pub fn format_number(value: f64, step: Option<f64>) -> String {
    trim_number(&format!("{value:.*}", fraction_digits(step)))
}

#[must_use]
pub fn parse_number(text: &str) -> Option<f64> {
    let cleaned = text.trim().replace(',', ".");
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

#[must_use]
pub fn parse_frequency(text: &str) -> Option<f64> {
    let lowered = text.trim().to_lowercase();
    let split = lowered
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(lowered.len());
    let (number, unit) = lowered.split_at(split);
    let number = number.trim().replace(',', ".");
    if number.is_empty()
        || number.matches('.').count() > 1
        || number.ends_with('.')
        || !number.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return None;
    }
    let scale = match unit.trim() {
        "" | "m" | "mhz" => 1e6,
        "g" | "ghz" => 1e9,
        "k" | "khz" => 1e3,
        "h" | "hz" => 1.0,
        _ => return None,
    };
    let value = number.parse::<f64>().ok()?;
    value.is_finite().then(|| (value * scale).round())
}

#[must_use]
pub fn step_value(value: f64, step: f64, direction: f64, limit: NumberLimit) -> f64 {
    let moved = value + step * direction;
    let digits = fraction_digits(Some(step)) as i32;
    let rounded = (moved * 10f64.powi(digits)).round() / 10f64.powi(digits);
    limit.clamp(rounded)
}

#[must_use]
pub fn snap(value: f64, min: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return value;
    }
    min + ((value - min) / step).round() * step
}

#[must_use]
pub fn level_unit(db: f32, floor_db: f32) -> f32 {
    if !db.is_finite() || db <= floor_db {
        return 0.0;
    }
    ((db - floor_db) / -floor_db).min(1.0)
}

#[must_use]
pub fn gate_db(level: Option<&ChannelLevel>, setting_db: Option<f32>) -> Option<f32> {
    level.and_then(|level| level.squelch_db).or(setting_db)
}

#[must_use]
pub fn gate_open(level: Option<&ChannelLevel>, setting_db: Option<f32>) -> bool {
    match (level, gate_db(level, setting_db)) {
        (Some(level), Some(gate)) => level.level_db >= gate,
        _ => false,
    }
}

#[must_use]
pub fn format_level(db: Option<f32>) -> String {
    match db {
        Some(db) if db.is_finite() && db > LEVEL_FLOOR_DB => format!("{db:.1} dB"),
        _ => String::from("-"),
    }
}

#[must_use]
pub fn commit_text(candidate: &str, value: &str, on_commit: impl FnOnce(&str) -> bool) -> String {
    let next = candidate.trim();
    if next != value && !on_commit(next) {
        value.to_owned()
    } else {
        next.to_owned()
    }
}

pub fn icon(svg: &'static str, class: &'static str) -> impl IntoView {
    zgui::elements::vector().class(class).document(svg)
}

pub fn tip(text: &'static str, child: AnyView) -> AnyView {
    AnyView::new(view! {
        Tooltip(delay = Duration::from_millis(250)) {
            TooltipTrigger {{child}}
            TooltipContent {{text}}
        }
    })
}

pub fn setting_row(
    label: impl Into<String>,
    title: Option<&'static str>,
    body: impl IntoView + 'static,
) -> AnyView {
    let label = label.into();
    let name = match title {
        None => AnyView::new(view! { text(class = "kc-name") {{label}} }),
        Some(title) => tip(
            title,
            AnyView::new(view! { text(class = "kc-name kc-name--tip") {{label}} }),
        ),
    };
    AnyView::new(view! {
        row(class = "kc-row") {
            {name}
            row(class = "kc-cell") {{body}}
        }
    })
}

pub fn toggle_row(
    label: &'static str,
    title: Option<&'static str>,
    on: Signal<bool>,
    change: impl Fn(bool) + 'static,
) -> AnyView {
    setting_row(label, title, check(on, change))
}

pub fn group(label: impl Into<String>, action: AnyView, body: impl IntoView + 'static) -> AnyView {
    let label = label.into();
    AnyView::new(view! {
        column(class = "kc-group") {
            row(class = "kc-group__head") {
                text(class = "kc-name") {{label}}
                {action}
            }
            {body}
        }
    })
}

pub fn button<M>(
    label: impl Into<String>,
    class: &'static str,
    disabled: impl IntoReactiveValue<bool, M> + 'static,
    press: impl Fn() + 'static,
) -> impl IntoView {
    let label = label.into();
    view! {
        control(
            class = class,
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            state:disabled = disabled,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click:stop = move |_| press()
        ) {
            {label}
        }
    }
}

pub fn readout_row(label: &'static str, value: impl IntoView + 'static) -> AnyView {
    AnyView::new(view! {
        row(class = "kc-readout__row") {
            text(class = "kc-readout__name") {{label}}
            row(class = "kc-readout__value") {{value}}
        }
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NumberSpec {
    pub label: &'static str,
    pub limit: NumberLimit,
    pub unit: Option<&'static str>,
    pub placeholder: &'static str,
    pub optional: bool,
    pub size: &'static str,
}

impl NumberSpec {
    #[must_use]
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn limit(mut self, limit: NumberLimit) -> Self {
        self.limit = limit;
        self
    }

    #[must_use]
    pub fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }

    #[must_use]
    pub fn optional(mut self, placeholder: &'static str) -> Self {
        self.optional = true;
        self.placeholder = placeholder;
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = placeholder;
        self
    }

    #[must_use]
    pub fn size(mut self, size: &'static str) -> Self {
        self.size = size;
        self
    }
}

#[must_use]
pub fn number_text(value: Option<f64>, step: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format_number(value, step))
}

#[must_use]
pub fn resolve_number(text: &str, spec: NumberSpec) -> Result<Option<f64>, ()> {
    if text.trim().is_empty() {
        return if spec.optional { Ok(None) } else { Err(()) };
    }
    parse_number(text)
        .map(|value| Some(spec.limit.clamp(value)))
        .ok_or(())
}

pub fn number_field(
    value: Signal<Option<f64>>,
    spec: NumberSpec,
    commit: impl Fn(Option<f64>) + Clone + 'static,
) -> impl IntoView {
    let step = spec.limit.step;
    let draft = RwSignal::new_local(number_text(value.get_untracked(), step));
    let focused = RwSignal::new_local(false);
    let sync = RenderEffect::new(move |_| {
        let current = value.get();
        if !focused.get() {
            draft.set(number_text(current, step));
        }
    });
    on_cleanup_local(move || drop(sync));
    let submit = {
        let commit = commit.clone();
        move || {
            let current = value.get_untracked();
            match resolve_number(&draft.get_untracked(), spec) {
                Ok(next) if next != current => {
                    draft.set(number_text(next, step));
                    commit(next);
                }
                _ => draft.set(number_text(current, step)),
            }
        }
    };
    let blur = submit.clone();
    let keys = move |ev: &mut EventCx<'_, events::KeyDown>| {
        let direction = match ev.key {
            Key::Named(NamedKey::Enter) => {
                submit();
                ev.prevent_default();
                return;
            }
            Key::Named(NamedKey::Escape) => {
                draft.set(number_text(value.get_untracked(), step));
                ev.prevent_default();
                return;
            }
            Key::Named(NamedKey::ArrowUp) => 1.0,
            Key::Named(NamedKey::ArrowDown) => -1.0,
            _ => return,
        };
        ev.prevent_default();
        let factor = if ev.modifiers.shift() {
            10.0
        } else if ev.modifiers.alt() {
            0.1
        } else {
            1.0
        };
        let base = parse_number(&draft.get_untracked())
            .or(value.get_untracked())
            .or(spec.limit.min)
            .unwrap_or(0.0);
        let next = step_value(base, step.unwrap_or(1.0) * factor, direction, spec.limit);
        draft.set(number_text(Some(next), step));
        commit(Some(next));
    };
    let size = if spec.size.is_empty() {
        "kc-num"
    } else {
        spec.size
    };
    let unit = spec.unit;
    view! {
        box(class = size, on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            {unit.map(|unit| view! { text(class = "kc-num__unit") {{unit}} })}
            Input(
                value = draft,
                class = "native-input",
                label = spec.label,
                placeholder = spec.placeholder,
                on:focus_in = move |_| focused.set(true),
                on:focus_out = move |_| { blur(); focused.set(false); },
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| { keys(ev); ev.stop_propagation(); },
            )
        }
    }
}

pub fn text_field(
    value: Signal<String>,
    label: &'static str,
    placeholder: &'static str,
    size: &'static str,
    commit: impl Fn(String) -> bool + Clone + 'static,
) -> impl IntoView {
    let draft = RwSignal::new_local(value.get_untracked());
    let focused = RwSignal::new_local(false);
    let sync = RenderEffect::new(move |_| {
        let current = value.get();
        if !focused.get() {
            draft.set(current);
        }
    });
    on_cleanup_local(move || drop(sync));
    let submit = move || {
        let current = value.get_untracked();
        let commit = commit.clone();
        draft.set(commit_text(&draft.get_untracked(), &current, move |next| {
            commit(next.to_owned())
        }));
    };
    let blur = submit.clone();
    view! {
        box(class = size, on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            Input(
                value = draft,
                class = "native-input",
                label = label,
                placeholder = placeholder,
                on:focus_in = move |_| focused.set(true),
                on:focus_out = move |_| { blur(); focused.set(false); },
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    match ev.key {
                        Key::Named(NamedKey::Enter) => { submit(); ev.prevent_default(); }
                        Key::Named(NamedKey::Escape) => { draft.set(value.get_untracked()); ev.prevent_default(); }
                        _ => {}
                    }
                    ev.stop_propagation();
                },
            )
        }
    }
}

pub struct Debounced {
    pub shown: Signal<f64>,
    pub pending: RwSignal<Option<f64>>,
}

pub fn debounced(
    value: Signal<f64>,
    commit: impl Fn(f64) + 'static,
) -> (Debounced, impl Fn(f64) + Clone) {
    let pending = RwSignal::new(None::<f64>);
    let held = StoredValue::new_local(None::<TimeoutHandle>);
    let clock = Timers::current();
    let commit = std::rc::Rc::new(commit);
    let flush = commit.clone();
    on_cleanup_local(move || {
        if let Some(value) = pending.try_get_untracked().flatten() {
            flush(value);
        }
    });
    let change = move |next: f64| {
        pending.set(Some(next));
        let commit = commit.clone();
        let handle = clock.as_ref().map(|clock| {
            clock.set_timeout(DEBOUNCE, move || {
                if let Some(value) = pending.get_untracked() {
                    commit(value);
                }
                pending.set(None);
            })
        });
        held.set_value(handle);
    };
    let shown = Signal::derive(move || pending.get().unwrap_or_else(|| value.get()));
    (Debounced { shown, pending }, change)
}

pub fn slider_field(
    value: Signal<f64>,
    range: (f64, f64, f64),
    disabled: Signal<bool>,
    read: impl Fn(f64) -> String + 'static,
    commit: impl Fn(f64) + 'static,
) -> impl IntoView {
    let (min, max, step) = range;
    let (held, change) = debounced(value, commit);
    let snapped = move |next: f64| {
        if !disabled.get_untracked() {
            change(snap(next, min, step).clamp(min, max));
        }
    };
    view! {
        row(class = "kc-slider", class:off = disabled) {
            {slide(held.shown, min, max, read, snapped)}
        }
    }
}

fn next_popover() -> u32 {
    NEXT_POPOVER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
pub struct Popover {
    owner: RwSignal<u32>,
    id: u32,
}

impl Popover {
    #[must_use]
    pub fn new() -> Self {
        let owner = use_context::<Menus>().map_or_else(|| RwSignal::new(0), |Menus(open)| open);
        Self {
            owner,
            id: next_popover(),
        }
    }

    #[must_use]
    pub fn open(self) -> bool {
        self.owner.get() == self.id
    }

    pub fn toggle(self) {
        self.owner.set(toggled(self.owner.get_untracked(), self.id));
    }

    pub fn close(self) {
        if self.owner.get_untracked() == self.id {
            self.owner.set(0);
        }
    }
}

impl Default for Popover {
    fn default() -> Self {
        Self::new()
    }
}

pub fn level_meter(
    level: Signal<Option<ChannelLevel>>,
    squelch_db: Signal<Option<f32>>,
) -> impl IntoView {
    let floor = SQUELCH_MIN_DB;
    let span = SQUELCH_MAX_DB - SQUELCH_MIN_DB;
    let now = move || {
        level_unit(
            level
                .get()
                .map_or(f32::NEG_INFINITY, |level| level.level_db),
            floor,
        )
    };
    let peak = move || {
        level_unit(
            level.get().map_or(f32::NEG_INFINITY, |level| level.peak_db),
            floor,
        )
    };
    let gate = move || gate_db(level.get().as_ref(), squelch_db.get());
    let open = move || gate().is_none() || gate_open(level.get().as_ref(), squelch_db.get());
    let heard = move || {
        level
            .get()
            .is_some_and(|level| level.level_db > LEVEL_FLOOR_DB)
    };
    let gaps = (1..LEVEL_SEGMENTS)
        .map(|at| {
            let left = format!("{}%", at as f32 * 100.0 / LEVEL_SEGMENTS as f32);
            view! { box(class = "kc-meter__gap", style:left = Some(left)) }
        })
        .collect::<Vec<_>>();
    view! {
        row(class = "kc-meter", a11y:role = Role::Meter, a11y:label = "Signal level") {
            box(class = "kc-meter__bar") {
                box(class = "kc-meter__fill", class:closed = move || !open(), style:width = move || Some(format!("{}%", now() * 100.0))) {}
                {gaps}
                {move || (peak() > 0.0).then(|| view! {
                    box(class = "kc-meter__peak", style:left = Some(format!("calc({}% - 1px)", peak() * 100.0)))
                })}
                {move || gate().map(|gate| view! {
                    box(class = "kc-meter__gate", style:left = Some(format!("calc({}% - 1px)", ((gate - floor) / span).clamp(0.0, 1.0) * 100.0)))
                })}
            }
            text(class = "kc-meter__read", class:heard = heard) {{move || format_level(level.get().map(|level| level.level_db))}}
        }
    }
}

pub fn tune_to(
    title: &'static str,
    hz: Signal<f64>,
    hint: Signal<String>,
    resolve: impl Fn(f64) -> Option<f64> + Clone + 'static,
    disabled: Signal<bool>,
    on_tune: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let popover = Popover::new();
    let text = RwSignal::new_local(String::new());
    let target = {
        let resolve = resolve.clone();
        move || parse_frequency(&text.get()).and_then(|entered| resolve(entered))
    };
    let typed_unit = move || text.get().chars().any(|c| c.is_ascii_alphabetic());
    let submit = {
        let target = target.clone();
        move || {
            if let Some(value) = target() {
                on_tune(value);
                popover.close();
            }
        }
    };
    let press = submit.clone();
    let invalid = {
        let target = target.clone();
        Signal::derive_local(move || target().is_none() && !text.get().trim().is_empty())
    };
    let no_target = Signal::derive_local(move || target().is_none());
    view! {
        box(class = "kc-pop") {
            {tip(title, AnyView::new(view! {
                control(
                    class = "kc-icon",
                    class:on = move || popover.open(),
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    a11y:label = title,
                    state:disabled = disabled,
                    on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                        ev.stop_propagation();
                        if !popover.open() {
                            text.set(format_number(hz.get_untracked() / 1e6, None));
                        }
                        popover.toggle();
                    }
                ) {
                    {icon(KEYBOARD_ICON, "kc-glyph")}
                }
            }))}
            {move || popover.open().then(|| {
                let submit = submit.clone();
                let press = press.clone();
                view! {
                    column(class = "kc-pop__panel", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
                        text(class = "kc-name") {"Frequency"}
                        row(class = "kc-pop__form") {
                            box(class = "kc-num kc-num--wide", class:invalid = invalid) {
                                {move || (!typed_unit()).then(|| view! { text(class = "kc-num__unit") {"MHz"} })}
                                Input(
                                    value = text,
                                    class = "native-input",
                                    label = "Frequency to tune to",
                                    invalid = invalid,
                                    on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                                        match ev.key {
                                            Key::Named(NamedKey::Enter) => { submit(); ev.prevent_default(); }
                                            Key::Named(NamedKey::Escape) => { popover.close(); ev.prevent_default(); }
                                            _ => {}
                                        }
                                        ev.stop_propagation();
                                    },
                                )
                            }
                            {button("Set", "kc-btn kc-btn--primary", no_target, move || press())}
                        }
                        text(class = "kc-note") {{move || hint.get()}}
                    }
                }
            })}
        }
    }
}

pub fn tuning_lock(
    locked: Signal<bool>,
    held: &'static str,
    free: &'static str,
    hold: Signal<Option<String>>,
    on_lock: impl Fn(bool) + Clone + 'static,
) -> impl IntoView {
    move || match hold.get() {
        Some(reason) => AnyView::new(view! {
            control(class = "kc-icon kc-icon--held", a11y:label = reason.clone(), a11y:tooltip = reason, state:disabled = true) {
                {icon(LOCK_ICON, "kc-glyph")}
            }
        }),
        None => {
            let on_lock = on_lock.clone();
            tip(
                if locked.get() { held } else { free },
                AnyView::new(view! {
                    control(
                        class = "kc-icon",
                        class:on = locked,
                        tabindex = Focus::Sequential,
                        a11y:role = Role::Button,
                        a11y:label = if locked.get_untracked() { "Unlock tuning" } else { "Lock tuning" },
                        on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                        on:click:stop = move |_| on_lock(!locked.get_untracked())
                    ) {
                        {move || if locked.get() { AnyView::new(icon(LOCK_ICON, "kc-glyph")) } else { AnyView::new(icon(UNLOCK_ICON, "kc-glyph")) }}
                    }
                }),
            )
        }
    }
}

pub fn blanker_control(
    blanker: Signal<NoiseBlankerSettings>,
    on_blanker: impl Fn(NoiseBlankerSettings) + Clone + 'static,
) -> AnyView {
    let enabled = Signal::derive(move || blanker.get().enabled);
    let threshold = Signal::derive(move || f64::from(blanker.get().threshold));
    let toggle = {
        let on_blanker = on_blanker.clone();
        move |on: bool| {
            let mut next = blanker.get_untracked();
            next.enabled = on;
            on_blanker(next);
        }
    };
    let slide_to = move |value: f64| {
        let mut next = blanker.get_untracked();
        next.threshold = value as f32;
        on_blanker(next);
    };
    setting_row(
        "Blanker",
        Some("Cuts impulse noise before the filter"),
        view! {
            {check(enabled, toggle)}
            {slider_field(
                threshold,
                (f64::from(MIN_BLANKER_THRESHOLD), f64::from(MAX_BLANKER_THRESHOLD), 0.5),
                Signal::derive(move || !enabled.get()),
                |value| format!("{value:.1}×"),
                slide_to,
            )}
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn si_picks_a_prefix_and_trims_the_zeros() {
        assert_eq!(format_hz(145_500_000.0), "145.5 MHz");
        assert_eq!(format_hz(12_500.0), "12.5 kHz");
        assert_eq!(format_hz(0.5), "0.5 Hz");
        assert_eq!(format_hz(2_400_000_000.0), "2.4 GHz");
        assert_eq!(format_hz(f64::NAN), "? Hz");
        assert_eq!(format_hz(460_275_000.0), "460.275 MHz");
    }

    #[test]
    fn megahertz_are_shown_to_four_places() {
        assert_eq!(format_mhz(145_500_000.0), "145.5000 MHz");
    }

    #[test]
    fn a_step_says_how_many_decimals_a_field_shows() {
        assert_eq!(fraction_digits(Some(1.0)), 0);
        assert_eq!(fraction_digits(Some(0.5)), 1);
        assert_eq!(fraction_digits(Some(0.0125)), 4);
        assert_eq!(fraction_digits(Some(0.00001)), 5);
        assert_eq!(fraction_digits(Some(500.0)), 0);
        assert_eq!(fraction_digits(None), 6);
        assert_eq!(fraction_digits(Some(0.0)), 6);
        assert_eq!(format_number(12.5, Some(0.5)), "12.5");
        assert_eq!(format_number(145.6, None), "145.6");
        assert_eq!(format_number(3300.0, Some(50.0)), "3300");
    }

    #[test]
    fn a_frequency_reads_with_or_without_its_unit() {
        assert_eq!(parse_frequency("145.5"), Some(145_500_000.0));
        assert_eq!(parse_frequency("145,5"), Some(145_500_000.0));
        assert_eq!(parse_frequency("433800k"), Some(433_800_000.0));
        assert_eq!(parse_frequency("2.4g"), Some(2_400_000_000.0));
        assert_eq!(parse_frequency(" 1090 MHz "), Some(1_090_000_000.0));
        assert_eq!(parse_frequency("77500 hz"), Some(77_500.0));
        assert_eq!(parse_frequency(".5"), Some(500_000.0));
        assert_eq!(parse_frequency(""), None);
        assert_eq!(parse_frequency("abc"), None);
        assert_eq!(parse_frequency("1.2.3"), None);
        assert_eq!(parse_frequency("5 parsecs"), None);
        assert_eq!(parse_frequency("-5"), None);
    }

    #[test]
    fn a_number_field_clamps_and_knows_when_empty_means_auto() {
        let spec = NumberSpec::new("x").limit(NumberLimit::new(5.0, 60.0, 1.0));
        assert_eq!(resolve_number("70", spec), Ok(Some(60.0)));
        assert_eq!(resolve_number("12", spec), Ok(Some(12.0)));
        assert_eq!(resolve_number("", spec), Err(()));
        assert_eq!(resolve_number("abc", spec), Err(()));
        assert_eq!(resolve_number(" ", spec.optional("auto")), Ok(None));
    }

    #[test]
    fn arrow_steps_land_on_whole_steps_inside_the_range() {
        let limit = NumberLimit::new(0.0, 1.0, 0.1);
        assert!((step_value(0.3, 0.1, 1.0, limit) - 0.4).abs() < 1e-12);
        assert_eq!(step_value(0.95, 0.1, 1.0, limit), 1.0);
        assert_eq!(step_value(0.0, 0.1, -1.0, limit), 0.0);
        assert_eq!(snap(7.26, 1.5, 0.5), 7.5);
    }

    #[test]
    fn the_level_reads_as_a_share_of_the_meter() {
        assert_eq!(level_unit(-60.0, -120.0), 0.5);
        assert_eq!(level_unit(-130.0, -120.0), 0.0);
        assert_eq!(level_unit(f32::NEG_INFINITY, -120.0), 0.0);
        assert_eq!(level_unit(10.0, -120.0), 1.0);
    }

    #[test]
    fn the_gate_prefers_what_the_channel_measured() {
        let level = ChannelLevel {
            channel: 1,
            level_db: -50.0,
            peak_db: -40.0,
            squelch_db: Some(-55.0),
        };
        assert_eq!(gate_db(Some(&level), Some(-70.0)), Some(-55.0));
        assert_eq!(gate_db(None, Some(-70.0)), Some(-70.0));
        assert!(gate_open(Some(&level), None));
        assert!(!gate_open(None, Some(-70.0)));
        assert_eq!(format_level(Some(-31.46)), "-31.5 dB");
        assert_eq!(format_level(Some(-150.0)), "-");
        assert_eq!(format_level(None), "-");
    }

    #[test]
    fn an_emptied_text_can_clear_and_a_refused_one_reverts() {
        let mut seen = Vec::new();
        assert_eq!(
            commit_text("  ", "TST", |value| {
                seen.push(value.to_owned());
                true
            }),
            ""
        );
        assert_eq!(seen, vec![String::new()]);
        assert_eq!(commit_text(" TST ", "TST", |_| panic!("unchanged")), "TST");
        assert_eq!(commit_text("BAD", "TST", |_| false), "TST");
    }
}
