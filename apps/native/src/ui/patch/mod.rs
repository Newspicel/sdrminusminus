pub mod graph;
mod header;
mod menu;
mod remove;

use sdrmm_wire::patch::{
    NodeBody, PatchEdge, PatchGraph, PatchNode, PortBacking, PortDirection, PortRef, PortType,
    Position,
};
use zgui::prelude::*;
use zgui_flow::{
    interaction::{Effect, Options},
    kurbo::{Point, Size},
    model::{Connection, Edge, Handle, HandleKind, Id, Node, Rgba},
    path::Side,
    view::{FlowHandle, FlowStyle, NodeCx, controls, flow_view, minimap},
};

use crate::{
    store::Store,
    ui::{
        faces,
        node::{self, Place},
        widgets::close_menus,
    },
};

pub type Canvas = FlowHandle<PatchNode, PortType>;

pub fn places_of(store: Store, node: &PatchNode) -> Vec<Place> {
    match &node.body {
        NodeBody::Channel(channel) => match store.descriptor_of(&channel.channel_type) {
            Some(descriptor) => node::places(node, Some(PortBacking::Channel(&descriptor))),
            None => node::places(node, None),
        },
        NodeBody::Device(_) => match store
            .device_set_of(&node.id)
            .and_then(|id| store.set_of(id))
        {
            Some(set) => node::places(node, Some(PortBacking::Device(&set.capabilities))),
            None => node::places(node, None),
        },
        _ => node::places(node, None),
    }
}

#[must_use]
pub fn wire_colour(port_type: PortType) -> Rgba {
    match port_type {
        PortType::Iq => Rgba::new(0.36, 0.68, 0.91, 0.9),
        PortType::Baseband => Rgba::new(0.35, 0.79, 0.85, 0.9),
        PortType::Audio => Rgba::new(0.31, 0.82, 0.63, 0.9),
        PortType::Events => Rgba::new(0.86, 0.71, 0.36, 0.9),
        PortType::Video => Rgba::new(0.93, 0.62, 0.48, 0.9),
        PortType::Control => Rgba::new(0.77, 0.61, 0.91, 0.9),
        PortType::Position => Rgba::new(0.55, 0.83, 0.6, 0.9),
        PortType::Tx => Rgba::new(0.95, 0.59, 0.74, 0.9),
    }
}

#[must_use]
pub fn shape_of(node: &PatchNode) -> String {
    match &node.body {
        NodeBody::Channel(channel) => format!("channel/{}", channel.channel_type),
        body => body.kind().to_owned(),
    }
}

fn handles_of(places: &[Place]) -> Vec<Handle> {
    places
        .iter()
        .map(|place| {
            let (kind, side) = match place.direction {
                PortDirection::In => (HandleKind::Target, Side::Left),
                PortDirection::Out => (HandleKind::Source, Side::Right),
            };
            Handle::new(place.name.as_str(), kind, side, f64::from(place.y))
                .labelled(place.name.clone())
                .class(place.port_type.as_str())
        })
        .collect()
}

fn flow_node(store: Store, node: &PatchNode, selected: bool) -> Node<PatchNode> {
    let places = places_of(store, node);
    let kind = node.body.kind();
    let natural = graph::natural_size(kind);
    let resizable = graph::is_resizable(kind);
    let width = node.size.map_or(natural.w, |size| size.w);
    let height = if resizable {
        node.size.map(|size| size.h).or(natural.h).unwrap_or(0.0)
    } else {
        0.0
    };
    let mut drawn = Node::new(
        node.id.as_str(),
        Point::new(f64::from(node.position.x), f64::from(node.position.y)),
        Size::new(f64::from(width), f64::from(height)),
        node.clone(),
    );
    let inputs = places
        .iter()
        .filter(|place| place.direction == PortDirection::In)
        .count();
    let (min_w, min_h) = graph::min_size(kind, inputs, places.len() - inputs);
    drawn.min_size = Size::new(f64::from(min_w), f64::from(min_h));
    drawn.handles = handles_of(&places);
    drawn.selected = selected;
    drawn.drag_handle = true;
    drawn.resizable = resizable;
    drawn.auto_height = !resizable;
    drawn.variant = Some(shape_of(node).into());
    drawn.class = Some(node::category_class(node.body.category()).to_owned());
    drawn
}

