use zgui::prelude::*;

use super::FlowHandle;
use crate::{
    interaction::Target,
    model::{HandleKind, HandleRef, Id},
    path::Side,
    resize::Grip,
};

const HANDLE_SIZE: f64 = 16.0;

fn side_class(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
        Side::Top => "top",
        Side::Bottom => "bottom",
    }
}

fn kind_class(kind: HandleKind) -> &'static str {
    match kind {
        HandleKind::Source => "source",
        HandleKind::Target => "target",
    }
}

pub(super) fn handles<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    node: Id,
) -> impl IntoView {
    let owner = node.clone();
    view! {
        for handle in move || listed(flow, &owner), key = |handle: &String| handle.clone() {
            {handle_view(flow, node.clone(), handle)}
        }
    }
}

fn handle_view<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    node: Id,
    key: String,
) -> AnyView {
    let Some((kind, handle)) = key.split_once(':').map(|(kind, id)| {
        let kind = if kind == kind_class(HandleKind::Source) {
            HandleKind::Source
        } else {
            HandleKind::Target
        };
        (kind, Id::from(id))
    }) else {
        return AnyView::new(());
    };
    let spec = {
        let node = node.clone();
        let handle = handle.clone();
        Memo::new(move |_| {
            flow.nodes.with(|nodes| {
                let found = nodes.iter().find(|found| found.id == node)?;
                let spec = found.handle(&handle, kind)?.clone();
                let frame = found.frame();
                let anchor = spec.anchor(frame);
                Some((spec, anchor.x - frame.x0, anchor.y - frame.y0))
            })
        })
    };
    let Some((first, _, _)) = spec.get_untracked() else {
        return AnyView::new(());
    };
    let reference = HandleRef {
        node: node.clone(),
        handle: handle.clone(),
        kind: first.kind,
    };
    let at = move || spec.with(|spec| spec.as_ref().map_or((0.0, 0.0), |(_, x, y)| (*x, *y)));
    let label = move || spec.with(|spec| spec.as_ref().and_then(|(spec, _, _)| spec.label.clone()));
    let class = move || spec.with(|spec| spec.as_ref().and_then(|(spec, _, _)| spec.class.clone()));
    let status = {
        let reference = reference.clone();
        move || {
            flow.state.with(|state| {
                state.pending.as_ref().map(|pending| {
                    if pending.from == reference {
                        "from"
                    } else if pending.hover.as_ref() == Some(&reference) {
                        if pending.valid { "valid" } else { "invalid" }
                    } else {
                        "idle"
                    }
                })
            })
        }
    };
    let press = {
        let reference = reference.clone();
        move |_: &mut EventCx<'_, events::PointerDown>| {
            flow.claim(Target::Handle(reference.clone()));
        }
    };
    let half = HANDLE_SIZE / 2.0;
    let side = side_class(first.side);
    let kind = kind_class(first.kind);
    let connectable = first.connectable;
    AnyView::new(view! {
        box(
            class = "flow__handle",
            class:connectable = connectable,
            attr:data-side = side,
            attr:data-kind = kind,
            attr:data-class = class,
            attr:data-status = move || status().map(str::to_owned),
            style:left = move || Some(format!("{:.2}px", at().0 - half)),
            style:top = move || Some(format!("{:.2}px", at().1 - half)),
            on:pointer_down = press
        ) {
            box(class = "flow__handle-dot")
            {{move || label().map(|text| AnyView::new(view! {
                text(class = "flow__handle-label", attr:data-side = side) {{text}}
            }))}}
        }
    })
}

pub(super) fn grips<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    node: Id,
) -> AnyView {
    let all: Vec<AnyView> = Grip::ALL
        .into_iter()
        .map(|grip| {
            let node = node.clone();
            AnyView::new(view! {
                box(
                    class = "flow__grip",
                    attr:data-grip = grip.class(),
                    on:pointer_down = move |_: &mut EventCx<'_, events::PointerDown>| {
                        flow.claim(Target::Grip(node.clone(), grip));
                    }
                )
            })
        })
        .collect();
    AnyView::new(all)
}

fn listed<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    node: &Id,
) -> Vec<String> {
    flow.nodes.with(|nodes| {
        nodes
            .iter()
            .find(|found| found.id == *node)
            .map(|found| {
                found
                    .handles
                    .iter()
                    .map(|handle| format!("{}:{}", kind_class(handle.kind), handle.id))
                    .collect()
            })
            .unwrap_or_default()
    })
}
