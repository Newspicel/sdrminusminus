use zgui::prelude::*;

use crate::{
    shell::theme_choice::ThemeChoice,
    store::{Pane, Store},
    ui::{
        kit_shell::{glyph, icon, icon_button},
        shell::{Dialog, Menu, Shell},
    },
};

const SHEET: &str = css!(
    r#"
.hb { flex: 0 0 auto; align-items: center; gap: 4px; height: 38px; padding: 0 10px; border-bottom: 1px solid var(--line); background-color: var(--panel); }
.hb__mark { font-family: var(--mono); font-size: 14px; font-weight: 600; color: var(--accent); padding-right: 6px; }
.hb__rule { width: 1px; height: 16px; margin: 0 4px; background-color: var(--line); flex: 0 0 auto; }
.hb__menu { display: flex; align-items: center; height: 26px; padding: 0 9px; border-radius: 5px; font-family: var(--mono); font-size: 12px; color: var(--ink); max-width: 240px; overflow: hidden; white-space: nowrap; }
.hb__menu:hover, .hb__menu.on { background-color: var(--panel-2); }
.hb__seg { align-items: center; gap: 2px; padding: 2px; border-radius: 6px; background-color: var(--panel-2); }
.hb__tab { display: flex; align-items: center; gap: 5px; height: 22px; padding: 0 10px; border-radius: 4px; color: var(--ink-dim); font-size: 12px; }
.hb__tab:hover { color: var(--ink); }
.hb__tab.on { background-color: var(--panel-3); color: var(--ink); }
.hb__count { font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.hb__add { display: flex; align-items: center; gap: 4px; height: 26px; padding: 0 10px 0 7px; border-radius: 5px; background-color: var(--accent); color: var(--bg); font-size: 12px; }
.hb__add:hover { background-color: var(--accent-dim); }
.hb__quiet { display: flex; align-items: center; height: 26px; padding: 0 10px; border-radius: 5px; color: var(--ink-dim); font-size: 12px; }
.hb__quiet:hover, .hb__quiet.on { background-color: var(--panel-2); color: var(--ink); }
"#
);

pub fn head(store: Store, shell: Shell) -> impl IntoView {
    install_stylesheet("shell-bar", SHEET);
    let active_name = move || {
        let id = store.workspace.get();
        store
            .workspaces
            .get()
            .iter()
            .find(|found| Some(found.id) == id)
            .map_or_else(|| store.name.get(), |found| found.name.clone())
    };
    view! {
        row(class = "hb") {
            text(class = "hb__mark") {"SDR--"}
            control(
                class = "hb__menu",
                class:on = move || shell.menu.get() == Some(Menu::Workspaces),
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                a11y:label = "Workspaces",
                on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                on:click:stop = move |_| shell.toggle_menu(Menu::Workspaces)
            ) {
                {active_name}
            }
            box(class = "hb__rule") {}
            {views(store)}
            box(class = "hb__rule")
            control(
                class = "hb__add",
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                a11y:label = "Add a node",
                on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                on:click:stop = move |_| {
                    shell.menu.set(None);
                    store.palette.update(|open| *open = !*open);
                }
            ) {
                {icon(glyph::PLUS)}
                "Add"
            }
            spacer() {}
            {history(store)}
            box(class = "hb__rule")
            control(
                class = "hb__quiet",
                class:on = move || shell.menu.get() == Some(Menu::Library),
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                on:click:stop = move |_| shell.toggle_menu(Menu::Library)
            ) {
                "Library"
            }
            box(class = "hb__rule") {}
            {theme_button(shell)}
            {icon_button(glyph::HELP, "Keyboard shortcuts, licenses and reports", move || shell.open(Dialog::Shortcuts))}
        }
    }
}

fn views(store: Store) -> impl IntoView {
    let tab = move |pane: Pane, label: &'static str| {
        let count = move || {
            let pinned = store.rack.get().slots.len();
            (pane == Pane::Rack && pinned > 0).then(|| pinned.to_string())
        };
        view! {
            control(
                class = "hb__tab",
                class:on = move || store.pane.get() == pane,
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                on:click:stop = move |_| store.pane.set(pane)
            ) {
                {label}
                text(class = "hb__count") {{count}}
            }
        }
    };
    view! {
        row(class = "hb__seg") {
            {tab(Pane::Patch, "Patch")}
            {tab(Pane::Rack, "Rack")}
        }
    }
}

fn history(store: Store) -> impl IntoView {
    view! {
        control(
            class = "sk-iconbtn",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = "Undo",
            state:disabled = move || !store.can_undo.get(),
            on:click:stop = move |_| store.step_history(true)
        ) {
            {icon(glyph::UNDO)}
        }
        control(
            class = "sk-iconbtn",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = "Redo",
            state:disabled = move || !store.can_redo.get(),
            on:click:stop = move |_| store.step_history(false)
        ) {
            {icon(glyph::REDO)}
        }
    }
}

fn theme_glyph(choice: ThemeChoice) -> &'static str {
    match choice {
        ThemeChoice::System => glyph::MONITOR,
        ThemeChoice::Dark => glyph::MOON,
        ThemeChoice::Light => glyph::SUN,
    }
}

fn theme_button(shell: Shell) -> impl IntoView {
    view! {
        control(
            class = "sk-iconbtn",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = "Theme",
            on:click:stop = move |_| shell.set_theme(shell.theme.get_untracked().next())
        ) {
            {move || icon(theme_glyph(shell.theme.get()))}
        }
    }
}
