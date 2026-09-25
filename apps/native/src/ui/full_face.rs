use zgui::prelude::*;

use crate::{
    store::Store,
    ui::{
        faces,
        kit_shell::{glyph, icon_button},
        node,
    },
};

const SHEET: &str = css!(
    r#"
.fullface { position: absolute; left: 0; top: 0; right: 0; bottom: 0; z-index: 30; padding: 1px; background-color: var(--bg); }
.fullface__card { width: 100%; height: 100%; flex-direction: column; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); overflow: hidden; }
.fullface__face { flex: 1 1 auto; min-height: 0; overflow: auto; }
"#
);

pub fn overlay(store: Store) -> impl IntoView {
    install_stylesheet("shell-fullface", SHEET);
    let dropping = zgui::reactive::RenderEffect::new(move |_| {
        if let Some(id) = store.expanded.get()
            && store.graph.get().node(&id).is_none()
        {
            store.expanded.set(None);
        }
    });
    on_cleanup_local(move || drop(dropping));
    move || {
        let id = store.expanded.get()?;
        let found = store.graph.get_untracked().node(&id).cloned()?;
        let title = node::title_of(&found);
        let category = node::category_class(found.body.category());
        let body = faces::face(store, &found);
        Some(view! {
            box(class = "fullface") {
                column(class = "fullface__card", attr:data-category = category) {
                    row(class = "node__bar") {
                        text(class = "node__title") {{title}}
                        spacer() {}
                        {icon_button(glyph::CROSS, "Close full screen", move || store.expanded.set(None))}
                    }
                    column(class = "fullface__face") {{body}}
                }
            }
        })
    }
}
