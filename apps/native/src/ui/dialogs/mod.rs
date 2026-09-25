mod about;
mod auto_off;
mod report;
mod shortcuts;

use zgui::prelude::*;

use crate::{
    store::Store,
    ui::{
        kit_shell::scrim,
        shell::{Dialog, Shell},
    },
};

pub fn layer(store: Store, shell: Shell) -> impl IntoView {
    let dialog = move || {
        let open = shell.dialog.get()?;
        let body = match open {
            Dialog::Shortcuts => AnyView::new(shortcuts::dialog(shell)),
            Dialog::About => AnyView::new(about::dialog(store, shell)),
            Dialog::Report(seed) => AnyView::new(report::dialog(store, shell, seed)),
        };
        Some(AnyView::new(scrim(move || shell.close(), body)))
    };
    let asking = move || {
        shell.asking_auto_off.get().then(|| {
            AnyView::new(scrim(
                move || shell.cancel_auto_off(),
                auto_off::dialog(shell),
            ))
        })
    };
    view! {
        {dialog}
        {asking}
    }
}

pub fn frame(
    class: &'static str,
    title: impl IntoView + 'static,
    aside: impl IntoView + 'static,
    body: impl IntoView + 'static,
    foot: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        column(
            class = format!("sk-dialog {class}"),
            a11y:role = Role::Dialog,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()
        ) {
            row(class = "sk-dialog__head") {
                text(class = "sk-dialog__title") {{title}}
                spacer() {}
                {aside}
            }
            column(class = "sk-dialog__body") {{body}}
            row(class = "sk-dialog__foot") {{foot}}
        }
    }
}
