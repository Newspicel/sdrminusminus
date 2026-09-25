use std::rc::Rc;

use kurbo::{Rect, Size};
use zgui::prelude::*;

use super::{
    FlowHandle,
    handle::{grips, handles},
};
use crate::{interaction::Target, model::Id};

const SELECTED_LIFT: i32 = 1000;

pub struct NodeCx<T: Send + Sync + 'static, E: Send + Sync + 'static> {
    pub id: Id,
    pub flow: FlowHandle<T, E>,
}

impl<T: Send + Sync + 'static, E: Send + Sync + 'static> NodeCx<T, E> {
    #[must_use]
    pub fn drag_handle(&self) -> Attrs {
        let flow = self.flow;
        let id = self.id.clone();
        Attrs::new().listener(
            events::POINTER_DOWN,
            ListenerOptions::DEFAULT,
            move |_: &mut EventCx<'_, events::PointerDown>| flow.claim(Target::Node(id.clone())),
        )
    }

    #[must_use]
    pub fn no_pan() -> Attrs {
        Attrs::new()
            .listener(
                events::POINTER_DOWN,
                ListenerOptions::DEFAULT,
                |ev: &mut EventCx<'_, events::PointerDown>| {
                    if ev.button == Some(PointerButton::Primary) {
                        ev.stop_propagation();
                    }
                },
            )
            .listener(
                events::WHEEL,
                ListenerOptions::DEFAULT,
                |ev: &mut EventCx<'_, events::Wheel>| {
                    if !(ev.modifiers.control() || ev.modifiers.meta()) {
                        ev.stop_propagation();
                    }
                },
            )
    }

    #[must_use]
    pub fn selected(&self) -> Signal<bool> {
        let flow = self.flow;
        let id = self.id.clone();
        Signal::derive(move || {
            flow.nodes
                .with(|nodes| nodes.iter().any(|node| node.id == id && node.selected))
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Placed {
    frame: Rect,
    selected: bool,
    z: i32,
    auto_height: bool,
    drag_handle: bool,
    resizable: bool,
    class: Option<String>,
}

pub(super) fn node_view<T, E, V>(
    flow: FlowHandle<T, E>,
    key: String,
    render: Rc<dyn Fn(NodeCx<T, E>) -> V>,
) -> AnyView
where
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
    V: IntoView + 'static,
{
    let Some(id) = flow.nodes.with_untracked(|nodes| {
        nodes
            .iter()
            .find(|node| node.key() == key)
            .map(|node| node.id.clone())
    }) else {
        return AnyView::new(());
    };
    let placed = {
        let id = id.clone();
        Memo::new(move |_| {
            flow.nodes.with(|nodes| {
                nodes.iter().find(|node| node.id == id).map(|node| Placed {
                    frame: node.frame(),
                    selected: node.selected,
                    z: node.z,
                    auto_height: node.auto_height,
                    drag_handle: node.drag_handle,
                    resizable: node.resizable,
                    class: node.class.clone(),
                })
            })
        })
    };
    let frame = move || placed.with(|placed| placed.as_ref().map_or(Rect::ZERO, |p| p.frame));
    let selected = move || placed.with(|placed| placed.as_ref().is_some_and(|p| p.selected));
    let z = move || {
        placed.with(|placed| {
            placed.as_ref().map(|p| {
                let lift = if p.selected { SELECTED_LIFT } else { 0 };
                (p.z + lift).to_string()
            })
        })
    };
    let height = move || {
        placed.with(|placed| {
            placed
                .as_ref()
                .filter(|p| !p.auto_height)
                .map(|p| format!("{:.1}px", p.frame.height()))
        })
    };
    let extra = move || placed.with(|placed| placed.as_ref().and_then(|p| p.class.clone()));

    let body = NodeCx {
        id: id.clone(),
        flow,
    };
    let content = render(body);

    let own = NodeRef::new();
    let border = own.observe_border_box();
    let measuring = {
        let id = id.clone();
        zgui::reactive::RenderEffect::new(move |_| {
            let Some(bounds) = border.get() else {
                return;
            };
            let auto =
                placed.with_untracked(|placed| placed.as_ref().is_some_and(|p| p.auto_height));
            if !auto {
                return;
            }
            let zoom = flow.state.with_untracked(|state| state.viewport.zoom);
            let scale = f64::from(own.scale().max(0.01)) * zoom;
            let measured = f64::from(bounds.size.height.0) / scale;
            let current = frame();
            if (current.height() - measured).abs() > 0.5 {
                flow.nodes.update(|nodes| {
                    if let Some(node) = nodes.iter_mut().find(|node| node.id == id) {
                        node.size = Size::new(node.size.width, measured);
                    }
                });
            }
        })
    };
    on_cleanup_local(move || drop(measuring));

    let press = {
        let id = id.clone();
        move |_: &mut EventCx<'_, events::PointerDown>| {
            let handle_only =
                placed.with_untracked(|placed| placed.as_ref().is_some_and(|p| p.drag_handle));
            flow.claim(if handle_only {
                Target::NodeBody(id.clone())
            } else {
                Target::Node(id.clone())
            });
        }
    };
    let resizable =
        move || placed.with(|placed| placed.as_ref().is_some_and(|p| p.resizable && p.selected));

    AnyView::new(view! {
        box(
            node_ref = own,
            class = "flow__node",
            class:selected = selected,
            attr:data-class = extra,
            style:left = move || Some(format!("{:.2}px", frame().x0)),
            style:top = move || Some(format!("{:.2}px", frame().y0)),
            style:width = move || Some(format!("{:.2}px", frame().width())),
            style:height = height,
            style:z-index = z,
            on:pointer_down = press
        ) {
            {content}
            {handles(flow, id.clone())}
            {move || resizable().then(|| grips(flow, id.clone()))}
        }
    })
}
