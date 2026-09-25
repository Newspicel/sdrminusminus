use zgui::prelude::*;

use crate::ui::{dialogs::frame, kit_shell::button, shell::Shell, widgets::check};

pub fn dialog(shell: Shell) -> impl IntoView {
    let never_again = RwSignal::new(false);
    frame(
        "narrow",
        "Leave Auto tuning?",
        (),
        view! {
            text(class = "sk-text") {"On Auto, the radio follows your decoders."}
            text(class = "sk-text") {"On Manual, it stays put. Decoders outside its window stop."}
            row(class = "sk-toolbar") {
                {check(never_again.into(), move |on| never_again.set(on))}
                text(class = "sk-text") {"Don't show again"}
            }
        },
        view! {
            spacer() {}
            {button("Switch to Manual", "quiet", move || shell.confirm_auto_off(never_again.get_untracked()))}
            {button("Stay on Auto", "primary", move || shell.cancel_auto_off())}
        },
    )
}
