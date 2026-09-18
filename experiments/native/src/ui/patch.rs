use std::sync::Arc;

use sdrmm_wire::patch::{
    PatchEdge, PatchGraph, PatchNode, PortBacking, PortDirection, PortRef, PortType,
};
use zgui::{
    canvas::{Brush, ShapeBuilder, zgui_color::Color},
    elements::{kurbo, kurbo::Shape as _},
    prelude::*,
};

use crate::{
    store::Store,
    ui::{
        faces,
        node::{self, Place},
        widgets::close_menus,
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    x: f32,
    y: f32,
    node_x: f32,
    node_y: f32,
    dx: f32,
    dy: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub port_type: PortType,
}

pub fn wires(store: Store, graph: &PatchGraph) -> Vec<Wire> {
    graph
        .edges
        .iter()
        .filter_map(|edge| {
            let from = anchor(store, graph, &edge.from, PortDirection::Out)?;
            let to = anchor(store, graph, &edge.to, PortDirection::In)?;
            Some(Wire {
                from: (from.0, from.1),
                to: (to.0, to.1),
                port_type: from.2,
            })
        })
        .collect()
}

fn anchor(
    store: Store,
    graph: &PatchGraph,
    reference: &PortRef,
    direction: PortDirection,
) -> Option<(f32, f32, PortType)> {
    let node = graph.nodes.iter().find(|node| node.id == reference.node)?;
    let place = places_of(store, node)
        .into_iter()
        .find(|place| place.name == reference.port && place.direction == direction)?;
    Some((
        node.position.x + place.x,
        node.position.y + place.y,
        place.port_type,
    ))
}

pub fn places_of(store: Store, node: &PatchNode) -> Vec<Place> {
    match &node.body {
        sdrmm_wire::patch::NodeBody::Channel(channel) => {
            match store.descriptor_of(&channel.channel_type) {
                Some(descriptor) => node::places(node, Some(PortBacking::Channel(&descriptor))),
                None => node::places(node, None),
            }
        }
        sdrmm_wire::patch::NodeBody::Device(_) => {
            match store
                .device_set_of(&node.id)
                .and_then(|id| store.set_of(id))
            {
                Some(set) => node::places(node, Some(PortBacking::Device(&set.capabilities))),
                None => node::places(node, None),
            }
        }
        _ => node::places(node, None),
    }
}

fn wire_colour(port_type: PortType) -> Color {
    match port_type {
        PortType::Iq => Color::srgb(0.36, 0.68, 0.91, 0.9),
        PortType::Baseband => Color::srgb(0.35, 0.79, 0.85, 0.9),
        PortType::Audio => Color::srgb(0.31, 0.82, 0.63, 0.9),
        PortType::Events => Color::srgb(0.86, 0.71, 0.36, 0.9),
        PortType::Video => Color::srgb(0.93, 0.62, 0.48, 0.9),
        PortType::Control => Color::srgb(0.77, 0.61, 0.91, 0.9),
        PortType::Position => Color::srgb(0.55, 0.83, 0.6, 0.9),
        PortType::Tx => Color::srgb(0.95, 0.59, 0.74, 0.9),
    }
}

const DOT_PITCH: f64 = 24.0;

fn dots(scene: &mut zgui::canvas::CanvasScene, width: f64, height: f64, pan_x: f64, pan_y: f64) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let ink = Color::srgb(0.24, 0.26, 0.31, 1.0);
    let first_x = pan_x.rem_euclid(DOT_PITCH);
    let first_y = pan_y.rem_euclid(DOT_PITCH);
    let mut y = first_y;
    while y < height {
        let mut x = first_x;
        while x < width {
            scene.push(
                ShapeBuilder::new(kurbo::Rect::new(x, y, x + 1.4, y + 1.4).to_path(0.1))
                    .fill(Brush::Solid(ink))
                    .build(),
            );
            x += DOT_PITCH;
        }
        y += DOT_PITCH;
    }
}

