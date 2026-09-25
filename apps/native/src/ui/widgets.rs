use std::sync::atomic::{AtomicU32, Ordering};

use zgui::{
    geom::{Css, CssPx, Device, DevicePx, Rect},
    prelude::*,
};

use crate::format::{self, DIAL_DIGITS};

static NEXT_MENU: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy)]
pub struct Menus(pub RwSignal<u32>);

pub fn provide_menus() -> Menus {
    let menus = Menus(RwSignal::new(0));
    provide_context(menus);
    menus
}

pub fn close_menus() {
    if let Some(Menus(open)) = use_context::<Menus>() {
        open.set(0);
    }
}

#[must_use]
pub fn toggled(current: u32, id: u32) -> u32 {
    if current == id { 0 } else { id }
}

fn menu_owner() -> RwSignal<u32> {
    use_context::<Menus>().map_or_else(|| RwSignal::new(0), |Menus(open)| open)
}

pub fn track(
    bounds: Option<Rect<DevicePx, Device>>,
    at: Point<CssPx, Css>,
    scale: f32,
) -> Option<f32> {
    let bounds = bounds?;
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let left = bounds.left().0 / scale;
    let width = bounds.width().0 / scale;
    (width > 0.0).then(|| ((at.x.0 - left) / width).clamp(0.0, 1.0))
}

pub type Point<T, S> = zgui::geom::Point<T, S>;

pub fn dial(hz: Signal<f64>, on_change: impl Fn(f64) + Clone + 'static) -> impl IntoView {
    let picked = RwSignal::new(3usize);
    let wheel = {
        let on_change = on_change.clone();
        move |ev: &mut EventCx<'_, events::Wheel>| {
            let steps = match ev.delta {
                ScrollDelta::Lines { y, .. } => -y,
                ScrollDelta::Pixels(size) => -size.height.0 / 40.0,
                _ => 0.0,
            };
            let steps = if steps > 0.0 {
                1
            } else if steps < 0.0 {
                -1
            } else {
                0
            };
            if steps != 0 {
                on_change(format::nudge(
                    hz.get_untracked(),
                    picked.get_untracked(),
                    steps,
                ));
            }
            ev.prevent_default();
        }
    };
    let keys = {
        let on_change = on_change.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            let at = picked.get_untracked();
            let value = hz.get_untracked();
            match &ev.key {
                Key::Named(NamedKey::ArrowUp) => on_change(format::nudge(value, at, 1)),
                Key::Named(NamedKey::ArrowDown) => on_change(format::nudge(value, at, -1)),
                Key::Named(NamedKey::ArrowLeft) => picked.set(at.saturating_sub(1)),
                Key::Named(NamedKey::ArrowRight) => picked.set((at + 1).min(DIAL_DIGITS - 1)),
                _ => return,
            }
            ev.prevent_default();
            ev.stop_propagation();
        }
    };

    let mut digits: Vec<AnyView> = Vec::with_capacity(DIAL_DIGITS + 1);
    for index in 0..DIAL_DIGITS {
        digits.push(AnyView::new(view! {
            control(
                class = "digit",
                class:on = move || picked.get() == index,
                class:dim = move || leading_zero(hz.get(), index),
                on:pointer_down:stop = move |_| picked.set(index)
            ) {
                {move || digit_text(hz.get(), index)}
            }
        }));
        if index == 3 {
            digits.push(AnyView::new(view! { box(class = "digit dot") {"."} }));
        }
    }

    view! {
        row(
            class = "dial",
            tabindex = Focus::Sequential,
            a11y:role = Role::Group,
            a11y:label = "Frequency",
            on:wheel = wheel,
            on:key_down = keys
        ) {
            {digits}
            text(class = "dial__unit") {"MHz"}
        }
    }
}

fn digit_text(hz: f64, index: usize) -> String {
    format::dial_digits(hz)
        .get(index)
        .copied()
        .unwrap_or(0)
        .to_string()
}

fn leading_zero(hz: f64, index: usize) -> bool {
    let digits = format::dial_digits(hz);
    index < 3 && digits[..=index].iter().all(|digit| *digit == 0)
}

pub fn row_field(name: impl Into<String>, body: impl IntoView + 'static) -> impl IntoView {
    let name = name.into();
    view! {
        row(class = "field") {
            label(class = "field__name") {{name}}
            row(class = "field__body") {{body}}
        }
    }
}