fn flow_edge(store: Store, graph: &PatchGraph, edge: &PatchEdge, selected: bool) -> Edge<PortType> {
    let carried = graph
        .node(&edge.from.node)
        .and_then(|node| {
            places_of(store, node)
                .into_iter()
                .find(|place| place.name == edge.from.port && place.direction == PortDirection::Out)
        })
        .map_or(PortType::Iq, |place| place.port_type);
    let mut drawn = Edge::new(&connection_of(edge), carried);
    drawn.color = Some(wire_colour(carried));
    drawn.selected = selected;
    drawn
}

fn connection_of(edge: &PatchEdge) -> Connection {
    Connection {
        source: edge.from.node.as_str().into(),
        source_handle: edge.from.port.as_str().into(),
        target: edge.to.node.as_str().into(),
        target_handle: edge.to.port.as_str().into(),
    }
}

fn edge_of(connection: &Connection) -> PatchEdge {
    PatchEdge {
        from: PortRef {
            node: connection.source.to_string(),
            port: connection.source_handle.to_string(),
        },
        to: PortRef {
            node: connection.target.to_string(),
            port: connection.target_handle.to_string(),
        },
    }
}

fn sync(store: Store, canvas: Canvas, graph: &PatchGraph) {
    let held: Vec<Id> = canvas.selected();
    let measured: Vec<(Id, Size)> = canvas.nodes.with_untracked(|nodes| {
        nodes
            .iter()
            .filter(|node| node.auto_height)
            .map(|node| (node.id.clone(), node.size))
            .collect()
    });
    let chosen: Vec<Id> = canvas.edges.with_untracked(|edges| {
        edges
            .iter()
            .filter(|edge| edge.selected)
            .map(|edge| edge.id.clone())
            .collect()
    });
    let nodes: Vec<Node<PatchNode>> = graph
        .nodes
        .iter()
        .map(|node| {
            let mut drawn = flow_node(store, node, held.iter().any(|id| **id == *node.id));
            if let Some((_, size)) = measured.iter().find(|(id, _)| **id == *node.id)
                && drawn.auto_height
            {
                drawn.size.height = size.height;
            }
            drawn
        })
        .collect();
    let edges: Vec<Edge<PortType>> = graph
        .edges
        .iter()
        .map(|edge| {
            let key = connection_of(edge).key();
            flow_edge(store, graph, edge, chosen.iter().any(|id| **id == *key))
        })
        .collect();
    canvas.nodes.set(nodes);
    canvas.edges.set(edges);
}

pub fn refusal(store: Store, graph: &PatchGraph, connection: &Connection) -> Option<String> {
    if connection.source == connection.target {
        return Some(String::from("a node cannot wire to itself"));
    }
    let edge = edge_of(connection);
    if graph.edges.contains(&edge) {
        return Some(String::from("already wired"));
    }
    let mut tentative = graph.clone();
    tentative.edges.push(edge);
    let descriptors = store.channel_types.get_untracked();
    tentative
        .validate_against(&descriptors)
        .err()
        .map(|error| error.to_string())
}

