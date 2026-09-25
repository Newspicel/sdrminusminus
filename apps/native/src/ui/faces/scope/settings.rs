use zgui::prelude::*;

use super::{
    ScopeCx,
    actions::{apply_range, toggle_phosphor, toggle_trace},
    colormap::{COLORMAPS, Colormap},
    traces::{
        AVERAGE_CHOICES, DB_LIMIT, DEFAULT_AVERAGE, TRACE_MODES, TraceMode, clamp_window,
        with_ceiling, with_floor,
    },
};
use crate::ui::widgets::{check, segments, slide};

const SLIDERS_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 17H5"/><path d="M19 7h-9"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/></svg>"#;

#[must_use]
pub fn changed(average: u32, traces: usize, phosphor: bool, manual: bool) -> bool {
    average != DEFAULT_AVERAGE || traces > 0 || phosphor || manual
}

fn trace_class(mode: TraceMode) -> &'static str {
    match mode {
        TraceMode::Peak => "scope__sample scope__sample--peak",
        TraceMode::Average => "scope__sample scope__sample--average",
        TraceMode::Min => "scope__sample scope__sample--min",
    }
}

pub fn button(cx: ScopeCx) -> impl IntoView {
    let differs = move || {
        changed(
            cx.average.get(),
            cx.modes.with(Vec::len),
            cx.phosphor.get(),
            cx.range.get().is_some(),
        )
    };
    view! {
        control(
            class = "scope__button scope__gear",
            class:on = differs,
            a11y:role = Role::Button,
            a11y:label = "Scope settings",
            on:pointer_down:stop = move |_| {},
            on:click:stop = move |_| cx.settings_open.update(|open| *open = !*open)
        ) {
            {zgui::elements::vector().class("scope__icon").document(SLIDERS_ICON)}
        }
    }
}

fn swatch(cx: ScopeCx, map: Colormap) -> impl IntoView {
    view! {
        control(
            class = "scope__swatch",
            class:on = move || cx.colormap.get() == map,
            a11y:role = Role::Button,
            on:click:stop = move |_| cx.choose_colormap(map)
        ) {
            box(class = "scope__ramp", style:background-image = Some(map.gradient("to right")))
            text(class = "scope__swatch-name") {{map.name()}}
        }
    }
}

fn trace_chip(cx: ScopeCx, mode: TraceMode) -> impl IntoView {
    view! {
        control(
            class = "scope__toggle",
            class:on = move || cx.modes.with(|modes| modes.contains(&mode)),
            a11y:role = Role::Button,
            on:click:stop = move |_| toggle_trace(cx, mode)
        ) {
            box(class = "scope__sample-box") { box(class = trace_class(mode)) }
            text {{mode.label()}}
        }
    }
}

fn phosphor_chip(cx: ScopeCx) -> impl IntoView {
    let ramp = move || Some(cx.colormap.get().gradient("to top"));
    view! {
        control(
            class = "scope__toggle",
            class:on = move || cx.phosphor.get(),
            a11y:role = Role::Button,
            on:click:stop = move |_| toggle_phosphor(cx)
        ) {
            box(class = "scope__sample-box") { box(class = "scope__sample--phosphor", style:background-image = ramp) }
            text {"phosphor"}
        }
    }
}

fn section(name: &'static str, body: impl IntoView + 'static) -> impl IntoView {
    view! {
        column(class = "scope__section") {
            text(class = "scope__section-name") {{name}}
            {body}
        }
    }
}

fn levels(cx: ScopeCx) -> impl IntoView {
    let shown = move || clamp_window(cx.shown_window());
    let floor = Signal::derive(move || shown().min);
    let ceiling = Signal::derive(move || shown().max);
    let auto = Signal::derive(move || cx.range.get().is_none());
    view! {
        column(class = "scope__section") {
            row(class = "scope__section-head") {
                text(class = "scope__section-name") {"Levels"}
                spacer()
                text(class = "scope__faint") {"auto"}
                {check(auto, move |on| {
                    if on {
                        apply_range(cx, None);
                    } else {
                        apply_range(cx, Some(clamp_window(cx.shown_window())));
                    }
                })}
            }
            row(class = "scope__level") {
                text(class = "scope__level-name") {"floor"}
                {slide(floor, DB_LIMIT.min, DB_LIMIT.max, |db| format!("{db:.0} dB"), move |db| {
                    apply_range(cx, Some(with_floor(clamp_window(cx.shown_window()), db)));
                })}
            }
            row(class = "scope__level") {
                text(class = "scope__level-name") {"ceiling"}
                {slide(ceiling, DB_LIMIT.min, DB_LIMIT.max, |db| format!("{db:.0} dB"), move |db| {
                    apply_range(cx, Some(with_ceiling(clamp_window(cx.shown_window()), db)));
                })}
            }
        }
    }
}

pub fn panel(cx: ScopeCx) -> impl IntoView {
    let swatches: Vec<_> = COLORMAPS
        .iter()
        .map(|map| AnyView::new(swatch(cx, *map)))
        .collect();
    let choices: Vec<(u32, &'static str)> = AVERAGE_CHOICES
        .iter()
        .map(|frames| {
            let label = match frames {
                1 => "off",
                2 => "2",
                4 => "4",
                8 => "8",
                _ => "16",
            };
            (*frames, label)
        })
        .collect();
    let chips: Vec<_> = TRACE_MODES
        .iter()
        .map(|mode| AnyView::new(trace_chip(cx, *mode)))
        .collect();
    let ruler = Signal::derive(move || cx.store.settings.with(|settings| settings.band_ruler));
    let average = segments(choices, cx.average.into(), move |frames| {
        cx.choose_average(frames)
    });
    let hold_wheel = move |ev: &mut EventCx<'_, events::Wheel>| ev.stop_propagation();
    let band_toggle = check(ruler, move |on| {
        cx.store.edit_settings(|settings| settings.band_ruler = on);
    });
    view! {
        column(
            class = "scope__panel",
            on:pointer_down:stop = move |_| {},
            on:wheel = hold_wheel
        ) {
            {section("Colours", view! { box(class = "scope__swatches") {{swatches}} })}
            {section("Average", average)}
            {section("Traces", view! { box(class = "scope__toggles") { {chips} {phosphor_chip(cx)} } })}
            row(class = "scope__section scope__section-head") {
                text(class = "scope__section-name") {"Band plan"}
                spacer() {}
                {band_toggle}
            }
            {levels(cx)}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_button_lights_up_only_off_defaults() {
        assert!(!changed(DEFAULT_AVERAGE, 0, false, false));
        assert!(changed(8, 0, false, false));
        assert!(changed(DEFAULT_AVERAGE, 1, false, false));
        assert!(changed(DEFAULT_AVERAGE, 0, true, false));
        assert!(changed(DEFAULT_AVERAGE, 0, false, true));
    }
}
