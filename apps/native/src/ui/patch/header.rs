use zgui::prelude::*;

use crate::{
    shell::rack_grid,
    store::Store,
    ui::{kit_shell::svg, node},
};

const MAXIMIZE: &str =
    r#"<path d="M15 3h6v6"/><path d="m21 3-7 7"/><path d="m3 21 7-7"/><path d="M9 21H3v-6"/>"#;
const MINIMIZE: &str =
    r#"<path d="m14 10 7-7"/><path d="M20 10h-6V4"/><path d="m3 21 7-7"/><path d="M4 14h6v6"/>"#;
const PIN: &str = r#"<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>"#;
const CROSS: &str = r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#;

const SHEET: &str = css!(
    r#"
.node__dot { width: 6px; height: 6px; border-radius: 50%; flex: 0 0 auto; background-color: var(--ink-faint); }
.node[data-category="source"] .node__dot { background-color: oklch(0.72 0.11 228); }
.node[data-category="channel"] .node__dot { background-color: oklch(0.74 0.12 158); }
.node[data-category="tool"] .node__dot { background-color: oklch(0.78 0.11 85); }
.node[data-category="output"] .node__dot { background-color: oklch(0.76 0.1 300); }

.node__buttons { align-items: center; gap: 2px; flex: 0 0 auto; cursor: default; }

.node__btn {
    width: 18px;
    height: 18px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--ink-faint);
}

.node__btn:hover { background-color: var(--panel-3); color: var(--ink); }
.node__btn.on { background-color: color-mix(in oklab, var(--accent) 15%, transparent); color: var(--accent); }
.node__btn.danger:hover { color: var(--danger); }
.node__btn-icon { width: 12px; height: 12px; pointer-events: none; }
"#
);

fn icon(body: &'static str) -> impl IntoView {
    zgui::elements::vector()
        .class("node__btn-icon")
        .document(&svg(body))
}

fn button(
    body: &'static str,
    label: Signal<String>,
    on: Signal<bool>,
    danger: bool,
    press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = "node__btn",
            class:on = on,
            class:danger = danger,
            a11y:role = Role::Button,
            a11y:label = move || zgui::vocab::SharedString::from(label.get()),
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click:stop = move |_| press()
        ) {
            {icon(body)}
        }
    }
}

pub fn header(store: Store, id: String, drag: Attrs, remove: impl Fn() + 'static) -> impl IntoView {
    install_stylesheet("node-header", SHEET);
    let title = {
        let id = id.clone();
        move || {
            store
                .graph
                .get()
                .node(&id)
                .map(node::title_of)
                .unwrap_or_default()
        }
    };
    let full = {
        let id = id.clone();
        Signal::derive(move || store.expanded.get().as_deref() == Some(id.as_str()))
    };
    let pinned = {
        let id = id.clone();
        Signal::derive(move || rack_grid::is_pinned(&store.rack.get(), &id))
    };
    let expand = {
        let id = id.clone();
        move || {
            let next = if full.get_untracked() {
                None
            } else {
                Some(id.clone())
            };
            store.expanded.set(next);
        }
    };
    let pin = {
        let id = id.clone();
        move || store.edit_rack(|rack| rack_grid::toggle_pin(rack, &id))
    };
    let full_label = Signal::derive(move || {
        String::from(if full.get() {
            "Leave full screen"
        } else {
            "Show full screen"
        })
    });
    let pin_label = Signal::derive(move || {
        String::from(if pinned.get() {
            "Unpin from the rack"
        } else {
            "Pin to the rack"
        })
    });
    view! {
        row(class = "node__bar", {..drag}) {
            box(class = "node__dot")
            text(class = "node__title") {{title}}
            spacer()
            row(class = "node__buttons") {
                {move || AnyView::new(button(
                    if full.get() { MINIMIZE } else { MAXIMIZE },
                    full_label,
                    full,
                    false,
                    expand.clone(),
                ))}
                {button(PIN, pin_label, pinned, false, pin)}
                {button(CROSS, Signal::stored(String::from("Remove")), Signal::stored(false), true, remove)}
            }
        }
    }
}
