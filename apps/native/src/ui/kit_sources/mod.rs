pub mod dial;
mod frequency;
pub mod icons;
mod sheet;
pub mod units;

use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use zgui::{prelude::*, reactive::RenderEffect, view::TimeoutHandle};
use zgui_ui::prelude::*;

pub use frequency::{DialSpec, frequency_dial, tune_to};

use crate::ui::widgets::{Menus, toggled};

static NEXT_POPOVER: AtomicU32 = AtomicU32::new(1 << 30);

pub fn install() {
    install_stylesheet("sources", sheet::SHEET);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Plain,
    Primary,
    Quiet,
    Danger,
}

impl Tone {
    const fn class(self) -> &'static str {
        match self {
            Self::Plain => "kit-btn",
            Self::Primary => "kit-btn kit-btn--primary",
            Self::Quiet => "kit-btn kit-btn--quiet",
            Self::Danger => "kit-btn kit-btn--danger",
        }
    }
}

pub fn button(
    label: impl Fn() -> String + Send + Sync + 'static,
    tone: Tone,
    disabled: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = tone.class(),
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            state:disabled = disabled,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click:stop = move |_| {
                if !disabled.get_untracked() {
                    on_press();
                }
            }
        ) {
            {label}
        }
    }
}

pub fn icon_button(
    svg: &'static str,
    label: &'static str,
    pressed: Signal<bool>,
    disabled: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = "kit-icon-btn",
            class:on = pressed,
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = label,
            state:disabled = disabled,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click:stop = move |_| {
                if !disabled.get_untracked() {
                    on_press();
                }
            }
        ) {
            {icons::icon(svg)}
        }
    }
}

pub fn segmented<T>(
    options: Vec<(T, String)>,
    chosen: Signal<T>,
    on_pick: impl Fn(T) + Clone + 'static,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let items: Vec<AnyView> = options
        .into_iter()
        .map(|(value, label)| {
            let on_pick = on_pick.clone();
            let marked = value.clone();
            AnyView::new(view! {
                control(
                    class = "seg__item",
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    class:on = move || chosen.get() == marked,
                    on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                    on:click:stop = move |_| on_pick(value.clone())
                ) {
                    {label}
                }
            })
        })
        .collect();
    view! { row(class = "seg kit-tabs") {{items}} }
}

pub fn footer(children: impl IntoView + 'static) -> impl IntoView {
    view! { row(class = "face__foot kit-foot") {{children}} }
}

pub fn readout(rows: Vec<(String, AnyView)>) -> impl IntoView {
    let cells: Vec<AnyView> = rows
        .into_iter()
        .map(|(label, value)| {
            AnyView::new(view! {
                row(class = "kit-read") {
                    text(class = "kit-read__name") {{label}}
                    box(class = "kit-read__value") {{value}}
                }
            })
        })
        .collect();
    view! { column(class = "kit-readout") {{cells}} }
}

pub fn group(label: impl Into<String>, children: impl IntoView + 'static) -> impl IntoView {
    let label = label.into();
    view! {
        column(class = "kit-group") {
            text(class = "kit-group__name") {{label}}
            {children}
        }
    }
}

pub fn collapsible(
    label: impl Fn() -> String + Send + Sync + 'static,
    class: &'static str,
    body: impl Fn() -> AnyView + 'static,
) -> impl IntoView {
    let open = RwSignal::new(false);
    view! {
        column(class = "kit-fold") {
            control(
                class = class,
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                on:click:stop = move |_| open.update(|open| *open = !*open)
            ) {
                {label}
            }
            {move || open.get().then(&body)}
        }
    }
}

pub fn popover_slot() -> (Signal<bool>, impl Fn() + Copy, impl Fn() + Copy) {
    let owner = use_context::<Menus>().map_or_else(|| RwSignal::new(0), |Menus(open)| open);
    let id = NEXT_POPOVER.fetch_add(1, Ordering::Relaxed);
    let open = Signal::derive(move || owner.get() == id);
    let toggle = move || owner.set(toggled(owner.get_untracked(), id));
    let close = move || {
        if owner.get_untracked() == id {
            owner.set(0);
        }
    };
    (open, toggle, close)
}

const DEBOUNCE: Duration = Duration::from_millis(150);

