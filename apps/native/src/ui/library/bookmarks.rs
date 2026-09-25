use sdrmm_wire::{
    rest::{Bookmark, CreateBookmarkRequest},
    ws::StateScope,
};
use zgui::prelude::*;

use crate::{
    shell::recordings::format_mhz,
    store::Store,
    ui::{
        kit_shell::{Entry, Row, entry, gated_button, glyph, hint, list, list_row, row_action},
        library::{
            channel_type_of, frequency_of, load, on_scope, reload, suggest_mode, target, tune,
        },
        shell::Shell,
    },
};

const PATH: &str = "/api/bookmarks";

pub fn panel(store: Store, shell: Shell) -> impl IntoView {
    let bookmarks = load::<Vec<Bookmark>>(store, PATH.to_owned());
    let again = move || reload(store, PATH.to_owned(), bookmarks);
    on_scope(store, StateScope::Bookmarks, again);
    let aimed = target(store);
    let label = RwSignal::new_local(String::new());
    let mode = RwSignal::new_local(String::new());
    let save = move || {
        let Some(freq_hz) = aimed
            .get_untracked()
            .and_then(|aimed| frequency_of(store, &aimed))
        else {
            return;
        };
        let typed = label.get_untracked().trim().to_owned();
        if typed.is_empty() {
            return;
        }
        let picked = mode.get_untracked().trim().to_owned();
        zgui::task::spawn_local(async move {
            let body = CreateBookmarkRequest {
                label: typed,
                freq_hz,
                mode: (!picked.is_empty()).then_some(picked),
                group: None,
            };
            match store
                .api()
                .post::<_, serde::de::IgnoredAny>(PATH, &body)
                .await
            {
                Ok(_) => {
                    label.set(String::new());
                    mode.set(String::new());
                }
                Err(error) => store.fail("Cannot save the bookmark", &error),
            }
            again();
        });
    };
    let enabled = Signal::derive_local(move || {
        !label.get().trim().is_empty()
            && aimed
                .get()
                .and_then(|aimed| frequency_of(store, &aimed))
                .is_some()
    });
    let sorted = move || {
        let mut found = bookmarks
            .get()
            .and_then(Result::ok)
            .map(|found| found.to_vec())
            .unwrap_or_default();
        found.sort_by(|a, b| a.freq_hz.total_cmp(&b.freq_hz));
        found
    };
    let ready = Signal::derive_local(move || aimed.get().is_some_and(|aimed| !aimed.locked()));
    let rows = move || {
        sorted()
            .into_iter()
            .map(|bookmark| AnyView::new(bookmark_row(store, shell, bookmark, aimed, ready, again)))
            .collect::<Vec<_>>()
    };
    let hints = move || {
        let aimed = aimed.get();
        let listed = sorted();
        if aimed.is_none() {
            Some(hint("Select a device or decoder first."))
        } else if aimed.is_some_and(|aimed| aimed.locked()) && !listed.is_empty() {
            Some(hint("Tuning is locked here."))
        } else if matches!(bookmarks.get(), Some(Ok(ref found)) if found.is_empty()) {
            Some(hint("No bookmarks yet."))
        } else {
            None
        }
    };
    view! {
        column(class = "sk-panel") {
            row(class = "sk-toolbar") {
                {entry(Entry::new(label, "Label current frequency"), save, || {})}
                {entry(Entry::new(mode, "mode").narrow(), save, || {})}
                {gated_button("Save", "", enabled, save)}
            }
            {hints}
            {move || (!sorted().is_empty()).then(|| list(None, rows))}
        }
    }
}

fn bookmark_row(
    store: Store,
    shell: Shell,
    bookmark: Bookmark,
    aimed: Signal<Option<crate::shell::library_target::TuneTarget>>,
    ready: Signal<bool, LocalStorage>,
    again: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let id = bookmark.id;
    let freq_hz = bookmark.freq_hz;
    let saved_mode = bookmark.mode.clone().filter(|mode| !mode.is_empty());
    let recall = move || {
        let Some(aimed) = aimed.get_untracked() else {
            return;
        };
        tune(store, shell, &aimed, freq_hz);
        suggest_mode(
            store,
            saved_mode.as_deref(),
            channel_type_of(store, &aimed).as_deref(),
            "bookmark",
        );
    };
    let remove = move || {
        zgui::task::spawn_local(async move {
            if let Err(error) = store.api().delete(&format!("{PATH}/{id}")).await {
                store.fail("Cannot delete the bookmark", &error);
            }
            again();
        });
    };
    let primary = match bookmark.mode.as_deref().filter(|mode| !mode.is_empty()) {
        Some(mode) => format!("{}  {mode}", format_mhz(bookmark.freq_hz)),
        None => format_mhz(bookmark.freq_hz),
    };
    let label = format!("Delete {}", bookmark.label);
    list_row(
        Row {
            primary,
            secondary: Some(bookmark.label),
        },
        Some(Box::new(recall)),
        ready,
        AnyView::new(row_action(glyph::TRASH, label, true, remove)),
        AnyView::new(()),
    )
}