pub fn pick<T>(
    options: Vec<(T, String)>,
    chosen: Signal<Option<T>>,
    on_pick: impl Fn(T) + Clone + 'static,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let owner = menu_owner();
    let id = NEXT_MENU.fetch_add(1, Ordering::Relaxed);
    let open = Signal::derive(move || owner.get() == id);
    let shown = {
        let options = options.clone();
        move || {
            let chosen = chosen.get();
            options
                .iter()
                .find(|(value, _)| Some(value) == chosen.as_ref())
                .map_or_else(|| String::from("—"), |(_, label)| label.clone())
        }
    };
    let keys = {
        let options = options.clone();
        let on_pick = on_pick.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            let current = options
                .iter()
                .position(|(value, _)| chosen.get_untracked().as_ref() == Some(value));
            let index = match ev.key {
                Key::Named(NamedKey::ArrowDown) => choice_index(current, options.len(), 1),
                Key::Named(NamedKey::ArrowUp) => choice_index(current, options.len(), -1),
                Key::Named(NamedKey::Home) => (!options.is_empty()).then_some(0),
                Key::Named(NamedKey::End) => options.len().checked_sub(1),
                Key::Named(NamedKey::Escape) => {
                    owner.set(0);
                    None
                }
                Key::Named(NamedKey::Enter) => {
                    owner.set(toggled(owner.get_untracked(), id));
                    None
                }
                _ if ev.key.inserted_text() == Some(" ") => {
                    owner.set(toggled(owner.get_untracked(), id));
                    None
                }
                _ => return,
            };
            if let Some(index) = index {
                on_pick(options[index].0.clone());
            }
            ev.prevent_default();
        }
    };
    let rows = move || {
        options
            .iter()
            .map(|(value, label)| {
                let on_pick = on_pick.clone();
                let picked = value.clone();
                let marked = value.clone();
                let label = label.clone();
                view! {
                    control(
                        class = "menu__row",
                        tabindex = Focus::Sequential,
                        a11y:role = Role::Button,
                        class:on = move || chosen.get().as_ref() == Some(&marked),
                        on:pointer_down:stop = move |_| {
                            owner.set(0);
                            on_pick(picked.clone());
                        }
                    ) {
                        {label}
                    }
                }
            })
            .collect::<Vec<_>>()
    };

    view! {
        box(class = "pick__wrap") {
            control(
                class = "pick",
                on:key_down = keys,
                a11y:role = Role::Button,
                class:on = move || open.get(),
                tabindex = Focus::Sequential,
                on:pointer_down:stop = move |_| owner.set(toggled(owner.get_untracked(), id))
            ) {
                text {{shown}}
                text(class = "pick__caret") {"v"}
            }
            {move || open.get().then(|| AnyView::new(view! {
                column(class = "menu") {{rows()}}
            }))}
        }
    }
}

fn choice_index(current: Option<usize>, count: usize, direction: i32) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match current {
        Some(index) if direction < 0 => index.saturating_sub(1),
        Some(index) => (index + 1).min(last),
        None if direction < 0 => last,
        None => 0,
    })
}

pub fn segments<T>(
    options: Vec<(T, &'static str)>,
    chosen: Signal<T>,
    on_pick: impl Fn(T) + Clone + 'static,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let items: Vec<_> = options
        .into_iter()
        .map(|(value, label)| {
            let on_pick = on_pick.clone();
            let picked = value.clone();
            let marked = value.clone();
            view! {
                control(
                    class = "seg__item",
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    class:on = move || chosen.get() == marked,
                    on:click:stop = move |_| on_pick(picked.clone())
                ) {
                    {label}
                }
            }
        })
        .collect();
    view! { row(class = "seg") {{items}} }
}

pub fn slide(
    value: Signal<f64>,
    min: f64,
    max: f64,
    read: impl Fn(f64) -> String + 'static,
    on_change: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let span = if (max - min).abs() > f64::EPSILON {
        max - min
    } else {
        1.0
    };
    let fraction = move || ((value.get() - min) / span).clamp(0.0, 1.0) as f32 * 100.0;
    let rail = NodeRef::new();
    let held = RwSignal::new(false);
    let seek = {
        let on_change = on_change.clone();
        move |at: Point<CssPx, Css>| {
            if let Some(at) = track(rail.window_bounds(), at, rail.scale()) {
                on_change(min + span * f64::from(at));
            }
        }
    };
    let press = {
        let seek = seek.clone();
        move |ev: &mut EventCx<'_, events::PointerDown>| {
            if ev.button != Some(PointerButton::Primary) {
                return;
            }
            ev.stop_propagation();
            ev.capture_pointer();
            held.set(true);
            seek(ev.position);
        }
    };
    let drag = move |ev: &mut EventCx<'_, events::PointerMove>| {
        if held.get_untracked() {
            seek(ev.position);
        }
    };
    let release = move |ev: &mut EventCx<'_, events::PointerUp>| {
        ev.release_pointer();
        held.set(false);
    };
    let keys = {
        let on_change = on_change.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            let step = span / 100.0;
            let now = value.get_untracked();
            let next = match &ev.key {
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => now - step,
                Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => now + step,
                Key::Named(NamedKey::Home) => min,
                Key::Named(NamedKey::End) => max,
                _ => return,
            };
            ev.prevent_default();
            on_change(next.clamp(min.min(max), max.max(min)));
        }
    };

    view! {
        row(class = "field__body") {
            box(
                node_ref = rail,
                class = "slide",
                class:held = move || held.get(),
                tabindex = Focus::Sequential,
                a11y:role = Role::Slider,
                on:pointer_down = press,
                on:pointer_move = drag,
                on:pointer_up = release,
                on:pointer_cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
                    ev.release_pointer();
                    held.set(false);
                },
                on:key_down = keys
            ) {
                box(class = "slide__rail")
                box(class = "slide__fill", style:width = move || Some(format!("{}%", fraction())))
                box(class = "slide__grip", style:left = move || Some(format!("{}%", fraction())))
            }
            text(class = "slide__read") {{move || read(value.get())}}
        }
    }
}

