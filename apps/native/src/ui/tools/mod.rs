pub mod antenna;
pub mod cps;
pub mod kit;
pub mod nanovna;
pub mod registry;
mod sheet;

use sdrmm_wire::tools::ToolsResponse;
use zgui::prelude::*;

use self::{
    kit::{Query, button},
    registry::{Launchable, Panel, Size, find, grouped, launchable, size_of},
};
use crate::store::Store;

pub fn dialog(store: Store, open: RwSignal<Option<String>>) -> impl IntoView {
    install_stylesheet("tools", sheet::SHEET);
    let tools: Query<ToolsResponse> = Query::new();
    let fetch = zgui::reactive::RenderEffect::new(move |_| {
        if open.get().is_some()
            && tools.data.get_untracked().is_none()
            && !tools.busy.get_untracked()
        {
            tools.run(async move { store.api().get("/api/tools").await });
        }
    });
    on_cleanup_local(move || drop(fetch));
    let shown = Memo::new(move |_| open.get());
    move || {
        shown
            .get()
            .map(|id| AnyView::new(modal(store, open, id, tools)))
    }
}

fn modal(
    store: Store,
    open: RwSignal<Option<String>>,
    id: String,
    tools: Query<ToolsResponse>,
) -> impl IntoView {
    let listing = id.is_empty();
    let all = Memo::new(move |_| {
        tools
            .data
            .with(|data| launchable(data.as_ref().map_or(&[][..], |data| data.tools.as_slice())))
    });
    let wanted = id.clone();
    let active = Memo::new(move |_| all.with(|all| find(all, Some(wanted.as_str()))));
    let panel =
        Memo::new(move |_| active.with(|active| active.as_ref().and_then(|tool| tool.panel)));
    let full = size_of(Some(id.as_str())) == Size::Full && !listing;
    let title = move || {
        if listing {
            return "Tools".to_owned();
        }
        active.with(|active| {
            active
                .as_ref()
                .map_or_else(|| "Tool".to_owned(), |tool| tool.descriptor.name.clone())
        })
    };
    let gone = move || !listing && tools.data.with(Option::is_some) && active.with(Option::is_none);
    let close = move || open.set(None);
    view! {
        box(
            class = "tools",
            tabindex = Focus::Programmatic,
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                if ev.target == ev.current {
                    close();
                }
            },
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                    close();
                    ev.stop_propagation();
                }
            }
        ) {
            column(class = "tools__box", class:full = full) {
                row(class = "tools__head") {
                    text(class = "tools__title") {{title}}
                    {move || gone().then(|| AnyView::new(view! { text(class = "legend") {"This build no longer offers that tool"} }))}
                }
                box(class = "tools__body") {
                    {move || if listing {
                        AnyView::new(list(open, tools, all))
                    } else {
                        body(store, panel.get(), active)
                    }}
                }
                row(class = "tools__foot") {
                    {(!listing).then(|| AnyView::new(button("btn", || "All tools".to_owned(), || false, move || open.set(Some(String::new())))))}
                    spacer() {}
                    {button("btn", || "Close".to_owned(), || false, close)}
                }
            }
        }
    }
}

fn body(store: Store, panel: Option<Panel>, active: Memo<Option<Launchable>>) -> AnyView {
    match panel {
        Some(Panel::Antenna) => AnyView::new(antenna::panel(store)),
        Some(Panel::Cps) => AnyView::new(cps::panel(store)),
        Some(Panel::NanoVna) => AnyView::new(nanovna::panel(store)),
        None => AnyView::new(move || {
            active.with(|active| {
                active.as_ref().map(|tool| {
                    let name = tool.descriptor.name.clone();
                    AnyView::new(view! {
                        text(class = "tool-dim") {{format!("{name} has no panel here. It is still reachable over the API.")}}
                    })
                })
            })
        }),
    }
}

fn list(
    open: RwSignal<Option<String>>,
    tools: Query<ToolsResponse>,
    all: Memo<Vec<Launchable>>,
) -> impl IntoView {
    move || {
        if tools.error.get().is_some() {
            return AnyView::new(view! { text(class = "tool-dim") {"Could not load the tools."} });
        }
        let groups = all.with(|all| grouped(all));
        if groups.is_empty() {
            let hint = if tools.busy.get() {
                "Loading the tools\u{2026}"
            } else {
                "This build has no tools."
            };
            return AnyView::new(view! { text(class = "tool-dim") {{hint}} });
        }
        let blocks: Vec<AnyView> = groups
            .into_iter()
            .map(|group| {
                let rows: Vec<AnyView> = group
                    .tools
                    .into_iter()
                    .map(|tool| {
                        let id = tool.descriptor.id.clone();
                        let name = tool.descriptor.name.clone();
                        let summary = tool.descriptor.summary.clone();
                        AnyView::new(view! {
                            control(
                                class = "tools__row",
                                a11y:role = Role::Button,
                                tabindex = Focus::Sequential,
                                on:click:stop = move |_| open.set(Some(id.clone()))
                            ) {
                                text(class = "tool-ink") {{name}}
                                text(class = "tool-faint") {{summary}}
                            }
                        })
                    })
                    .collect();
                AnyView::new(view! {
                    column(class = "tool-group") {
                        text(class = "legend") {{group.label}}
                        column(class = "tools__list") {{rows}}
                    }
                })
            })
            .collect();
        AnyView::new(view! { column(class = "tool-stack") {{blocks}} })
    }
}
