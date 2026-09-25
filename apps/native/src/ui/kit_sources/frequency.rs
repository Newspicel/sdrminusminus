use zgui::{
    geom::{Css, CssPx, Point},
    prelude::*,
    reactive::RenderEffect,
};
use zgui_ui::prelude::*;

use super::{
    Tone, button,
    dial::{Reach, dial_digits, dial_places, parse_frequency, set_dial_digit, step_dial},
    icon_button, icons, install, popover_slot,
};

const PIXELS_PER_STEP: f32 = 24.0;
const DEFAULT_PLACE: i32 = 6;

#[derive(Clone, Copy)]
pub struct DialSpec {
    pub hz: Signal<f64>,
    pub reach: Signal<Reach>,
    pub disabled: Signal<bool>,
    pub wheel: Signal<bool>,
}

#[derive(Clone, Copy)]
struct Dial {
    spec: DialSpec,
    places: Memo<Vec<i32>>,
    active: RwSignal<i32>,
    spin: RwSignal<f32>,
}

impl Dial {
    fn place(self) -> i32 {
        current_place(&self.places.get_untracked(), self.active.get_untracked())
    }

    fn step(self, place: i32, direction: i32) -> f64 {
        step_dial(
            self.spec.hz.get_untracked(),
            place,
            direction,
            self.spec.reach.get_untracked(),
        )
    }
}

fn current_place(places: &[i32], active: i32) -> i32 {
    if places.contains(&active) {
        active
    } else {
        places.first().copied().unwrap_or(DEFAULT_PLACE)
    }
}

fn neighbour(places: &[i32], place: i32, by: isize) -> i32 {
    let at = places.iter().position(|p| *p == place).unwrap_or(0) as isize;
    let last = places.len().saturating_sub(1) as isize;
    places
        .get((at + by).clamp(0, last) as usize)
        .copied()
        .unwrap_or(place)
}

fn wheel_steps(delta: ScrollDelta, spin: RwSignal<f32>) -> i32 {
    match delta {
        ScrollDelta::Lines { y, .. } if y < 0.0 => 1,
        ScrollDelta::Lines { y, .. } if y > 0.0 => -1,
        ScrollDelta::Pixels(size) => {
            let total = spin.get_untracked() - size.height.0;
            let steps = (total / PIXELS_PER_STEP).trunc();
            spin.set(total - steps * PIXELS_PER_STEP);
            steps as i32
        }
        _ => 0,
    }
}

fn half_at(el: NodeRef, at: Point<CssPx, Css>) -> i32 {
    let Some(bounds) = el.window_bounds() else {
        return 1;
    };
    let scale = if el.scale() > 0.0 { el.scale() } else { 1.0 };
    let middle = (bounds.top().0 + bounds.height().0 / 2.0) / scale;
    if at.y.0 < middle { 1 } else { -1 }
}

pub fn frequency_dial(spec: DialSpec, on_tune: impl Fn(f64) + Clone + 'static) -> impl IntoView {
    install();
    let dial = Dial {
        spec,
        places: Memo::new(move |_| dial_places(spec.reach.get().max)),
        active: RwSignal::new(DEFAULT_PLACE),
        spin: RwSignal::new(0.0),
    };
    let editing = RwSignal::new(false);
    let keys = {
        let on_tune = on_tune.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            if spec.disabled.get_untracked() {
                return;
            }
            if press_key(dial, editing, &ev.key, &on_tune) {
                ev.prevent_default();
                ev.stop_propagation();
            }
        }
    };
    let digits = {
        let on_tune = on_tune.clone();
        move || {
            dial.places
                .get()
                .into_iter()
                .map(|place| digit(dial, place, on_tune.clone()))
                .collect::<Vec<_>>()
        }
    };
    view! {
        box(class = "kit-dial-wrap") {
            if move || editing.get() {
                {direct_entry(dial, editing, on_tune.clone())}
            } else {
                row(
                    class = "kit-dial",
                    class:off = spec.disabled,
                    tabindex = Focus::Sequential,
                    a11y:role = Role::SpinButton,
                    a11y:label = "Tuned frequency",
                    on:key_down = keys.clone()
                ) {
                    {digits.clone()}
                    text(class = "kit-dial__unit") {"MHz"}
                }
            }
        }
    }
}

