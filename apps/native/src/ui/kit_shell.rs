use zgui::prelude::*;
use zgui_ui::prelude::*;

pub mod glyph {
    pub const PLUS: &str = r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#;
    pub const UNDO: &str =
        r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#;
    pub const REDO: &str =
        r#"<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>"#;
    pub const HELP: &str = r#"<circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>"#;
    pub const SUN: &str = r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#;
    pub const MOON: &str = r#"<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>"#;
    pub const MONITOR: &str = r#"<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/>"#;
    pub const PENCIL: &str = r#"<path d="M21.17 6.81a1 1 0 0 0-3.99-3.99L3.84 16.17a2 2 0 0 0-.5.83l-1.32 4.35a.5.5 0 0 0 .62.62l4.35-1.32a2 2 0 0 0 .83-.5z"/><path d="m15 5 4 4"/>"#;
    pub const COPY: &str = r#"<rect x="8" y="8" width="14" height="14" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#;
    pub const DOWNLOAD: &str = r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>"#;
    pub const CROSS: &str = r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#;
    pub const FLAG: &str = r#"<path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z"/><path d="M4 22v-7"/>"#;
    pub const TRASH: &str = r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#;
    pub const FOLDER: &str = r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#;
    pub const INFO: &str =
        r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#;
    pub const EXPAND: &str = r#"<path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/>"#;
}

const SHEET: &str = css!(
    r#"
.sk-icon { display: block; width: 15px; height: 15px; flex: 0 0 auto; }
.sk-icon.sm { width: 12px; height: 12px; }

.sk-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--panel-2);
    color: var(--ink-dim);
    font-size: 12px;
    flex: 0 0 auto;
}
.sk-btn:hover { border-color: var(--line-strong); color: var(--ink); }
.sk-btn:disabled { opacity: 0.45; }
.sk-btn.primary { background-color: var(--accent); border-color: var(--accent); color: var(--bg); }
.sk-btn.primary:hover { background-color: var(--accent-dim); }
.sk-btn.quiet { background-color: transparent; border-color: transparent; }
.sk-btn.quiet:hover { background-color: var(--panel-2); }
.sk-btn.danger { border-color: var(--danger); color: var(--danger); }
.sk-btn.sm { height: 22px; padding: 0 7px; font-size: 11px; }

.sk-iconbtn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 5px;
    color: var(--ink-dim);
    flex: 0 0 auto;
}
.sk-iconbtn:hover { background-color: var(--panel-2); color: var(--ink); }
.sk-iconbtn:disabled { opacity: 0.35; }
.sk-iconbtn.sm { width: 20px; height: 20px; }
.sk-iconbtn.danger:hover { color: var(--danger); }

.sk-field {
    height: 26px;
    min-width: 0;
    width: 100%;
    padding: 3px 8px;
    font-size: 12px;
    line-height: 18px;
    background-color: var(--panel-2);
    color: var(--ink);
    border: 1px solid var(--line);
    border-radius: 5px;
}
.sk-field:focus-visible { border-color: var(--accent); outline: 1px solid var(--accent); }
.sk-entry { flex: 1 1 auto; min-width: 0; }
.sk-entry.narrow { flex: 0 0 72px; }

.sk-scrim {
    position: absolute;
    left: 0;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: color-mix(in oklab, var(--bg) 70%, transparent);
}

.sk-dialog {
    display: flex;
    flex-direction: column;
    max-height: 85%;
    width: 560px;
    max-width: 94%;
    padding: 16px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background-color: var(--panel);
    box-shadow: 0 22px 52px rgba(0, 0, 0, 0.5);
}
.sk-dialog.narrow { width: 380px; }
.sk-dialog.wide { width: 760px; }
.sk-dialog__head { align-items: baseline; gap: 12px; flex: 0 0 auto; }
.sk-dialog__title { font-size: 14px; font-weight: 600; color: var(--ink); }
.sk-dialog__body { flex-direction: column; gap: 10px; margin-top: 12px; min-height: 0; flex: 1 1 auto; overflow: auto; }
.sk-dialog__foot {
    align-items: center;
    gap: 8px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
    flex: 0 0 auto;
}