pub fn curve(from: (f32, f32), to: (f32, f32)) -> kurbo::BezPath {
    let (x0, y0) = (f64::from(from.0), f64::from(from.1));
    let (x1, y1) = (f64::from(to.0), f64::from(to.1));
    let reach = ((x1 - x0).abs() * 0.5).clamp(40.0, 220.0);
    let mut path = kurbo::BezPath::new();
    path.move_to((x0, y0));
    path.curve_to((x0 + reach, y0), (x1 - reach, y1), (x1, y1));
    path
}

pub fn pane(store: Store) -> impl IntoView {
    let scale = use_window().scale();
    let pan = RwSignal::new((0.0f32, 0.0f32));
    let panning = RwSignal::new(None::<(f32, f32, f32, f32)>);
    let dragging = RwSignal::new(None::<(String, Drag)>);
    let armed = RwSignal::new(None::<(String, String, PortType)>);

    let drawn = Signal::derive(move || Arc::new(wires(store, &store.graph.get())));
    let wire_layer = zgui::elements::canvas()
        .class("patch__wires")
        .draw(move |cx| {
            let (width, height) = (f64::from(cx.size.width.0), f64::from(cx.size.height.0));
            let (pan_x, pan_y) = pan.get();
            dots(cx.scene, width, height, f64::from(pan_x), f64::from(pan_y));
            for wire in drawn.get().iter() {
                let from = (wire.from.0 + pan_x, wire.from.1 + pan_y);
                let to = (wire.to.0 + pan_x, wire.to.1 + pan_y);
                cx.scene.push(
                    ShapeBuilder::new(curve(from, to))
                        .stroke(Brush::Solid(wire_colour(wire.port_type)), 1.5)
                        .build(),
                );
            }
        })
        .into_view();

    let start_pan = move |ev: &mut EventCx<'_, events::PointerDown>| {
        ev.capture_pointer();
        let (x, y) = pan.get_untracked();
        panning.set(Some((ev.position.x.0, ev.position.y.0, x, y)));
        store.selected.set(None);
        armed.set(None);
    };
    let move_pan = move |ev: &mut EventCx<'_, events::PointerMove>| {
        if let Some((start_x, start_y, from_x, from_y)) = panning.get_untracked() {
            pan.set((
                from_x + ev.position.x.0 - start_x,
                from_y + ev.position.y.0 - start_y,
            ));
        } else if let Some((node, drag)) = dragging.get_untracked() {
            dragging.set(Some((
                node,
                Drag {
                    dx: ev.position.x.0 - drag.x,
                    dy: ev.position.y.0 - drag.y,
                    ..drag
                },
            )));
        }
    };
    let end_pan = move |_: &mut EventCx<'_, events::PointerUp>| {
        panning.set(None);
        if let Some((node, drag)) = dragging.get_untracked() {
            dragging.set(None);
            store.move_node(node, drag.node_x + drag.dx, drag.node_y + drag.dy);
            store.commit_layout();
        }
    };

    let _ = scale;

    view! {
        box(
            class = "patch",
            on:pointer_down = start_pan,
            on:pointer_move = move_pan,
            on:pointer_up = end_pan,
            on:pointer_cancel = move |_: &mut EventCx<'_, events::PointerCancel>| {
                panning.set(None);
                dragging.set(None);
            }
        ) {
            {wire_layer}
            for node in move || store.graph.get().nodes.clone(), key = shape_of {
                {card(store, &node, pan, dragging, armed)}
            }
        }
    }
}

#[must_use]
pub fn shape_of(node: &PatchNode) -> String {
    match &node.body {
        sdrmm_wire::patch::NodeBody::Channel(channel) => {
            format!("{}/channel/{}", node.id, channel.channel_type)
        }
        body => format!("{}/{}", node.id, body.kind()),
    }
}

