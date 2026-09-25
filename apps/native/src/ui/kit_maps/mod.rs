pub mod feed;
pub mod trail;
pub mod wiring;

use std::time::Duration;

use zgui::prelude::*;
use zgui::reactive::RenderEffect;
use zgui_ui::prelude::*;

pub fn ticker(every: Duration) -> RwSignal<i64> {
    let now = RwSignal::new(now_ms());
    let handle = set_interval(every, move || now.set(now_ms()));
    on_cleanup_local(move || drop(handle));
    now
}

pub fn every(period: Duration, run: impl FnMut() + 'static) {
    let handle = set_interval(period, run);
    on_cleanup_local(move || drop(handle));
}

pub fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

#[must_use]
pub fn millis_of(iso: &str) -> Option<i64> {
    iso.parse::<jiff::Timestamp>()
        .ok()
        .map(|at| at.as_millisecond())
}

#[must_use]
pub fn iso_of(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(|_| String::new(), |at| format!("{at:.3}"))
}

#[must_use]
pub fn utc_clock(ms: i64) -> String {
    let iso = iso_of(ms);
    iso.get(11..19)
        .map_or_else(String::new, |clock| format!("{clock}Z"))
}

#[must_use]
pub fn mhz(hz: f64) -> String {
    format!("{:.3} MHz", hz / 1e6)
}

pub fn armed_clear(enabled: Signal<bool>, on_clear: impl Fn() + 'static) -> impl IntoView {
    let armed = RwSignal::new(false);
    view! {
        control(
            class = "btn",
            class:danger = move || armed.get(),
            state:disabled = move || !enabled.get(),
            on:pointer_down:stop = |_| {},
            on:focus_out = move |_| armed.set(false),
            on:click:stop = move |_| {
                if armed.get_untracked() {
                    armed.set(false);
                    on_clear();
                } else {
                    armed.set(true);
                }
            }
        ) {
            {move || if armed.get() { "Confirm clear" } else { "Clear" }}
        }
    }
}

pub fn entry(
    label: &'static str,
    value: Signal<String>,
    disabled: Signal<bool>,
    commit: impl Fn(String) + 'static,
) -> impl IntoView {
    let text = RwSignal::new_local(value.get_untracked());
    let syncing = RenderEffect::new(move |_| text.set(value.get()));
    on_cleanup_local(move || drop(syncing));
    let commit = std::rc::Rc::new(commit);
    let on_leave = commit.clone();
    view! {
        column(class = "entry geo__entry", on:pointer_down:stop = |_| {}) {
            text(class = "geo__label") {{label}}
            Input(
                class = "native-input geo__input",
                value = text,
                label = label,
                disabled = Signal::derive_local(move || disabled.get()),
                on:focus_out = move |_| on_leave(text.get_untracked()),
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    if matches!(ev.key, Key::Named(NamedKey::Enter)) {
                        commit(text.get_untracked());
                    }
                }
            )
        }
    }
}

pub const SHEET: &str = css!(
    r#"
.geo { flex-direction: column; flex: 1 1 auto; min-height: 0; }
.geo__bar {
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    background-color: var(--panel-2);
}
.geo__strip {
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 3px 10px;
    border-bottom: 1px solid var(--line);
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-dim);
}
.geo__strip .on { color: var(--accent); }
.geo__num { color: var(--ink); }
.geo__pair { gap: 4px; align-items: baseline; }
.geo__check { align-items: center; gap: 5px; }
.geo__entry { gap: 2px; }
.geo__note {
    padding: 3px 10px;
    border-top: 1px solid var(--line);
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
}
.geo__note.bad, .geo__bad { color: var(--danger); }
.geo__legend {
    position: absolute;
    top: 8px;
    left: 8px;
    flex-direction: column;
    gap: 3px;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--bg);
    font-family: var(--mono);
    font-size: 10px;
    pointer-events: none;
}
.geo__legend-row { align-items: center; gap: 8px; }
.geo__legend-name { color: var(--ink-dim); flex: 1 1 auto; }
.geo__legend-count { color: var(--ink); }
.geo__swatch { width: 8px; height: 8px; border-radius: 4px; }
.geo__ramp { height: 6px; border-radius: 3px; min-width: 140px; }
.geo__ends { justify-content: space-between; color: var(--ink-faint); }
.geo__table { flex-direction: column; max-height: 160px; overflow: auto; border-top: 1px solid var(--line); }
.geo__row { gap: 0; border-bottom: 1px solid var(--line); font-family: var(--mono); font-size: 10px; }
.geo__row.head { background-color: var(--panel-2); color: var(--ink-faint); }
.geo__cell { flex: 1 1 0; padding: 2px 6px; }
.geo__label { font-size: 11px; color: var(--ink-faint); }
.geo__input { width: 90px; }
.btn.danger { border-color: var(--danger); color: var(--danger); }
.btn.primary { border-color: var(--accent-dim); color: var(--accent); }
.btn:disabled { opacity: 0.45; }
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_read_and_write_as_utc_milliseconds() {
        let at = millis_of("2026-08-15T10:00:00Z").expect("a time");
        assert_eq!(iso_of(at), "2026-08-15T10:00:00.000Z");
        assert_eq!(utc_clock(at), "10:00:00Z");
        assert!(millis_of("not a time").is_none());
        assert_eq!(mhz(14_074_000.0), "14.074 MHz");
    }
}