pub fn check(on: Signal<bool>, on_change: impl Fn(bool) + 'static) -> impl IntoView {
    view! {
        control(
            class = "check",
            class:on = move || on.get(),
            tabindex = Focus::Sequential,
            a11y:role = Role::CheckBox,
            on:click:stop = move |_| on_change(!on.get_untracked())
        ) {
            "x"
        }
    }
}

pub fn meter(level_db: Signal<f32>) -> impl IntoView {
    view! { row(class = "field__body") {{level_bar(level_db)}} }
}

pub fn level_bar(level_db: Signal<f32>) -> impl IntoView {
    let fraction = move || ((level_db.get() + 90.0) / 90.0).clamp(0.0, 1.0) * 100.0;
    let read = move || {
        let value = level_db.get();
        if value <= -119.0 {
            String::from("--")
        } else {
            format::decibels(value)
        }
    };
    view! {
        row(class = "bar") {
            box(class = "meter") {
                box(class = "meter__fill", style:width = move || Some(format!("{}%", fraction())))
            }
            text(class = "meter__read") {{read}}
        }
    }
}

#[cfg(test)]
mod tests {
    use zgui::geom::Size;

    use super::*;

    fn rect(left: f32, width: f32) -> Rect<DevicePx, Device> {
        Rect::new(
            Point::new(DevicePx(left), DevicePx(0.0)),
            Size::new(DevicePx(width), DevicePx(10.0)),
        )
    }

    fn at(x: f32) -> Point<CssPx, Css> {
        Point::new(CssPx(x), CssPx(0.0))
    }

    #[test]
    fn keyboard_choices_stay_in_range_and_start_at_the_nearest_end() {
        assert_eq!(choice_index(None, 0, 1), None);
        assert_eq!(choice_index(None, 3, 1), Some(0));
        assert_eq!(choice_index(None, 3, -1), Some(2));
        assert_eq!(choice_index(Some(0), 3, -1), Some(0));
        assert_eq!(choice_index(Some(2), 3, 1), Some(2));
        assert_eq!(choice_index(Some(1), 3, -1), Some(0));
    }

    #[test]
    fn a_press_lands_where_it_fell_on_the_track() {
        let bounds = Some(rect(100.0, 400.0));
        assert_eq!(track(bounds, at(50.0), 2.0), Some(0.0));
        assert_eq!(track(bounds, at(150.0), 2.0), Some(0.5));
        assert_eq!(track(bounds, at(250.0), 2.0), Some(1.0));
    }

    #[test]
    fn a_track_with_no_box_and_one_with_no_width_move_nothing() {
        assert_eq!(track(None, at(10.0), 1.0), None);
        assert_eq!(track(Some(rect(0.0, 0.0)), at(10.0), 1.0), None);
    }

    #[test]
    fn a_missing_scale_is_read_as_one_to_one() {
        assert_eq!(track(Some(rect(0.0, 100.0)), at(25.0), 0.0), Some(0.25));
    }

    #[test]
    fn a_menu_closes_when_it_is_pressed_again_and_steals_the_slot_from_another() {
        assert_eq!(toggled(0, 7), 7);
        assert_eq!(toggled(7, 7), 0);
        assert_eq!(toggled(3, 7), 7);
    }

    #[test]
    fn a_dial_dims_the_zeros_in_front_and_carries_the_point() {
        assert_eq!(digit_text(100_300_000.0, 3), "0");
        assert_eq!(digit_text(100_300_000.0, 4), "3");
        assert!(leading_zero(100_300_000.0, 0));
        assert!(!leading_zero(100_300_000.0, 1));
        assert!(!leading_zero(1_000_000_000.0, 0));
    }
}