fn card(
    store: Store,
    node: &PatchNode,
    pan: RwSignal<(f32, f32)>,
    dragging: RwSignal<Option<(String, Drag)>>,
    armed: RwSignal<Option<(String, String, PortType)>>,
) -> AnyView {
    let id = node.id.clone();
    let width = node::width_of(node);
    let category = node::category_class(node.body.category());
    let title = node::title_of(node);
    let places = places_of(store, node);
    let body = faces::face(store, node);
    let status = {
        let id = id.clone();
        Signal::derive(move || faces::status_of(store, &id))
    };
    let placed = {
        let id = id.clone();
        move || {
            let (pan_x, pan_y) = pan.get();
            let (mut x, mut y) = store
                .graph
                .get()
                .nodes
                .iter()
                .find(|found| found.id == id)
                .map_or((0.0, 0.0), |found| (found.position.x, found.position.y));
            if let Some((held, drag)) = dragging.get()
                && held == id
            {
                x = drag.node_x + drag.dx;
                y = drag.node_y + drag.dy;
            }
            (x + pan_x, y + pan_y)
        }
    };
    let placed = Signal::derive(placed);
    let left = move || placed.get().0;
    let top = move || placed.get().1;

    let grab = {
        let id = id.clone();
        move |ev: &mut EventCx<'_, events::PointerDown>| {
            ev.stop_propagation();
            ev.capture_pointer();
            close_menus();
            store.selected.set(Some(id.clone()));
            let node = store
                .graph
                .get_untracked()
                .nodes
                .iter()
                .find(|found| found.id == id)
                .map(|found| (found.position.x, found.position.y));
            if let Some((node_x, node_y)) = node {
                dragging.set(Some((
                    id.clone(),
                    Drag {
                        x: ev.position.x.0,
                        y: ev.position.y.0,
                        node_x,
                        node_y,
                        dx: 0.0,
                        dy: 0.0,
                    },
                )));
            }
        }
    };
    let shut = {
        let id = id.clone();
        move |_: &mut EventCx<'_, events::Click>| {
            let id = id.clone();
            store.edit_graph(move |graph| {
                graph.nodes.retain(|found| found.id != id);
                graph
                    .edges
                    .retain(|edge| edge.from.node != id && edge.to.node != id);
            });
        }
    };

    let selected = {
        let id = id.clone();
        move || store.selected.get().as_deref() == Some(id.as_str())
    };

    let dots: Vec<_> = places
        .into_iter()
        .map(|place| port(store, id.clone(), place, width, armed))
        .collect();

    AnyView::new(view! {
        box(
            class = "node",
            class:sel = selected,
            class:wide = width > node::DEFAULT_WIDTH,
            attr:data-category = category,
            style:left = move || Some(format!("{}px", left())),
            style:top = move || Some(format!("{}px", top())),
            style:width = Some(format!("{width}px")),
            on:pointer_down:stop = {
                let id = id.clone();
                move |_: &mut EventCx<'_, events::PointerDown>| {
                    close_menus();
                    store.selected.set(Some(id.clone()));
                }
            },
            on:pointer_up = move |_: &mut EventCx<'_, events::PointerUp>| {
                if let Some((node, drag)) = dragging.get_untracked() {
                    dragging.set(None);
                    store.move_node(node, drag.node_x + drag.dx, drag.node_y + drag.dy);
                    store.commit_layout();
                }
            }
        ) {
            row(class = "node__bar", on:pointer_down = grab) {
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
                control(class = "node__shut", a11y:label = "Remove", on:click:stop = shut) {"x"}
            }
            {body}
            {dots}
        }
    })
}

