use zgui::prelude::*;

use crate::decoders::log::{COLUMN_STEP, COLUMNS, Column};

use super::state::Log;

pub fn header(log: Log) -> impl IntoView {
    let cells: Vec<AnyView> = COLUMNS
        .iter()
        .map(|(column, label, _)| AnyView::new(cell(log, *column, label)))
        .collect();
    view! { row(class = "dlog__head") {{cells}} }
}

fn cell(log: Log, column: Column, label: &'static str) -> AnyView {
    if column == Column::Summary {
        return AnyView::new(view! { text(class = "dlog__th flex") {{label}} });
    }
    let width = move || Some(format!("{}px", log.widths.get().of(column)));
    AnyView::new(view! {
        box(class = "dlog__th", style:width = width) {
            text {{label}}
            {grip(log, column, label)}
        }
    })
}

fn grip(log: Log, column: Column, label: &'static str) -> impl IntoView {
    let drag = RwSignal::new(None::<(f32, f32)>);
    let resize = move |px: f32| {
        log.widths
            .update(|widths| *widths = widths.resized(column, px))
    };
    let down = move |ev: &mut EventCx<'_, events::PointerDown>| {
        if ev.button != Some(PointerButton::Primary) {
            return;
        }
        ev.stop_propagation();
        ev.prevent_default();
        ev.capture_pointer();
        drag.set(Some((
            ev.position.x.0,
            log.widths.get_untracked().of(column),
        )));
    };
    let moved = move |ev: &mut EventCx<'_, events::PointerMove>| {
        if let Some((x, width)) = drag.get_untracked() {
            resize(width + ev.position.x.0 - x);
        }
    };
    let up = move |ev: &mut EventCx<'_, events::PointerUp>| {
        if drag.get_untracked().is_some() {
            drag.set(None);
            ev.release_pointer();
            log.save_widths();
        }
    };
    let keys = move |ev: &mut EventCx<'_, events::KeyDown>| {
        let step = match ev.key {
            Key::Named(NamedKey::ArrowLeft) => -COLUMN_STEP,
            Key::Named(NamedKey::ArrowRight) => COLUMN_STEP,
            _ => return,
        };
        ev.prevent_default();
        resize(log.widths.get_untracked().of(column) + step);
        log.save_widths();
    };
    view! {
        box(
            class = "dlog__grip",
            class:on = move || drag.get().is_some(),
            tabindex = Focus::Sequential,
            a11y:role = Role::Splitter,
            a11y:label = format!("Resize {label} column"),
            on:pointer_down = down,
            on:pointer_move = moved,
            on:pointer_up = up,
            on:pointer_cancel = move |_| drag.set(None),
            on:key_down = keys,
        ) {}
    }
}