.sk-text { font-size: 12px; color: var(--ink-dim); }
.sk-text.faint { color: var(--ink-faint); }
.sk-text.bad { color: var(--danger); }
.sk-mono { font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.sk-pre {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
    white-space: pre-wrap;
    padding: 8px;
    border-radius: 4px;
    background-color: var(--panel-2);
    overflow: auto;
}

.sk-list { flex-direction: column; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; flex: 0 0 auto; }
.sk-list__title { padding: 0 2px 3px 2px; }
.sk-row { flex-direction: column; gap: 5px; padding: 6px 8px; background-color: var(--panel); border-top: 1px solid var(--line); }
.sk-row:first-child { border-top-width: 0; }
.sk-row:hover { background-color: var(--panel-2); }
.sk-row__line { align-items: center; gap: 8px; min-width: 0; }
.sk-row__main { flex-direction: column; flex: 1 1 auto; min-width: 0; gap: 1px; }
.sk-row__pick:hover .sk-row__primary { color: var(--accent); }
.sk-row__pick:disabled { opacity: 0.45; }
.sk-row__primary { font-family: var(--mono); font-size: 12px; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sk-row__secondary { font-family: var(--mono); font-size: 10px; color: var(--ink-faint); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sk-row__actions { align-items: center; gap: 4px; flex: 0 0 auto; }
.sk-chip {
    padding: 0 5px;
    border: 1px solid var(--line);
    border-radius: 3px;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-dim);
    flex: 0 0 auto;
}
.sk-chip:hover { border-color: var(--accent-dim); color: var(--accent); }
.sk-toolbar { align-items: center; gap: 8px; flex-wrap: wrap; flex: 0 0 auto; }
.sk-panel { flex-direction: column; gap: 10px; padding: 12px; }
"#
);

pub fn install() {
    install_stylesheet("kit-shell", SHEET);
}

#[must_use]
pub fn svg(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    )
}

pub fn icon(body: &'static str) -> impl IntoView {
    zgui::elements::vector()
        .class("sk-icon")
        .document(&svg(body))
}

pub fn small_icon(body: &'static str) -> impl IntoView {
    zgui::elements::vector()
        .class("sk-icon sm")
        .document(&svg(body))
}

pub fn icon_button(
    body: &'static str,
    label: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    let label = label.into();
    view! {
        control(
            class = "sk-iconbtn",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = label,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click:stop = move |_| on_press()
        ) {
            {icon(body)}
        }
    }
}

pub fn row_action(
    body: &'static str,
    label: impl Into<String>,
    danger: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    let label = label.into();
    view! {
        control(
            class = "sk-iconbtn sm",
            class:danger = danger,
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = label,
            on:click:stop = move |_| on_press()
        ) {
            {small_icon(body)}
        }
    }
}

pub fn button(
    label: impl Into<String>,
    tone: &'static str,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    let label = label.into();
    view! {
        control(
            class = "sk-btn",
            class:primary = tone == "primary",
            class:quiet = tone == "quiet",
            class:danger = tone == "danger",
            class:sm = tone == "sm",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            on:click:stop = move |_| on_press()
        ) {
            {label}
        }
    }
}

pub fn gated_button(
    label: impl Into<String>,
    tone: &'static str,
    enabled: Signal<bool, LocalStorage>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    let label = label.into();
    view! {
        control(
            class = "sk-btn",
            class:primary = tone == "primary",
            class:quiet = tone == "quiet",
            class:sm = tone == "sm",
            state:disabled = move || !enabled.get(),
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            on:click:stop = move |_| if enabled.get_untracked() { on_press() }
        ) {
            {label}
        }
    }
}

pub fn typing(ev: &mut EventCx<'_, events::KeyDown>) {
    if !matches!(ev.key, Key::Named(NamedKey::Escape)) {
        ev.stop_propagation();
    }
}

