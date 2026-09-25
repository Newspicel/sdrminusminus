use sdrmm_wire::patch::NodeBody;
use zgui::prelude::*;
use zgui_flow::{interaction::MenuTarget, kurbo::Point};

use super::{Canvas, graph, remove_selection};
use crate::store::Store;

const SHEET: &str = css!(
    r#"
.canvas-menu {
    position: absolute;
    min-width: 200px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 8px;
    background-color: var(--panel);
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.5);
    pointer-events: auto;
}

.canvas-menu__item {
    padding: 5px 10px;
    border-radius: 5px;
    color: var(--ink-dim);
    font-size: 12px;
}

.canvas-menu__item:hover { background-color: var(--panel-2); color: var(--ink); }
.canvas-menu__item.danger:hover { color: var(--danger); }

.canvas-menu__hint {
    padding: 4px 10px 2px 10px;
    font-size: 10px;
    color: var(--ink-faint);
}
"#
);

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub target: MenuTarget,
    pub window: Point,
    pub flow: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    AddHere,
    DeleteWire,
    DeleteNode,
    ResetSize,
    Fit,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::AddHere => "Add node here",
            Self::DeleteWire => "Delete wire",
            Self::DeleteNode => "Delete node",
            Self::ResetSize => "Reset size",
            Self::Fit => "Fit the patch on screen",
        }
    }

    const fn danger(self) -> bool {
        matches!(self, Self::DeleteWire | Self::DeleteNode)
    }
}

fn actions(store: Store, target: &MenuTarget) -> Vec<Action> {
    let mut listed = Vec::new();
    match target {
        MenuTarget::Pane => listed.push(Action::AddHere),
        MenuTarget::Edge(_) => listed.push(Action::DeleteWire),
        MenuTarget::Node(id) => {
            let graph = store.graph.get_untracked();
            if let Some(node) = graph.node(id) {
                if node.size.is_some() && graph::is_resizable(node.body.kind()) {
                    listed.push(Action::ResetSize);
                }
                listed.push(Action::DeleteNode);
            }
        }
    }
    listed.push(Action::Fit);
    listed
}

fn run(store: Store, canvas: Canvas, menu: &Menu, action: Action) {
    match (action, &menu.target) {
        (Action::AddHere, _) => store.open_palette_at(menu.flow.x as f32, menu.flow.y as f32),
        (Action::DeleteWire, MenuTarget::Edge(id)) => {
            let key = id.to_string();
            store.edit_graph(move |graph| *graph = graph::remove_edges(graph, &[key]));
        }
        (Action::DeleteNode, MenuTarget::Node(id)) => {
            remove_selection(store, vec![id.to_string()], Vec::new());
        }
        (Action::ResetSize, MenuTarget::Node(id)) => {
            let id = id.to_string();
            store.edit_graph(move |graph| {
                if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                    node.size = None;
                }
            });
        }
        (Action::Fit, _) => canvas.fit_view(true),
        _ => {}
    }
}

pub fn view(store: Store, canvas: Canvas, open: RwSignal<Option<Menu>>) -> impl IntoView {
    install_stylesheet("canvas-menu", SHEET);
    move || {
        open.get().map(|menu| {
            let shows_hint = matches!(menu.target, MenuTarget::Node(_) | MenuTarget::Edge(_));
            let is_channel = match &menu.target {
                MenuTarget::Node(id) => store
                    .graph
                    .get_untracked()
                    .node(id)
                    .is_some_and(|node| matches!(node.body, NodeBody::Channel(_))),
                _ => false,
            };
            let listed = menu.clone();
            let items = move || -> Vec<AnyView> {
                actions(store, &listed.target)
                .into_iter()
                .map(|action| {
                    let chosen = listed.clone();
                    AnyView::new(view! {
                        control(
                            class = "canvas-menu__item",
                            class:danger = action.danger(),
                            on:click = move |_| {
                                open.set(None);
                                run(store, canvas, &chosen, action);
                            }
                        ) {{action.label()}}
                    })
                })
                .collect()
            };
            let hint = if is_channel {
                "m and M cycle the analog modes. Backspace deletes."
            } else {
                "Backspace deletes the selection."
            };
            AnyView::new(view! {
                Portal(layer = OverlayLayer::Popover) {
                    box(
                        class = "canvas-dismiss",
                        style = Some(String::from("position: absolute; inset: 0; pointer-events: auto;")),
                        on:pointer_down = move |_: &mut EventCx<'_, events::PointerDown>| open.set(None)
                    )
                    column(
                        class = "canvas-menu",
                        style:left = Some(format!("{:.0}px", menu.window.x)),
                        style:top = Some(format!("{:.0}px", menu.window.y)),
                        on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()
                    ) {
                        {items()}
                        {shows_hint.then(|| AnyView::new(view! { text(class = "canvas-menu__hint") {{hint}} }))}
                    }
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_short_label_without_dashes() {
        for action in [
            Action::AddHere,
            Action::DeleteWire,
            Action::DeleteNode,
            Action::ResetSize,
            Action::Fit,
        ] {
            let label = action.label();
            assert!(!label.is_empty() && label.len() < 32);
            assert!(!label.contains('\u{2014}'));
        }
    }

    #[test]
    fn only_deletions_are_dangerous() {
        assert!(Action::DeleteWire.danger());
        assert!(Action::DeleteNode.danger());
        assert!(!Action::Fit.danger());
    }
}
