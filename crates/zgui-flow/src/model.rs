use std::sync::Arc;

use kurbo::{Point, Rect, Size};

use crate::path::{EdgeShape, Side};

pub type Id = Arc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandleKind {
    Source,
    Target,
}

impl HandleKind {
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Source => Self::Target,
            Self::Target => Self::Source,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Handle {
    pub id: Id,
    pub kind: HandleKind,
    pub side: Side,
    pub offset: f64,
    pub label: Option<String>,
    pub class: Option<String>,
    pub connectable: bool,
}

impl Handle {
    #[must_use]
    pub fn new(id: impl Into<Id>, kind: HandleKind, side: Side, offset: f64) -> Self {
        Self {
            id: id.into(),
            kind,
            side,
            offset,
            label: None,
            class: None,
            connectable: true,
        }
    }

    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    #[must_use]
    pub fn anchor(&self, frame: Rect) -> Point {
        match self.side {
            Side::Left => Point::new(frame.x0, frame.y0 + self.offset),
            Side::Right => Point::new(frame.x1, frame.y0 + self.offset),
            Side::Top => Point::new(frame.x0 + self.offset, frame.y0),
            Side::Bottom => Point::new(frame.x0 + self.offset, frame.y1),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node<T> {
    pub id: Id,
    pub position: Point,
    pub size: Size,
    pub data: T,
    pub handles: Vec<Handle>,
    pub selected: bool,
    pub draggable: bool,
    pub selectable: bool,
    pub deletable: bool,
    pub resizable: bool,
    pub hidden: bool,
    pub z: i32,
    pub class: Option<String>,
}

impl<T> Node<T> {
    #[must_use]
    pub fn new(id: impl Into<Id>, position: Point, size: Size, data: T) -> Self {
        Self {
            id: id.into(),
            position,
            size,
            data,
            handles: Vec::new(),
            selected: false,
            draggable: true,
            selectable: true,
            deletable: true,
            resizable: false,
            hidden: false,
            z: 0,
            class: None,
        }
    }

    #[must_use]
    pub fn frame(&self) -> Rect {
        Rect::from_origin_size(self.position, self.size)
    }

    #[must_use]
    pub fn handle(&self, id: &str) -> Option<&Handle> {
        self.handles.iter().find(|handle| &*handle.id == id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge<E> {
    pub id: Id,
    pub source: Id,
    pub source_handle: Id,
    pub target: Id,
    pub target_handle: Id,
    pub data: E,
    pub selected: bool,
    pub selectable: bool,
    pub deletable: bool,
    pub animated: bool,
    pub shape: Option<EdgeShape>,
    pub label: Option<String>,
    pub class: Option<String>,
}

impl<E> Edge<E> {
    #[must_use]
    pub fn new(connection: &Connection, data: E) -> Self {
        Self {
            id: connection.key().into(),
            source: connection.source.clone(),
            source_handle: connection.source_handle.clone(),
            target: connection.target.clone(),
            target_handle: connection.target_handle.clone(),
            data,
            selected: false,
            selectable: true,
            deletable: true,
            animated: false,
            shape: None,
            label: None,
            class: None,
        }
    }

    #[must_use]
    pub fn connection(&self) -> Connection {
        Connection {
            source: self.source.clone(),
            source_handle: self.source_handle.clone(),
            target: self.target.clone(),
            target_handle: self.target_handle.clone(),
        }
    }

    #[must_use]
    pub fn touches(&self, node: &str) -> bool {
        &*self.source == node || &*self.target == node
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Connection {
    pub source: Id,
    pub source_handle: Id,
    pub target: Id,
    pub target_handle: Id,
}

impl Connection {
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}:{}->{}:{}",
            self.source, self.source_handle, self.target, self.target_handle
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HandleRef {
    pub node: Id,
    pub handle: Id,
    pub kind: HandleKind,
}

impl HandleRef {
    #[must_use]
    pub fn connect_to(&self, other: &Self) -> Option<Connection> {
        if self.kind == other.kind || self.node == other.node {
            return None;
        }
        let (source, target) = match self.kind {
            HandleKind::Source => (self, other),
            HandleKind::Target => (other, self),
        };
        Some(Connection {
            source: source.node.clone(),
            source_handle: source.handle.clone(),
            target: target.node.clone(),
            target_handle: target.handle.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeChange {
    Position {
        id: Id,
        position: Point,
        dragging: bool,
    },
    Size {
        id: Id,
        position: Point,
        size: Size,
        resizing: bool,
    },
    Select {
        id: Id,
        selected: bool,
    },
    Remove {
        id: Id,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum EdgeChange {
    Select { id: Id, selected: bool },
    Remove { id: Id },
}

pub fn apply_node_changes<T>(nodes: &mut Vec<Node<T>>, changes: &[NodeChange]) {
    for change in changes {
        match change {
            NodeChange::Remove { id } => nodes.retain(|node| node.id != *id),
            NodeChange::Position { id, position, .. } => {
                if let Some(node) = nodes.iter_mut().find(|node| node.id == *id) {
                    node.position = *position;
                }
            }
            NodeChange::Size {
                id, position, size, ..
            } => {
                if let Some(node) = nodes.iter_mut().find(|node| node.id == *id) {
                    node.position = *position;
                    node.size = *size;
                }
            }
            NodeChange::Select { id, selected } => {
                if let Some(node) = nodes.iter_mut().find(|node| node.id == *id) {
                    node.selected = *selected;
                }
            }
        }
    }
}

pub fn apply_edge_changes<E>(edges: &mut Vec<Edge<E>>, changes: &[EdgeChange]) {
    for change in changes {
        match change {
            EdgeChange::Remove { id } => edges.retain(|edge| edge.id != *id),
            EdgeChange::Select { id, selected } => {
                if let Some(edge) = edges.iter_mut().find(|edge| edge.id == *id) {
                    edge.selected = *selected;
                }
            }
        }
    }
}

pub fn connected_edges<'a, E>(
    edges: &'a [Edge<E>],
    nodes: &'a [Id],
) -> impl Iterator<Item = &'a Edge<E>> {
    edges
        .iter()
        .filter(move |edge| nodes.iter().any(|node| edge.touches(node)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, x: f64) -> Node<()> {
        Node::new(id, Point::new(x, 0.0), Size::new(100.0, 50.0), ())
    }

    fn link(from: &str, to: &str) -> Edge<()> {
        Edge::new(
            &Connection {
                source: from.into(),
                source_handle: "out".into(),
                target: to.into(),
                target_handle: "in".into(),
            },
            (),
        )
    }

    #[test]
    fn a_handle_anchors_on_its_side_at_its_offset() {
        let frame = Rect::new(10.0, 20.0, 110.0, 80.0);
        assert_eq!(
            Handle::new("a", HandleKind::Target, Side::Left, 15.0).anchor(frame),
            Point::new(10.0, 35.0)
        );
        assert_eq!(
            Handle::new("b", HandleKind::Source, Side::Right, 15.0).anchor(frame),
            Point::new(110.0, 35.0)
        );
        assert_eq!(
            Handle::new("c", HandleKind::Source, Side::Bottom, 50.0).anchor(frame),
            Point::new(60.0, 80.0)
        );
        assert_eq!(
            Handle::new("d", HandleKind::Target, Side::Top, 50.0).anchor(frame),
            Point::new(60.0, 20.0)
        );
    }

    #[test]
    fn two_handles_connect_source_first_whichever_was_grabbed() {
        let out = HandleRef {
            node: "a".into(),
            handle: "out".into(),
            kind: HandleKind::Source,
        };
        let input = HandleRef {
            node: "b".into(),
            handle: "in".into(),
            kind: HandleKind::Target,
        };
        let forward = out.connect_to(&input);
        let backward = input.connect_to(&out);
        assert_eq!(forward, backward);
        assert_eq!(
            forward.map(|connection| connection.source),
            Some("a".into())
        );
    }

    #[test]
    fn a_handle_cannot_connect_to_its_own_kind_or_its_own_node() {
        let a = HandleRef {
            node: "a".into(),
            handle: "out".into(),
            kind: HandleKind::Source,
        };
        let b = HandleRef {
            node: "b".into(),
            handle: "out".into(),
            kind: HandleKind::Source,
        };
        let own = HandleRef {
            node: "a".into(),
            handle: "in".into(),
            kind: HandleKind::Target,
        };
        assert!(a.connect_to(&b).is_none());
        assert!(a.connect_to(&own).is_none());
    }

    #[test]
    fn node_changes_move_resize_select_and_remove() {
        let mut nodes = vec![node("a", 0.0), node("b", 200.0)];
        apply_node_changes(
            &mut nodes,
            &[
                NodeChange::Position {
                    id: "a".into(),
                    position: Point::new(5.0, 6.0),
                    dragging: true,
                },
                NodeChange::Select {
                    id: "b".into(),
                    selected: true,
                },
                NodeChange::Size {
                    id: "b".into(),
                    position: Point::new(190.0, 0.0),
                    size: Size::new(300.0, 70.0),
                    resizing: false,
                },
            ],
        );
        assert_eq!(nodes[0].position, Point::new(5.0, 6.0));
        assert!(nodes[1].selected);
        assert_eq!(nodes[1].frame(), Rect::new(190.0, 0.0, 490.0, 70.0));
        apply_node_changes(&mut nodes, &[NodeChange::Remove { id: "a".into() }]);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn edge_changes_select_and_remove() {
        let mut edges = vec![link("a", "b"), link("b", "c")];
        let first = edges[0].id.clone();
        apply_edge_changes(
            &mut edges,
            &[EdgeChange::Select {
                id: first.clone(),
                selected: true,
            }],
        );
        assert!(edges[0].selected);
        apply_edge_changes(&mut edges, &[EdgeChange::Remove { id: first }]);
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn an_edge_id_names_both_of_its_ends() {
        let edge = link("a", "b");
        assert_eq!(&*edge.id, "a:out->b:in");
        assert_eq!(edge.connection().key(), "a:out->b:in");
    }

    #[test]
    fn connected_edges_are_those_touching_any_given_node() {
        let edges = vec![link("a", "b"), link("b", "c"), link("c", "d")];
        let touching: Vec<_> = connected_edges(&edges, &["a".into(), "d".into()])
            .map(|edge| edge.id.clone())
            .collect();
        assert_eq!(touching.len(), 2);
    }
}