pub fn debounced(
    commit: impl Fn(f64) + Clone + 'static,
) -> (RwSignal<Option<f64>>, impl Fn(f64) + Clone) {
    let pending = RwSignal::new(None::<f64>);
    let timer = StoredValue::new_local(None::<TimeoutHandle>);
    let clock = Timers::current();
    let change = move |value: f64| {
        pending.set(Some(value));
        let commit = commit.clone();
        let Some(clock) = clock.as_ref() else {
            pending.set(None);
            commit(value);
            return;
        };
        let handle = clock.set_timeout(DEBOUNCE, move || {
            pending.try_set(None);
            commit(value);
        });
        timer.set_value(Some(handle));
    };
    (pending, change)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberSpec {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub unit: &'static str,
}

impl NumberSpec {
    #[must_use]
    pub const fn unit(unit: &'static str) -> Self {
        Self {
            min: None,
            max: None,
            step: None,
            unit,
        }
    }

    #[must_use]
    pub const fn within(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    #[must_use]
    pub const fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    #[must_use]
    pub fn show(&self, value: f64) -> String {
        let digits = units::fraction_digits(self.step);
        let fixed = format!("{value:.digits$}");
        if fixed.contains('.') {
            fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            fixed
        }
    }

    #[must_use]
    pub fn read(&self, text: &str) -> Option<f64> {
        let value: f64 = text.trim().replace(',', ".").parse().ok()?;
        if !value.is_finite() {
            return None;
        }
        let low = self.min.map_or(value, |min| value.max(min));
        Some(self.max.map_or(low, |max| low.min(max)))
    }
}

pub fn number_field(
    label: impl Into<String>,
    value: Signal<f64>,
    spec: NumberSpec,
    disabled: Signal<bool>,
    on_commit: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    number_field_in(label, spec.unit, value, spec, disabled, on_commit)
}

pub fn number_field_in(
    label: impl Into<String>,
    unit: impl Into<String>,
    value: Signal<f64>,
    spec: NumberSpec,
    disabled: Signal<bool>,
    on_commit: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let shown = Signal::derive(move || spec.show(value.get()));
    text_field(
        label,
        shown,
        unit,
        disabled,
        move |text| spec.read(&text).is_some(),
        move |text| {
            if let Some(next) = spec.read(&text)
                && (next - value.get_untracked()).abs() > f64::EPSILON
            {
                on_commit(next);
            }
        },
    )
}

pub fn text_field(
    label: impl Into<String>,
    value: Signal<String>,
    unit: impl Into<String>,
    disabled: Signal<bool>,
    valid: impl Fn(String) -> bool + Clone + 'static,
    on_commit: impl Fn(String) + Clone + 'static,
) -> impl IntoView {
    let label: String = label.into();
    let unit: String = unit.into();
    let draft = RwSignal::new_local(value.get_untracked());
    let focused = RwSignal::new_local(false);
    let sync = RenderEffect::new(move |_| {
        let current = value.get();
        if !focused.get() {
            draft.set(current);
        }
    });
    on_cleanup_local(move || drop(sync));
    let check = valid.clone();
    let invalid = Signal::derive_local(move || !check(draft.get()));
    let commit = move || {
        let text = draft.get_untracked();
        if valid(text.clone()) {
            if text != value.get_untracked() {
                on_commit(text);
            }
        } else {
            draft.set(value.get_untracked());
        }
    };
    let blur = commit.clone();
    view! {
        row(class = "kit-field", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            Input(
                value = draft,
                class = "native-input",
                label = label,
                disabled = Signal::derive_local(move || disabled.get()),
                invalid = invalid,
                on:focus_in = move |_| focused.set(true),
                on:focus_out = move |_| { blur(); focused.set(false); },
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    match ev.key {
                        Key::Named(NamedKey::Enter) => { commit(); ev.prevent_default(); }
                        Key::Named(NamedKey::Escape) => { draft.set(value.get_untracked()); ev.prevent_default(); }
                        _ => {}
                    }
                },
            )
            text(class = "kit-field__unit", hidden = unit.is_empty()) {{unit}}
        }
    }
}

pub fn draft_field(
    label: &'static str,
    draft: RwSignal<String, LocalStorage>,
    placeholder: &'static str,
    invalid: Signal<bool, LocalStorage>,
    on_enter: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        row(class = "kit-field", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            Input(
                value = draft,
                class = "native-input",
                label = label,
                placeholder = placeholder,
                invalid = invalid,
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    if matches!(ev.key, Key::Named(NamedKey::Enter)) {
                        on_enter();
                        ev.prevent_default();
                    }
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_shows_as_many_decimals_as_its_step_and_reads_back_clamped() {
        let spec = NumberSpec::unit("MHz").within(0.0, 10.0).step(0.001);
        assert_eq!(spec.show(2.4), "2.4");
        assert_eq!(spec.show(2.40049), "2.4");
        assert_eq!(spec.read("3,5"), Some(3.5));
        assert_eq!(spec.read("40"), Some(10.0));
        assert_eq!(spec.read("-1"), Some(0.0));
        assert_eq!(spec.read("abc"), None);
        assert_eq!(NumberSpec::unit("ppm").step(1.0).show(12.0), "12");
    }
}
