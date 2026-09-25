use kurbo::{Affine, BezPath, Point, Stroke};
use zgui::{
    canvas::{Brush, ShapeBuilder, zgui_color::Color},
    prelude::*,
};

use super::{FlowHandle, FlowStyle};
use crate::{
    interaction::{Pending, Scene},
    model::{Id, Rgba},
    path::{EdgePath, Endpoints},
    viewport::Viewport,
};

pub(super) fn colour(rgba: Rgba) -> Color {
    Color::srgb(rgba.r, rgba.g, rgba.b, rgba.a)
}

fn screen(path: &BezPath, viewport: Viewport) -> BezPath {
    Affine::new([
        viewport.zoom,
        0.0,
        0.0,
        viewport.zoom,
        viewport.x,
        viewport.y,
    ]) * path.clone()
}

struct Drawn {
    path: BezPath,
    colour: Rgba,
    width: f64,
    dashed: bool,
}

fn routed<T, E>(
    scene: &Scene<'_, T, E>,
    style: FlowStyle,
    hovered: Option<&Id>,
    fallback: crate::path::EdgeShape,
) -> Vec<Drawn> {
    scene
        .edges
        .iter()
        .filter_map(|edge| {
            let ends = scene.endpoints(edge)?;
            let EdgePath { path, .. } = edge.shape.unwrap_or(fallback).route(ends);
            let base = edge.color.unwrap_or(style.edge);
            let lit = edge.selected || hovered == Some(&edge.id);
            let colour = if edge.selected {
                style.edge_selected
            } else if lit {
                base.with_alpha(1.0)
            } else {
                base
            };
            let width = edge.width.unwrap_or(style.edge_width) + if lit { 1.0 } else { 0.0 };
            Some(Drawn {
                path,
                colour,
                width,
                dashed: edge.animated,
            })
        })
        .collect()
}

fn connection_line<T, E>(
    scene: &Scene<'_, T, E>,
    pending: &Pending,
    shape: crate::path::EdgeShape,
) -> Option<EdgePath> {
    let from = scene
        .nodes
        .iter()
        .find(|node| node.id == pending.from.node)?;
    let from_side = from.handle(&pending.from.handle, pending.from.kind)?.side;
    let (target, target_side) = pending
        .hover
        .as_ref()
        .filter(|_| pending.valid)
        .and_then(|hover| {
            let node = scene.nodes.iter().find(|node| node.id == hover.node)?;
            let handle = node.handle(&hover.handle, hover.kind)?;
            Some((handle.anchor(node.frame()), handle.side))
        })
        .unwrap_or((pending.pointer, from_side.opposite()));
    let ends = if pending.from.kind == crate::model::HandleKind::Source {
        Endpoints {
            source: pending.anchor,
            source_side: from_side,
            target,
            target_side,
        }
    } else {
        Endpoints {
            source: target,
            source_side: target_side,
            target: pending.anchor,
            target_side: from_side,
        }
    };
    Some(shape.route(ends))
}

pub(super) fn edge_layer<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    style: FlowStyle,
) -> impl IntoView {
    zgui::elements::canvas()
        .class("flow__edges")
        .draw(move |cx| {
            let (viewport, pending, shape) = flow.state.with(|state| {
                (
                    state.viewport,
                    state.pending.clone(),
                    state.options.edge_shape,
                )
            });
            let hovered = flow.hovered_edge.get();
            let dash_speed = flow.state.with_untracked(|state| state.options.dash_speed);
            let offset = -flow.clock.get() * dash_speed * viewport.zoom;
            flow.nodes.with(|nodes| {
                flow.edges.with(|edges| {
                    let always = |_: &crate::model::Connection| true;
                    let scene = Scene {
                        nodes,
                        edges,
                        valid: &always,
                    };
                    for drawn in routed(&scene, style, hovered.as_ref(), shape) {
                        let path = screen(&drawn.path, viewport);
                        let width = (drawn.width * viewport.zoom).max(1.0);
                        let brush = Brush::Solid(colour(drawn.colour));
                        let shape = if drawn.dashed {
                            ShapeBuilder::new(path).stroke_styled(
                                brush,
                                Stroke::new(width).with_dashes(
                                    offset,
                                    [5.0 * viewport.zoom, 5.0 * viewport.zoom],
                                ),
                            )
                        } else {
                            ShapeBuilder::new(path).stroke(brush, width)
                        };
                        cx.scene.push(shape.build());
                    }
                    if let Some(pending) = pending.as_ref()
                        && let Some(line) = connection_line(&scene, pending, shape)
                    {
                        let tint = match (&pending.hover, pending.valid) {
                            (Some(_), true) => style.connection_valid,
                            (Some(_), false) => style.connection_invalid,
                            (None, _) => style.connection,
                        };
                        let width = (style.edge_width * viewport.zoom).max(1.0);
                        cx.scene.push(
                            ShapeBuilder::new(screen(&line.path, viewport))
                                .stroke_styled(
                                    Brush::Solid(colour(tint)),
                                    Stroke::new(width).with_dashes(0.0, [6.0, 4.0]),
                                )
                                .build(),
                        );
                    }
                });
            });
        })
        .into_view()
}

pub(super) fn edge_labels<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
) -> impl IntoView {
    view! {
        for id in move || labelled(flow), key = |id: &String| id.clone() {
            {label_view(flow, id.as_str().into())}
        }
    }
}

fn label_view<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    id: Id,
) -> AnyView {
    let placed = Memo::new(move |_| {
        let shape = flow.state.with(|state| state.options.edge_shape);
        flow.nodes.with(|nodes| {
            flow.edges.with(|edges| {
                let always = |_: &crate::model::Connection| true;
                let scene = Scene {
                    nodes,
                    edges,
                    valid: &always,
                };
                let edge = edges.iter().find(|edge| edge.id == id)?;
                let ends = scene.endpoints(edge)?;
                let at: Point = edge.shape.unwrap_or(shape).route(ends).label;
                Some((at, edge.label.clone().unwrap_or_default(), edge.selected))
            })
        })
    });
    let at = move || placed.with(|placed| placed.as_ref().map_or(Point::ZERO, |(at, _, _)| *at));
    let text = move || {
        placed.with(|placed| {
            placed
                .as_ref()
                .map(|(_, text, _)| text.clone())
                .unwrap_or_default()
        })
    };
    let selected =
        move || placed.with(|placed| placed.as_ref().is_some_and(|(_, _, selected)| *selected));
    AnyView::new(view! {
        box(
            class = "flow__edge-label",
            class:selected = selected,
            style:left = move || Some(format!("{:.2}px", at().x)),
            style:top = move || Some(format!("{:.2}px", at().y))
        ) {
            {text}
        }
    })
}

fn labelled<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
) -> Vec<String> {
    flow.edges.with(|edges| {
        edges
            .iter()
            .filter(|edge| edge.label.is_some())
            .map(|edge| edge.id.to_string())
            .collect()
    })
}