fn on_effect(store: Store, canvas: Canvas, menu: RwSignal<Option<menu::Menu>>, effect: &Effect) {
    tracing::debug!(?effect, "canvas");
    match effect {
        Effect::DragStop(ids) | Effect::Delete { nodes: ids, .. } if ids.is_empty() => {}
        Effect::DragStop(ids) => {
            let placed: Vec<(String, Point)> = canvas.nodes.with_untracked(|nodes| {
                nodes
                    .iter()
                    .filter(|node| ids.contains(&node.id))
                    .map(|node| (node.id.to_string(), node.position))
                    .collect()
            });
            for (id, at) in placed {
                store.move_node(id, at.x as f32, at.y as f32);
            }
            store.commit_layout();
        }
        Effect::ResizeStop(id) => {
            let size = canvas.nodes.with_untracked(|nodes| {
                nodes
                    .iter()
                    .find(|node| node.id == *id)
                    .map(|node| (node.position, node.size))
            });
            if let Some((at, size)) = size {
                store.resize_node(
                    id.to_string(),
                    at.x as f32,
                    at.y as f32,
                    size.width as f32,
                    size.height as f32,
                );
            }
        }
        Effect::Connect(connection) => {
            if let Some(reason) = refusal(store, &store.graph.get_untracked(), connection) {
                store.say(reason);
                return;
            }
            let edge = edge_of(connection);
            store.edit_graph(move |graph| *graph = graph::add_edge(graph, edge));
        }
        Effect::ConnectEnd { .. } => {}
        Effect::Delete { nodes, edges } => remove_selection(
            store,
            nodes.iter().map(ToString::to_string).collect(),
            edges.iter().map(ToString::to_string).collect(),
        ),
        Effect::Menu { target, at } => {
            close_menus();
            menu.set(Some(menu::Menu {
                target: target.clone(),
                window: canvas.pane_to_window(*at),
                flow: canvas.pane_to_flow(*at),
            }));
        }
        Effect::SelectionChanged | Effect::NodeClick(_) | Effect::PaneClick(_) => {
            close_menus();
            let first = canvas.selected().first().map(ToString::to_string);
            if store.selected.get_untracked() != first {
                store.selected.set(first);
            }
        }
        Effect::PaneDoubleClick(at) => store.open_palette_at(at.x as f32, at.y as f32),
        _ => {}
    }
}

pub fn remove_selection(store: Store, nodes: Vec<String>, edges: Vec<String>) {
    let graph = store.graph.get_untracked();
    let closing = remove::teardowns(store, &graph, &nodes);
    zgui::task::spawn_local(async move {
        remove::close(store, closing).await;
        store.edit_graph(move |graph| {
            *graph = graph::remove_edges(&graph::remove_nodes(graph, &nodes), &edges);
        });
    });
}

#[derive(Clone, Copy)]
struct Clipboard {
    held: StoredValue<Option<graph::Clipboard>, LocalStorage>,
    pastes: StoredValue<u32, LocalStorage>,
}

fn copy(store: Store, canvas: Canvas, clipboard: Clipboard) -> bool {
    let ids: Vec<String> = canvas.selected().iter().map(ToString::to_string).collect();
    let Some(copied) = graph::copy_nodes(&store.graph.get_untracked(), &ids) else {
        return false;
    };
    let count = copied.nodes.len();
    clipboard.held.set_value(Some(copied));
    clipboard.pastes.set_value(0);
    store.say(if count == 1 {
        String::from("Copied 1 node")
    } else {
        format!("Copied {count} nodes")
    });
    true
}

fn paste(store: Store, canvas: Canvas, clipboard: Clipboard) -> bool {
    let Some(held) = clipboard.held.get_value() else {
        return false;
    };
    let current = store.graph.get_untracked();
    if let Some(reason) = graph::paste_refusal(&current, &held) {
        store.say(reason);
        return true;
    }
    let pastes = clipboard.pastes.get_value() + 1;
    clipboard.pastes.set_value(pastes);
    let step = graph::PASTE_OFFSET_PX * pastes as f32;
    let (next, ids) = graph::paste_nodes(&current, &held, Position { x: step, y: step });
    store.edit_graph(move |graph| *graph = next);
    let chosen: Vec<Id> = ids.iter().map(|id| Id::from(id.as_str())).collect();
    canvas.select_only(&chosen);
    store.selected.set(ids.first().cloned());
    true
}

fn chord(key: &Key, held: zgui::prelude::Modifiers) -> Option<char> {
    if !(held.control() || held.meta()) || held.alt() || held.shift() {
        return None;
    }
    key.as_str()
        .and_then(|text| text.chars().next())
        .map(|letter| letter.to_ascii_lowercase())
        .filter(|letter| matches!(letter, 'c' | 'v'))
}

#[must_use]
pub fn canvas() -> Canvas {
    FlowHandle::new(Options::default())
}