fn press_key(dial: Dial, editing: RwSignal<bool>, key: &Key, on_tune: &impl Fn(f64)) -> bool {
    let places = dial.places.get_untracked();
    let place = dial.place();
    match key {
        Key::Named(NamedKey::ArrowUp) => on_tune(dial.step(place, 1)),
        Key::Named(NamedKey::ArrowDown) => on_tune(dial.step(place, -1)),
        Key::Named(NamedKey::PageUp) => on_tune(dial.step(place + 1, 1)),
        Key::Named(NamedKey::PageDown) => on_tune(dial.step(place + 1, -1)),
        Key::Named(NamedKey::ArrowLeft) => dial.active.set(neighbour(&places, place, -1)),
        Key::Named(NamedKey::ArrowRight) => dial.active.set(neighbour(&places, place, 1)),
        Key::Named(NamedKey::Home) => dial.active.set(places.first().copied().unwrap_or(place)),
        Key::Named(NamedKey::End) => dial.active.set(places.last().copied().unwrap_or(place)),
        Key::Named(NamedKey::Enter) => editing.set(true),
        _ => {
            let Some(typed) = key
                .as_str()
                .and_then(|text| text.parse::<u8>().ok())
                .filter(|digit| *digit < 10)
            else {
                return false;
            };
            on_tune(set_dial_digit(
                dial.spec.hz.get_untracked(),
                place,
                typed,
                dial.spec.reach.get_untracked(),
            ));
            dial.active.set(neighbour(&places, place, 1));
        }
    }
    true
}

fn digit(dial: Dial, place: i32, on_tune: impl Fn(f64) + Clone + 'static) -> AnyView {
    let el = NodeRef::new();
    let armed = RwSignal::new(0i32);
    let spec = dial.spec;
    let shown = move || {
        let places = dial.places.get();
        dial_digits(spec.hz.get(), &places)
            .into_iter()
            .find(|digit| digit.place == place)
    };
    let leading = move || shown().is_some_and(|digit| digit.leading);
    let press = {
        let on_tune = on_tune.clone();
        move |ev: &mut EventCx<'_, events::PointerDown>| {
            if ev.button != Some(PointerButton::Primary) || spec.disabled.get_untracked() {
                return;
            }
            ev.stop_propagation();
            dial.active.set(place);
            on_tune(dial.step(place, half_at(el, ev.position)));
        }
    };
    let wheel = move |ev: &mut EventCx<'_, events::Wheel>| {
        let modified = ev.modifiers.control() || ev.modifiers.meta();
        if spec.disabled.get_untracked() || !spec.wheel.get_untracked() || modified {
            return;
        }
        ev.prevent_default();
        ev.stop_propagation();
        let steps = wheel_steps(ev.delta, dial.spin);
        if steps != 0 {
            dial.active.set(place);
            on_tune(dial.step(place, steps.signum()));
        }
    };
    let separator = match place {
        6 => Some("."),
        3 => Some(" "),
        _ => None,
    };
    AnyView::new(view! {
        box(
            node_ref = el,
            class = "kit-digit",
            class:on = move || current_place(&dial.places.get(), dial.active.get()) == place,
            class:dim = leading,
            class:up = move || armed.get() > 0,
            class:down = move || armed.get() < 0,
            a11y:label = format!("{} hertz digit", 10u64.pow(place.max(0) as u32)),
            on:pointer_down = press,
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| {
                if !spec.disabled.get_untracked() {
                    let half = half_at(el, ev.position);
                    if armed.get_untracked() != half {
                        armed.set(half);
                    }
                }
            },
            on:pointer_leave = move |_| armed.set(0),
            on:wheel = wheel
        ) {
            box(class = "kit-digit__half")
            text(class = "kit-digit__glyph") {{move || shown().map_or(0, |digit| digit.digit).to_string()}}
        }
        {separator.map(|mark| AnyView::new(view! { text(class = "kit-digit__sep") {{mark}} }))}
    })
}

