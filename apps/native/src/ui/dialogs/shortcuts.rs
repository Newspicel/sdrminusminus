use zgui::prelude::*;

use crate::{
    shell::hotkeys::BINDINGS,
    ui::{
        dialogs::frame,
        kit_shell::button,
        shell::{Dialog, Shell},
    },
};

const SHEET: &str = css!(
    r#"
.keys { display: grid; grid-template-columns: 150px 1fr; column-gap: 16px; row-gap: 6px; }
.keys__chord { text-align: right; font-family: var(--mono); font-size: 12px; color: var(--accent); }
.keys__what { font-size: 12px; color: var(--ink-dim); }
"#
);

pub fn dialog(shell: Shell) -> impl IntoView {
    install_stylesheet("shell-keys", SHEET);
    let rows: Vec<AnyView> = BINDINGS
        .iter()
        .flat_map(|(keys, what)| {
            [
                AnyView::new(view! { text(class = "keys__chord") {{*keys}} }),
                AnyView::new(view! { text(class = "keys__what") {{*what}} }),
            ]
        })
        .collect();
    frame(
        "",
        "Keyboard",
        (),
        view! { box(class = "keys") {{rows}} },
        view! {
            {button("Licenses", "quiet", move || shell.open(Dialog::About))}
            {button("Report a problem", "quiet", move || shell.open(Dialog::Report(None)))}
            spacer() {}
            {button("Close", "", move || shell.close())}
        },
    )
}