fn port(
    store: Store,
    node: String,
    place: Place,
    width: f32,
    armed: RwSignal<Option<(String, String, PortType)>>,
) -> AnyView {
    let kind = place.port_type.as_str();
    let name = place.name.clone();
    let direction = place.direction;
    let left = if direction == PortDirection::In {
        -6.5
    } else {
        width - 6.5
    };
    let tag_left = if direction == PortDirection::In {
        -54.0
    } else {
        width + 6.0
    };
    let tag_side = if direction == PortDirection::In {
        "in"
    } else {
        "out"
    };
    let click = {
        let node = node.clone();
        let name = name.clone();
        let port_type = place.port_type;
        move |ev: &mut EventCx<'_, events::PointerDown>| {
            ev.stop_propagation();
            match (armed.get_untracked(), direction) {
                (None, PortDirection::Out) => {
                    armed.set(Some((node.clone(), name.clone(), port_type)));
                }
                (Some((from_node, from_port, from_type)), PortDirection::In) => {
                    armed.set(None);
                    if from_type != port_type || from_node == node {
                        return;
                    }
                    let edge = PatchEdge {
                        from: PortRef {
                            node: from_node,
                            port: from_port,
                        },
                        to: PortRef {
                            node: node.clone(),
                            port: name.clone(),
                        },
                    };
                    store.edit_graph(move |graph| {
                        graph.edges.retain(|held| *held != edge);
                        graph.edges.push(edge);
                    });
                }
                _ => armed.set(None),
            }
        }
    };
    let is_armed = {
        let node = node.clone();
        let name = name.clone();
        move || {
            armed
                .get()
                .is_some_and(|(held_node, held_port, _)| held_node == node && held_port == name)
        }
    };

    AnyView::new(view! {
        box {
            box(
                class = "port",
                class:armed = is_armed,
                attr:data-port = kind,
                style:left = Some(format!("{left}px")),
                style:top = Some(format!("{}px", place.y - 4.0)),
                on:pointer_down = click
            )
            box(
                class = "port__tag",
                attr:data-side = tag_side,
                class:out = direction == PortDirection::Out,
                style:left = Some(format!("{tag_left}px")),
                style:top = Some(format!("{}px", place.y - 5.0)),
                style:width = Some(String::from("46px"))
            ) {
                {place.name.clone()}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, body: sdrmm_wire::patch::NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: sdrmm_wire::patch::Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    #[test]
    fn a_moved_node_keeps_its_key_so_its_view_is_moved_and_not_rebuilt() {
        use sdrmm_wire::patch::{DeviceNode, NodeBody};
        let mut moved = node("device", NodeBody::Device(DeviceNode::default()));
        let before = shape_of(&moved);
        moved.position = sdrmm_wire::patch::Position { x: 400.0, y: 90.0 };
        assert_eq!(shape_of(&moved), before);
    }

    #[test]
    fn a_channel_that_changes_decoder_gets_a_key_of_its_own() {
        use sdrmm_wire::patch::{ChannelNode, NodeBody};
        let nfm = node(
            "ch",
            NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
        );
        let wfm = node(
            "ch",
            NodeBody::Channel(ChannelNode {
                channel_type: "wfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
        );
        assert_ne!(shape_of(&nfm), shape_of(&wfm));
        assert_ne!(shape_of(&nfm), shape_of(&node("ch", NodeBody::Scope)));
    }

    #[test]
    fn a_wire_bulges_towards_the_side_each_end_leaves_from() {
        let path = curve((0.0, 0.0), (400.0, 100.0));
        let elements: Vec<_> = path.elements().to_vec();
        assert_eq!(elements.len(), 2);
        let kurbo::PathEl::CurveTo(first, second, end) = elements[1] else {
            panic!("a wire is one curve");
        };
        assert!(first.x > 0.0 && first.x <= 220.0);
        assert!(second.x < 400.0);
        assert_eq!(end, kurbo::Point::new(400.0, 100.0));
    }

    #[test]
    fn a_short_wire_still_bends_and_a_long_one_stops_bending_further() {
        let short = curve((0.0, 0.0), (10.0, 0.0));
        let kurbo::PathEl::CurveTo(first, _, _) = short.elements()[1] else {
            panic!("a wire is one curve");
        };
        assert_eq!(first.x, 40.0);

        let long = curve((0.0, 0.0), (2000.0, 0.0));
        let kurbo::PathEl::CurveTo(first, _, _) = long.elements()[1] else {
            panic!("a wire is one curve");
        };
        assert_eq!(first.x, 220.0);
    }

    #[test]
    fn every_port_type_is_drawn_in_a_colour_of_its_own() {
        let all = [
            PortType::Iq,
            PortType::Baseband,
            PortType::Audio,
            PortType::Events,
            PortType::Video,
            PortType::Control,
            PortType::Position,
            PortType::Tx,
        ];
        let colours: Vec<String> = all
            .iter()
            .map(|port| format!("{:?}", wire_colour(*port)))
            .collect();
        for (at, colour) in colours.iter().enumerate() {
            assert!(
                !colours[at + 1..].contains(colour),
                "{:?} shares a colour",
                all[at]
            );
        }
    }
}