fn direct_entry(
    dial: Dial,
    editing: RwSignal<bool>,
    on_tune: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let text = RwSignal::new_local(String::new());
    let field = NodeRef::new();
    let focus = RenderEffect::new(move |_| {
        if field.get().is_some() {
            field.focus();
        }
    });
    on_cleanup_local(move || drop(focus));
    let invalid = Signal::derive_local(move || {
        let typed = text.get();
        !typed.trim().is_empty() && parse_frequency(&typed).is_none()
    });
    view! {
        box(class = "kit-entry", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            Input(
                node_ref = field,
                value = text,
                class = "native-input",
                label = "Tune to frequency",
                placeholder = "145.5 · 433800k · 2.4g",
                invalid = invalid,
                on:focus_out = move |_| editing.set(false),
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    match ev.key {
                        Key::Named(NamedKey::Enter) => {
                            if let Some(hz) = parse_frequency(&text.get_untracked()) {
                                editing.set(false);
                                on_tune(dial.spec.reach.get_untracked().clamp(hz));
                            }
                            ev.prevent_default();
                        }
                        Key::Named(NamedKey::Escape) => {
                            editing.set(false);
                            ev.prevent_default();
                        }
                        _ => {}
                    }
                },
            )
        }
    }
}

pub fn tune_to(
    title: &'static str,
    hz: Signal<f64>,
    hint: Signal<String>,
    resolve: impl Fn(f64) -> Option<f64> + Clone + Send + Sync + 'static,
    disabled: Signal<bool>,
    on_tune: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    install();
    let (open, toggle, close) = popover_slot();
    let form = move || {
        let text = RwSignal::new_local(format!("{}", hz.get_untracked() / 1e6));
        let resolve = resolve.clone();
        let target = Signal::derive_local(move || parse_frequency(&text.get()).and_then(&resolve));
        let typed_unit =
            Signal::derive_local(move || text.get().chars().any(|c| c.is_ascii_alphabetic()));
        let submit = {
            let on_tune = on_tune.clone();
            move || {
                if let Some(value) = target.get_untracked() {
                    on_tune(value);
                    close();
                }
            }
        };
        let set = submit.clone();
        let invalid =
            Signal::derive_local(move || target.get().is_none() && !text.get().trim().is_empty());
        AnyView::new(view! {
            column(class = "kit-pop", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
                text(class = "legend") {"Frequency"}
                row(class = "kit-pop__row") {
                    row(class = "kit-field") {
                        Input(
                            value = text,
                            class = "native-input",
                            label = "Frequency to tune to",
                            invalid = invalid,
                            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                                match ev.key {
                                    Key::Named(NamedKey::Enter) => { submit(); ev.prevent_default(); }
                                    Key::Named(NamedKey::Escape) => { close(); ev.prevent_default(); }
                                    _ => {}
                                }
                            },
                        )
                        text(class = "kit-field__unit", hidden = move || typed_unit.get()) {"MHz"}
                    }
                    {button(|| "Set".to_owned(), Tone::Primary, Signal::derive(move || target.with(Option::is_none)), set)}
                }
                text(class = "legend kit-pop__hint") {{move || hint.get()}}
            }
        })
    };
    let form = move || open.get().then(&form);
    view! {
        box(class = "kit-pop-wrap", a11y:description = title) {
            {icon_button(icons::KEYBOARD, title, open, disabled, toggle)}
            {form}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dial_addresses_the_megahertz_digit_until_told_otherwise() {
        let places = vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        assert_eq!(current_place(&places, DEFAULT_PLACE), 6);
        assert_eq!(current_place(&places, 11), 9);
        assert_eq!(neighbour(&places, 6, 1), 5);
        assert_eq!(neighbour(&places, 9, -1), 9);
        assert_eq!(neighbour(&places, 0, 1), 0);
    }
}
