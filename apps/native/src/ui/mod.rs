pub mod bar;
pub mod dialogs;
pub mod faces;
pub mod files;
pub mod full_face;
pub mod gates;
pub mod gpu;
pub mod hotkeys;
pub mod kit_shell;
pub mod library;
pub mod kit_decoders;
pub mod node;
pub mod palette;
pub mod params;
pub mod patch;
pub mod plot;
pub mod rack;
pub mod shell;
pub mod toasts;
pub mod tools;
pub mod widgets;
pub mod workspace_menu;

use std::time::Duration;

use zgui::prelude::*;

use crate::{
    shell::prefs::PrefsFile,
    store::{Pane, Phase, Store},
    ui::{
        shell::{Menu, Shell},
        widgets::{close_menus, provide_menus},
    },
};

const TOKEN_WATCH: Duration = Duration::from_secs(1);

const SHEET: &str = css!(
    r#"
.shell { position: relative; }
"#
);

pub fn app(store: Store, prefs: PrefsFile) -> impl IntoView {
    provide_menus();
    kit_shell::install();
    gates::install();
    install_stylesheet("shell-root", SHEET);
    let shell = Shell::new(prefs);
    provide_context(shell);
    shell::themed(shell);
    watch_token(store, shell);
    let canvas = patch::canvas();
    let root = NodeRef::new();
    listen_everywhere(root);
    let phase = Memo::new(move |_| store.phase.get());
    view! {
        column(
            node_ref = root,
            class = "shell",
            tabindex = Focus::Programmatic,
            on:pointer_down = move |_: &mut EventCx<'_, events::PointerDown>| {
                close_menus();
                shell.menu.set(None);
            },
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| hotkeys::press(store, shell, ev)
        ) {
            {move || (phase.get() == Phase::Ready).then(|| AnyView::new(bar::head(store, shell)))}
            box(class = "pane") {
                {move || match phase.get() {
                    Phase::Ready => AnyView::new(panes(store, canvas)),
                    Phase::NoWorkspace => AnyView::new(gates::workspace_start(store)),
                    _ => AnyView::new(gates::loading()),
                }}
            }
            {move || match shell.menu.get() {
                Some(Menu::Workspaces) => Some(AnyView::new(workspace_menu::menu(store, shell))),
                Some(Menu::Library) => Some(AnyView::new(library::popover(store, shell))),
                None => None,
            }}
            {gates::gate(store, shell)}
            {dialogs::layer(store, shell)}
            {toasts::stack(store, shell)}
        }
    }
}

fn panes(store: Store, canvas: patch::Canvas) -> impl IntoView {
    view! {
        if move || store.pane.get() == Pane::Patch {
            {patch::pane(store, canvas)}
        } else {
            {rack::pane(store)}
        }
        {full_face::overlay(store)}
        if move || store.palette.get() {
            {palette::sheet(store, canvas)}
        }
    }
}

fn listen_everywhere(root: NodeRef) {
    let guard = StoredValue::new_local(None::<WindowShortcut>);
    let binding = zgui::reactive::RenderEffect::new(move |_| {
        if root.get().is_some() && guard.with_value(Option::is_none) {
            guard.set_value(root.window_shortcut());
        }
    });
    on_cleanup_local(move || drop(binding));
}

fn watch_token(store: Store, shell: Shell) {
    let watching = set_interval(TOKEN_WATCH, move || {
        if store.api().token().take_rejected() {
            shell.remember(|prefs| prefs.token = None);
            store.phase.set(Phase::Locked { refused: true });
        }
    });
    on_cleanup_local(move || drop(watching));
}
