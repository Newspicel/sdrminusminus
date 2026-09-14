use zgui::prelude::*;

use crate::{
    store::Store,
    ui::{faces, node},
};

pub fn pane(store: Store) -> impl IntoView {
    let cards = move || {
        let graph = store.graph.get();
        graph
            .nodes
            .iter()
            .map(|found| {
                let title = node::title_of(found);
                let width = node::width_of(found);
                let category = node::category_class(found.body.category());
                let id = found.id.clone();
                let status = Signal::derive(move || faces::status_of(store, &id));
                let body = faces::face(store, found);
                view! {
                    box(
                        class = "node",
                        attr:data-category = category,
                        style:width = Some(format!("{width}px"))
                    ) {
                        row(class = "node__bar") {
                            text(class = "node__title") {{title}}
                            spacer()
                            text(
                                class = "node__state",
                                class:run = move || status.get().0 == "run",
                                class:err = move || status.get().0 == "err",
                                class:idle = move || status.get().0 == "idle"
                            ) {
                                {move || status.get().1}
                            }
                        }
                        {body}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! { row(class = "rack") {{cards}} }
}
