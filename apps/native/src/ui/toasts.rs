use std::{sync::Arc, time::Duration};

use zgui::prelude::*;

use crate::{
    shell::toasts::{Toast, Tone},
    store::Store,
    ui::{
        kit_shell::{glyph, small_icon},
        shell::{Dialog, Shell},
    },
};

const TICK: Duration = Duration::from_millis(500);

const SHEET: &str = css!(
    r#"
.toasts { position: absolute; right: 12px; bottom: 12px; z-index: 250; width: 320px; flex-direction: column; gap: 8px; }
.toast2 { align-items: flex-start; gap: 8px; padding: 8px 8px 8px 12px; border: 1px solid var(--line-strong); border-radius: 6px; background-color: var(--panel-3); box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45); }
.toast2.error { border-color: color-mix(in oklab, var(--danger) 60%, transparent); }
.toast2__tag { padding-top: 2px; font-family: var(--mono); font-size: 10px; letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-dim); flex: 0 0 auto; }
.toast2.error .toast2__tag { color: var(--danger); }
.toast2__text { flex: 1 1 auto; min-width: 0; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.toast2__count { color: var(--ink-faint); }
"#
);

fn now_ms() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap_or_default()
}

pub fn stack(store: Store, shell: Shell) -> impl IntoView {
    install_stylesheet("shell-toasts", SHEET);
    let ageing = set_interval(TICK, move || {
        let now = now_ms();
        if store.toasts.with_untracked(|toasts| toasts.expired(now)) {
            let mut next = (*store.toasts.get_untracked()).clone();
            next.expire(now);
            store.toasts.set(Arc::new(next));
        }
    });
    on_cleanup_local(move || drop(ageing));
    view! {
        column(class = "toasts") {
            for toast in move || store.toasts.get().shown.clone(), key = |toast: &Toast| toast.id.clone() {
                {toast_view(store, shell, toast)}
            }
        }
    }
}

fn toast_view(store: Store, shell: Shell, toast: Toast) -> impl IntoView {
    let id = toast.id.clone();
    let repeats = {
        let id = id.clone();
        move || {
            store
                .toasts
                .get()
                .shown
                .iter()
                .find(|shown| shown.id == id)
                .filter(|shown| shown.repeats > 0)
                .map(|shown| format!(" \u{d7}{}", shown.repeats + 1))
        }
    };
    let error = toast.tone == Tone::Error;
    let dismiss = move || {
        let mut next = (*store.toasts.get_untracked()).clone();
        next.dismiss(&id);
        store.toasts.set(Arc::new(next));
    };
    let seed = toast.message.clone();
    view! {
        row(class = "toast2", class:error = error) {
            text(class = "toast2__tag") {{toast.tag()}}
            text(class = "toast2__text") {
                {toast.message.clone()}
                text(class = "toast2__count") {{repeats}}
            }
            {error.then(|| view! {
                control(
                    class = "sk-iconbtn sm",
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    a11y:label = "Report this problem",
                    on:click:stop = move |_| shell.open(Dialog::Report(Some(seed.clone())))
                ) {
                    {small_icon(glyph::FLAG)}
                }
            })}
            control(
                class = "sk-iconbtn sm",
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                a11y:label = "Dismiss",
                on:click:stop = move |_| dismiss()
            ) {
                {small_icon(glyph::CROSS)}
            }
        }
    }
}