pub fn pane(store: Store, canvas: Canvas) -> impl IntoView {
    let syncing = zgui::reactive::RenderEffect::new(move |_| {
        let graph = store.graph.get();
        store.state.with(|_| ());
        store.channel_types.with(|_| ());
        sync(store, canvas, &graph);
    });
    on_cleanup_local(move || drop(syncing));
    let following = zgui::reactive::RenderEffect::new(move |_| {
        let wanted = store.selected.get();
        let held = canvas.selected();
        let same = match &wanted {
            Some(id) => held.len() == 1 && *held[0] == **id,
            None => held.is_empty(),
        };
        if !same {
            let ids: Vec<Id> = wanted.iter().map(|id| Id::from(id.as_str())).collect();
            canvas.select_only(&ids);
        }
    });
    on_cleanup_local(move || drop(following));

    let style = FlowStyle::default();
    let open = RwSignal::new(None::<menu::Menu>);
    let clipboard = Clipboard {
        held: StoredValue::new_local(None),
        pastes: StoredValue::new_local(0),
    };
    let valid = move |connection: &Connection| {
        refusal(store, &store.graph.get_untracked(), connection).is_none()
    };
    let effects = move |effect: &Effect| on_effect(store, canvas, open, effect);
    let render = move |cx: NodeCx<PatchNode, PortType>| card(store, cx);
    let keys = move |ev: &mut EventCx<'_, events::KeyDown>| {
        if !canvas.is_pane(ev.target) {
            return;
        }
        let done = match chord(&ev.key_without_modifiers, ev.modifiers) {
            Some('c') => copy(store, canvas, clipboard),
            Some('v') => paste(store, canvas, clipboard),
            _ => false,
        };
        if done {
            ev.prevent_default();
        }
    };
    view! {
        box(class = "patch", on:key_down = keys) {
            {flow_view(canvas, style, valid, effects, render, view! {
                {controls(canvas)}
                {minimap(canvas, style)}
            })}
            {menu::view(store, canvas, open)}
        }
    }
}

fn card(store: Store, cx: NodeCx<PatchNode, PortType>) -> AnyView {
    let id = cx.id.to_string();
    let Some(node) = store.graph.get_untracked().node(&id).cloned() else {
        return AnyView::new(());
    };
    let remove = {
        let id = id.clone();
        move || remove_selection(store, vec![id.clone()], Vec::new())
    };
    let category = node::category_class(node.body.category());
    let body = faces::face(store, &node);
    let bar = header::header(store, id, cx.drag_handle(), remove);
    AnyView::new(view! {
        column(class = "node", attr:data-category = category) {
            {bar}
            column(class = "node__body") {
                {body}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{ChannelNode, DeviceNode, Position};

    use super::*;

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn channel(kind: &str) -> NodeBody {
        NodeBody::Channel(ChannelNode {
            channel_type: kind.to_owned(),
            record_calls: false,
            tuning_locked: false,
        })
    }

    #[test]
    fn a_channel_that_changes_decoder_gets_a_key_of_its_own() {
        assert_ne!(
            shape_of(&node("ch", channel("nfm"))),
            shape_of(&node("ch", channel("wfm")))
        );
        assert_ne!(
            shape_of(&node("ch", channel("nfm"))),
            shape_of(&node("ch", NodeBody::Scope))
        );
    }

    #[test]
    fn a_moved_node_keeps_its_key() {
        let mut moved = node("device", NodeBody::Device(DeviceNode::default()));
        let before = shape_of(&moved);
        moved.position = Position { x: 400.0, y: 90.0 };
        assert_eq!(shape_of(&moved), before);
    }

    #[test]
    fn inputs_become_targets_on_the_left_and_outputs_sources_on_the_right() {
        let places = node::places(&node("scope", NodeBody::Scope), None);
        for handle in handles_of(&places) {
            match handle.kind {
                HandleKind::Target => assert_eq!(handle.side, Side::Left),
                HandleKind::Source => assert_eq!(handle.side, Side::Right),
            }
        }
    }

    #[test]
    fn an_edge_and_its_connection_convert_both_ways() {
        let edge = PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned(),
            },
        };
        assert_eq!(edge_of(&connection_of(&edge)), edge);
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
        for (at, port) in all.iter().enumerate() {
            for other in &all[at + 1..] {
                assert_ne!(
                    wire_colour(*port),
                    wire_colour(*other),
                    "{port:?} and {other:?}"
                );
            }
        }
    }
}
