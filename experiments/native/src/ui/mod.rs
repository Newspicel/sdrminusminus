pub mod faces;
pub mod gpu;
pub mod node;
pub mod palette;
pub mod params;
pub mod patch;
pub mod rack;
pub mod scope;
pub mod widgets;

use zgui::prelude::*;

use crate::{
    store::{Pane, Store},
    ui::widgets::{close_menus, provide_menus},
};

pub fn app(store: Store) -> impl IntoView {
    provide_menus();
    view! {
        column(
            class = "shell",
            tabindex = Focus::Sequential,
            on:pointer_down = move |_: &mut EventCx<'_, events::PointerDown>| close_menus(),
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                    close_menus();
                    store.selected.set(None);
                    store.palette.set(false);
                }
            }
        ) {
            {head(store)}
            box(class = "pane") {
                if move || store.pane.get() == Pane::Patch {
                    {patch::pane(store)}
                } else {
                    {rack::pane(store)}
                }
                if move || store.palette.get() {
                    {palette::sheet(store)}
                }
                if move || store.notice.get().is_some() {
                    {notice(store)}
                }
            }
        }
    }
}

fn head(store: Store) -> impl IntoView {
    let tab = move |pane: Pane, label: &'static str| {
        view! {
            control(
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,                class = "tab",
                class:on = move || store.pane.get() == pane,
                on:click = move |_| store.pane.set(pane)
            ) {
                {label}
            }
        }
    };
    view! {
        row(class = "head") {
            text(class = "head__mark") {"SDR--"}
            text(class = "head__name") {{move || store.name.get()}}
            box(class = "head__rule") {}
            {tab(Pane::Patch, "Patch")}
            {tab(Pane::Rack, "Rack")}
            box(class = "head__rule")
            control(
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,                class = "head__link",
                on:click = move |_| store.palette.update(|open| *open = !*open)
            ) {
                "+ Node"
            }
            spacer()
            control(
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,                class = "head__step",
                state:disabled = move || !store.can_undo.get(),
                a11y:label = "Undo",
                on:click = move |_| store.step_history(true)
            ) {
                "\u{21ba}"
            }
            control(
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,                class = "head__step",
                state:disabled = move || !store.can_redo.get(),
                a11y:label = "Redo",
                on:click = move |_| store.step_history(false)
            ) {
                "\u{21bb}"
            }
            box(class = "head__rule") {}
            control(
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,                class = "head__link",
                on:click = move |_| store.palette.set(true)
            ) {
                "Library"
            }
        }
    }
}

fn notice(store: Store) -> impl IntoView {
    view! {
        row(class = "toast", on:click = move |_| store.notice.set(None)) {
            text {{move || store.notice.get().unwrap_or_default()}}
        }
    }
}
