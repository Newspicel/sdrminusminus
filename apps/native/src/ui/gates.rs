use std::time::Duration;

use sdrmm_wire::rest::AuthInfo;
use zgui::prelude::*;

use crate::{
    shell::{
        server_status::server_down_detail,
        workspace_file::{DEFAULT_NAME, workspace_name},
    },
    store::{Phase, Store},
    ui::{
        kit_shell::{Entry, button, entry, gated_button},
        shell::{Dialog, Shell},
    },
};

const PROBE: Duration = Duration::from_secs(3);

const SHEET: &str = css!(
    r#"
.gate { position: absolute; left: 0; top: 0; right: 0; bottom: 0; z-index: 180; display: flex; align-items: center; justify-content: center; padding: 24px; background-color: var(--bg); }
.gate.dim { background-color: color-mix(in oklab, var(--bg) 90%, transparent); }
.gate__card { flex-direction: column; gap: 12px; width: 400px; max-width: 100%; padding: 16px; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); }
.gate__brand { font-family: var(--mono); font-size: 18px; font-weight: 600; color: var(--accent); }
.gate__title { font-size: 13px; font-weight: 600; color: var(--ink); }
.gate__actions { align-items: center; gap: 10px; }
.start { flex: 1 1 auto; align-items: center; justify-content: center; }
.start__form { align-items: center; gap: 8px; width: 380px; }
.loading { flex: 1 1 auto; align-items: center; justify-content: center; }
"#
);

pub fn install() {
    install_stylesheet("shell-gates", SHEET);
}

pub fn workspace_start(store: Store) -> impl IntoView {
    let typed = RwSignal::new_local(String::new());
    let create = move || {
        store.create(workspace_name(&typed.get_untracked()));
        typed.set(String::new());
    };
    view! {
        row(class = "start") {
            row(class = "start__form") {
                {entry(Entry::new(typed, DEFAULT_NAME).focused(), create, || {})}
                {button("Create a workspace", "primary", create)}
            }
        }
    }
}

pub fn loading() -> impl IntoView {
    view! { row(class = "loading") { text(class = "sk-text faint") {"Loading"} } }
}

pub fn token_gate(store: Store, shell: Shell, refused: bool) -> impl IntoView {
    let typed = RwSignal::new_local(String::new());
    let connect = move || {
        let token = typed.get_untracked().trim().to_owned();
        if token.is_empty() {
            return;
        }
        let kept = token.clone();
        shell.remember(move |prefs| prefs.token = Some(kept));
        store.submit_token(token);
    };
    let enabled = Signal::derive_local(move || !typed.get().trim().is_empty());
    view! {
        box(class = "gate") {
            column(class = "gate__card") {
                text(class = "gate__brand") {"SDR--"}
                text(class = "sk-text") {"This server needs its shared token (--token)."}
                {refused.then(|| view! { text(class = "sk-text bad") {"That token was refused."} })}
                {entry(Entry::new(typed, "Shared token").focused(), connect, || {})}
                {gated_button("Connect", "primary", enabled, connect)}
            }
        }
    }
}

pub fn server_down(store: Store, shell: Shell, reason: String) -> impl IntoView {
    let probing = set_interval(PROBE, move || {
        zgui::task::spawn_local(async move {
            if store.api().get::<AuthInfo>("/api/auth").await.is_ok() {
                store.retry();
            }
        });
    });
    on_cleanup_local(move || drop(probing));
    let detail = server_down_detail(Some(&reason));
    view! {
        box(class = "gate dim") {
            column(class = "gate__card") {
                text(class = "gate__brand") {"SDR--"}
                text(class = "gate__title") {"Can't reach the server"}
                text(class = "sk-text") {"Your work is safe on the server. This window reconnects when it is back."}
                {detail.map(|detail| view! { text(class = "sk-mono") {{detail}} })}
                row(class = "gate__actions") {
                    {button("Try again", "primary", move || store.retry())}
                    {button("Report this", "", move || shell.open(Dialog::Report(None)))}
                    text(class = "sk-text faint") {"Retrying every few seconds"}
                }
            }
        }
    }
}

pub fn gate(store: Store, shell: Shell) -> impl IntoView {
    move || match store.phase.get() {
        Phase::Locked { refused } => Some(AnyView::new(token_gate(store, shell, refused))),
        Phase::Unreachable(reason) => Some(AnyView::new(server_down(store, shell, reason))),
        _ => None,
    }
}