pub struct Entry {
    pub value: RwSignal<String, LocalStorage>,
    pub placeholder: &'static str,
    pub label: &'static str,
    pub narrow: bool,
    pub focus: bool,
}

impl Entry {
    #[must_use]
    pub fn new(value: RwSignal<String, LocalStorage>, placeholder: &'static str) -> Self {
        Self {
            value,
            placeholder,
            label: placeholder,
            narrow: false,
            focus: false,
        }
    }

    #[must_use]
    pub fn narrow(self) -> Self {
        Self {
            narrow: true,
            ..self
        }
    }

    #[must_use]
    pub fn focused(self) -> Self {
        Self {
            focus: true,
            ..self
        }
    }
}

pub fn entry(
    spec: Entry,
    on_enter: impl Fn() + 'static,
    on_escape: impl Fn() + 'static,
) -> impl IntoView {
    let field = NodeRef::new();
    if spec.focus {
        let focus = zgui::reactive::RenderEffect::new(move |done: Option<bool>| {
            if done == Some(true) {
                return true;
            }
            if field.get().is_some() {
                field.focus();
                return true;
            }
            false
        });
        on_cleanup_local(move || drop(focus));
    }
    let keys = move |ev: &mut EventCx<'_, events::KeyDown>| {
        match ev.key {
            Key::Named(NamedKey::Enter) => {
                on_enter();
                ev.prevent_default();
            }
            Key::Named(NamedKey::Escape) => {
                on_escape();
                ev.stop_propagation();
            }
            _ => {}
        }
        typing(ev);
    };
    let value = spec.value;
    view! {
        box(class = "sk-entry", class:narrow = spec.narrow) {
            Input(
                class = "sk-field",
                value = value,
                label = spec.label,
                placeholder = spec.placeholder,
                node_ref = field,
                on:key_down = keys
            )
        }
    }
}

pub fn scrim(
    on_close: impl Fn() + Clone + 'static,
    body: impl IntoView + 'static,
) -> impl IntoView {
    let close = on_close.clone();
    view! {
        box(
            class = "sk-scrim",
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                if ev.target == ev.current {
                    close();
                }
                ev.stop_propagation();
            },
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                    on_close();
                    ev.stop_propagation();
                }
            }
        ) {
            {body}
        }
    }
}

pub fn list(title: Option<&'static str>, rows: impl IntoView + 'static) -> AnyView {
    let heading =
        title.map(|title| AnyView::new(view! { text(class = "legend sk-list__title") {{title}} }));
    AnyView::new(view! {
        column(class = "sk-panel__list") {
            {heading}
            column(class = "sk-list") {{rows}}
        }
    })
}

pub struct Row {
    pub primary: String,
    pub secondary: Option<String>,
}

pub fn list_row(
    row: Row,
    on_pick: Option<Box<dyn Fn()>>,
    enabled: Signal<bool, LocalStorage>,
    actions: AnyView,
    below: AnyView,
) -> impl IntoView {
    let Row { primary, secondary } = row;
    let main = view! {
        column(class = "sk-row__main") {
            text(class = "sk-row__primary") {{primary}}
            {secondary.map(|secondary| AnyView::new(view! { text(class = "sk-row__secondary") {{secondary}} }))}
        }
    };
    let face = match on_pick {
        Some(pick) => AnyView::new(view! {
            control(
                class = "sk-row__main sk-row__pick",
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                state:disabled = move || !enabled.get(),
                on:click:stop = move |_| if enabled.get_untracked() { pick() }
            ) {
                {main}
            }
        }),
        None => AnyView::new(main),
    };
    view! {
        column(class = "sk-row") {
            row(class = "sk-row__line") {
                {face}
                row(class = "sk-row__actions") {{actions}}
            }
            {below}
        }
    }
}

pub fn hint(text: impl Into<String>) -> AnyView {
    let text = text.into();
    AnyView::new(view! { text(class = "sk-text faint") {{text}} })
}
