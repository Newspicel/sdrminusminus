use kurbo::{Point, Rect, Size, Vec2};

use crate::{
    drag::{DRAG_THRESHOLD, SnapGrid, dragged, past_threshold},
    hit::{SelectionMode, caught, path_crosses, path_hit},
    model::{Connection, Edge, EdgeChange, HandleKind, HandleRef, Id, Node, NodeChange},
    path::{EdgeShape, Endpoints},
    resize::{Grip, Limits, resized},
    viewport::{Padding, Viewport, ZoomRange, bounds_of, wheel_zoom_factor},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn command(self) -> bool {
        self.ctrl || self.meta
    }

    #[must_use]
    pub const fn adds(self) -> bool {
        self.shift || self.ctrl || self.meta
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Button {
    #[default]
    Primary,
    Middle,
    Secondary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Pane,
    Node(Id),
    NodeBody(Id),
    Handle(HandleRef),
    Grip(Id, Grip),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanOnDrag {
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub zoom: ZoomRange,
    pub snap: Option<SnapGrid>,
    pub pan_on_drag: PanOnDrag,
    pub pan_on_scroll: bool,
    pub selection_mode: SelectionMode,
    pub connection_radius: f64,
    pub edge_reach: f64,
    pub edge_shape: EdgeShape,
    pub resize_limits: Limits,
    pub fit_padding: f64,
    pub connect_on_click: bool,
    pub keyboard_step: f64,
    pub fit_on_start: bool,
    pub dash_speed: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            zoom: ZoomRange {
                min: 0.15,
                max: 2.0,
            },
            snap: None,
            pan_on_drag: PanOnDrag::Always,
            pan_on_scroll: true,
            selection_mode: SelectionMode::Partial,
            connection_radius: 24.0,
            edge_reach: 6.0,
            edge_shape: EdgeShape::default(),
            resize_limits: Limits::default(),
            fit_padding: 0.12,
            connect_on_click: true,
            keyboard_step: 10.0,
            fit_on_start: true,
            dash_speed: 24.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub from: HandleRef,
    pub anchor: Point,
    pub pointer: Point,
    pub hover: Option<HandleRef>,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    Pan {
        from: Point,
        start: Viewport,
        moved: bool,
    },
    Select {
        from: Point,
        to: Point,
        keep: Vec<Id>,
        moved: bool,
    },
    Drag {
        from: Point,
        origins: Vec<(Id, Point)>,
        moved: bool,
    },
    Connect {
        press: Point,
        moved: bool,
    },
    Resize {
        id: Id,
        grip: Grip,
        from: Point,
        start: Rect,
        min: Size,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Nodes(Vec<NodeChange>),
    Edges(Vec<EdgeChange>),
    Connect(Connection),
    ConnectEnd { from: HandleRef, at: Point },
    DragStop(Vec<Id>),
    ResizeStop(Id),
    PaneClick(Point),
    PaneDoubleClick(Point),
    NodeClick(Id),
    EdgeClick(Id),
    Menu { target: MenuTarget, at: Point },
    Delete { nodes: Vec<Id>, edges: Vec<Id> },
    SelectionChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    Pane,
    Node(Id),
    Edge(Id),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Delete,
    Escape,
    SelectAll,
    Left,
    Right,
    Up,
    Down,
}

pub struct Scene<'a, T, E> {
    pub nodes: &'a [Node<T>],
    pub edges: &'a [Edge<E>],
    pub valid: &'a dyn Fn(&Connection) -> bool,
}

impl<T, E> Scene<'_, T, E> {
    fn node(&self, id: &str) -> Option<&Node<T>> {
        self.nodes
            .iter()
            .find(|node| &*node.id == id && !node.hidden)
    }

    fn anchor(&self, handle: &HandleRef) -> Option<Point> {
        let node = self.node(&handle.node)?;
        Some(
            node.handle(&handle.handle, handle.kind)?
                .anchor(node.frame()),
        )
    }

    fn selected(&self) -> Vec<Id> {
        self.nodes
            .iter()
            .filter(|node| node.selected)
            .map(|node| node.id.clone())
            .collect()
    }

    #[must_use]
    pub fn endpoints(&self, edge: &Edge<E>) -> Option<Endpoints> {
        let source = self.node(&edge.source)?;
        let target = self.node(&edge.target)?;
        let out = source.handle(&edge.source_handle, HandleKind::Source)?;
        let input = target.handle(&edge.target_handle, HandleKind::Target)?;
        Some(Endpoints {
            source: out.anchor(source.frame()),
            source_side: out.side,
            target: input.anchor(target.frame()),
            target_side: input.side,
        })
    }

    #[must_use]
    pub fn edge_at(&self, flow: Point, shape: EdgeShape, reach: f64) -> Option<Id> {
        self.edges.iter().rev().find_map(|edge| {
            let ends = self.endpoints(edge)?;
            let routed = edge.shape.unwrap_or(shape).route(ends);
            path_hit(&routed.path, flow, reach).then(|| edge.id.clone())
        })
    }

    fn closest_handle(&self, from: &HandleRef, at: Point, radius: f64) -> Option<HandleRef> {
        let mut best: Option<(f64, HandleRef)> = None;
        for node in self.nodes.iter().filter(|node| !node.hidden) {
            if !node.frame().inflate(radius, radius).contains(at) {
                continue;
            }
            for handle in node.handles.iter().filter(|handle| handle.connectable) {
                if node.id == from.node && handle.id == from.handle && handle.kind == from.kind {
                    continue;
                }
                let distance = (handle.anchor(node.frame()) - at).hypot();
                if distance > radius {
                    continue;
                }
                let candidate = HandleRef {
                    node: node.id.clone(),
                    handle: handle.id.clone(),
                    kind: handle.kind,
                };
                let better = match &best {
                    None => true,
                    Some((held, current)) => {
                        distance < *held
                            || (distance == *held
                                && candidate.kind == from.kind.opposite()
                                && current.kind == from.kind)
                    }
                };
                if better {
                    best = Some((distance, candidate));
                }
            }
        }
        best.map(|(_, handle)| handle)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flow {
    pub viewport: Viewport,
    pub screen: Size,
    pub options: Options,
    pub pending: Option<Pending>,
    pub selection_box: Option<Rect>,
    gesture: Option<Gesture>,
}

impl Flow {
    #[must_use]
    pub fn new(options: Options) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.gesture.is_some()
    }

    #[must_use]
    pub fn dragging(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Drag { moved: true, .. }))
    }

    #[must_use]
    pub fn panning(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Pan { moved: true, .. }))
    }

    pub fn fit<T>(&mut self, nodes: &[Node<T>]) {
        let Some(bounds) = bounds_of(nodes.iter().filter(|node| !node.hidden).map(Node::frame))
        else {
            return;
        };
        self.viewport = self.fitted(bounds);
    }

    #[must_use]
    pub fn fitted(&self, bounds: Rect) -> Viewport {
        Viewport::fitting(
            bounds,
            self.screen,
            ZoomRange {
                min: self.options.zoom.min,
                max: self.options.zoom.max.min(1.0),
            },
            Padding::fraction(self.options.fit_padding, self.screen),
        )
    }

    pub fn zoom_by(&mut self, factor: f64) {
        let centre = Point::new(self.screen.width / 2.0, self.screen.height / 2.0);
        self.viewport = self.viewport.scaled_at(centre, factor, self.options.zoom);
    }

    pub fn wheel(&mut self, at: Point, delta: Vec2, pixels: bool, modifiers: Modifiers) {
        let zooms = modifiers.command() || !self.options.pan_on_scroll;
        if zooms {
            let factor = wheel_zoom_factor(delta.y, pixels, modifiers.ctrl);
            self.viewport = self.viewport.scaled_at(at, factor, self.options.zoom);
            return;
        }
        let scale = if pixels { 1.0 } else { 20.0 };
        let mut shift = delta * scale;
        if modifiers.shift && shift.x == 0.0 {
            shift = Vec2::new(shift.y, 0.0);
        }
        self.viewport = self.viewport.panned(-shift);
    }

    pub fn magnify(&mut self, at: Point, factor: f64) {
        self.viewport = self.viewport.scaled_at(at, factor, self.options.zoom);
    }

    pub fn press<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        target: Target,
        at: Point,
        button: Button,
        modifiers: Modifiers,
    ) -> Vec<Effect> {
        let flow = self.viewport.to_flow(at);
        match button {
            Button::Secondary => return self.menu(scene, target, at, flow),
            Button::Middle => {
                self.gesture = Some(Gesture::Pan {
                    from: at,
                    start: self.viewport,
                    moved: false,
                });
                return Vec::new();
            }
            Button::Primary => {}
        }
        match target {
            Target::Handle(handle) => self.press_handle(scene, handle, at),
            Target::Grip(id, grip) => {
                if let Some(node) = scene.node(&id) {
                    self.gesture = Some(Gesture::Resize {
                        id,
                        grip,
                        from: flow,
                        start: node.frame(),
                        min: node.min_size,
                    });
                }
                Vec::new()
            }
            Target::Node(id) => self.press_node(scene, &id, at, modifiers, true),
            Target::NodeBody(id) => self.press_node(scene, &id, at, modifiers, false),
            Target::Pane => self.press_pane(scene, at, flow, modifiers),
        }
    }

    fn menu<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        target: Target,
        at: Point,
        flow: Point,
    ) -> Vec<Effect> {
        self.pending = None;
        let target = match target {
            Target::Node(id) | Target::NodeBody(id) | Target::Grip(id, _) => MenuTarget::Node(id),
            Target::Handle(handle) => MenuTarget::Node(handle.node),
            Target::Pane => scene
                .edge_at(flow, self.options.edge_shape, self.reach())
                .map_or(MenuTarget::Pane, MenuTarget::Edge),
        };
        vec![Effect::Menu { target, at }]
    }

    fn reach(&self) -> f64 {
        self.options.edge_reach / self.viewport.zoom
    }

    fn press_handle<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        handle: HandleRef,
        at: Point,
    ) -> Vec<Effect> {
        if let Some(pending) = self.pending.take() {
            if let Some(connection) = pending.from.connect_to(&handle)
                && (scene.valid)(&connection)
            {
                return vec![Effect::Connect(connection)];
            }
            if pending.from == handle {
                return Vec::new();
            }
        }
        let Some(anchor) = scene.anchor(&handle) else {
            return Vec::new();
        };
        let flow = self.viewport.to_flow(at);
        self.pending = Some(Pending {
            from: handle,
            anchor,
            pointer: flow,
            hover: None,
            valid: false,
        });
        self.gesture = Some(Gesture::Connect {
            press: at,
            moved: false,
        });
        Vec::new()
    }

    fn press_node<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        id: &Id,
        at: Point,
        modifiers: Modifiers,
        drags: bool,
    ) -> Vec<Effect> {
        self.pending = None;
        let Some(node) = scene.node(id) else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        if modifiers.adds() {
            if node.selectable {
                changes.push(NodeChange::Select {
                    id: id.clone(),
                    selected: !node.selected,
                });
            }
        } else if !node.selected && node.selectable {
            changes.extend(deselect_all(scene, Some(id)));
            changes.push(NodeChange::Select {
                id: id.clone(),
                selected: true,
            });
        }
        let dragging: Vec<Id> = if node.selected || modifiers.adds() {
            let mut held = scene.selected();
            if !held.contains(id) && !node.selected {
                held.push(id.clone());
            }
            held
        } else {
            vec![id.clone()]
        };
        let origins = dragging
            .iter()
            .filter_map(|held| scene.node(held))
            .filter(|node| node.draggable)
            .map(|node| (node.id.clone(), node.position))
            .collect();
        if drags {
            self.gesture = Some(Gesture::Drag {
                from: at,
                origins,
                moved: false,
            });
        }
        let mut effects = Vec::new();
        let edges = deselect_edges(scene);
        if !edges.is_empty() {
            effects.push(Effect::Edges(edges));
        }
        if !changes.is_empty() {
            effects.push(Effect::Nodes(changes));
            effects.push(Effect::SelectionChanged);
        }
        effects
    }

    fn press_pane<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        at: Point,
        flow: Point,
        modifiers: Modifiers,
    ) -> Vec<Effect> {
        self.pending = None;
        if let Some(edge) = scene.edge_at(flow, self.options.edge_shape, self.reach()) {
            return self.pick_edge(scene, edge, modifiers);
        }
        if modifiers.shift || self.options.pan_on_drag == PanOnDrag::Never {
            let keep = if modifiers.command() || modifiers.shift {
                scene.selected()
            } else {
                Vec::new()
            };
            self.gesture = Some(Gesture::Select {
                from: at,
                to: at,
                keep,
                moved: false,
            });
            return Vec::new();
        }
        self.gesture = Some(Gesture::Pan {
            from: at,
            start: self.viewport,
            moved: false,
        });
        Vec::new()
    }

    fn pick_edge<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        id: Id,
        modifiers: Modifiers,
    ) -> Vec<Effect> {
        let mut edges = Vec::new();
        if !modifiers.adds() {
            edges.extend(
                scene
                    .edges
                    .iter()
                    .filter(|edge| edge.selected && edge.id != id)
                    .map(|edge| EdgeChange::Select {
                        id: edge.id.clone(),
                        selected: false,
                    }),
            );
        }
        let now = scene
            .edges
            .iter()
            .find(|edge| edge.id == id)
            .is_some_and(|edge| edge.selectable && (!edge.selected || !modifiers.adds()));
        edges.push(EdgeChange::Select {
            id: id.clone(),
            selected: now,
        });
        let mut effects = vec![Effect::Edges(edges)];
        if !modifiers.adds() {
            let nodes = deselect_all(scene, None);
            if !nodes.is_empty() {
                effects.push(Effect::Nodes(nodes));
            }
        }
        effects.push(Effect::EdgeClick(id));
        effects.push(Effect::SelectionChanged);
        effects
    }

    pub fn motion<T, E>(&mut self, scene: &Scene<'_, T, E>, at: Point) -> Vec<Effect> {
        let flow = self.viewport.to_flow(at);
        let Some(gesture) = self.gesture.as_mut() else {
            if let Some(pending) = self.pending.as_mut() {
                pending.pointer = flow;
                let hover =
                    scene.closest_handle(&pending.from, flow, self.options.connection_radius);
                pending.valid = hover
                    .as_ref()
                    .and_then(|hover| pending.from.connect_to(hover))
                    .is_some_and(|connection| (scene.valid)(&connection));
                pending.hover = hover;
            }
            return Vec::new();
        };
        match gesture {
            Gesture::Pan { from, start, moved } => {
                *moved |= past_threshold(*from, at, DRAG_THRESHOLD);
                self.viewport = start.panned(at - *from);
                Vec::new()
            }
            Gesture::Select {
                from,
                to,
                keep,
                moved,
            } => {
                *to = at;
                *moved |= past_threshold(*from, at, DRAG_THRESHOLD);
                let area = self.viewport.rect_to_flow(Rect::from_points(*from, *to));
                self.selection_box = Some(Rect::from_points(*from, *to));
                let keep = keep.clone();
                box_select(scene, area, &keep, self.options.selection_mode)
            }
            Gesture::Drag {
                from,
                origins,
                moved,
            } => {
                if !*moved && !past_threshold(*from, at, DRAG_THRESHOLD) {
                    return Vec::new();
                }
                *moved = true;
                let by = (at - *from) / self.viewport.zoom;
                let starts: Vec<Point> = origins.iter().map(|(_, origin)| *origin).collect();
                let placed = dragged(&starts, by, self.options.snap);
                let changes = origins
                    .iter()
                    .zip(placed)
                    .map(|((id, _), position)| NodeChange::Position {
                        id: id.clone(),
                        position,
                        dragging: true,
                    })
                    .collect();
                vec![Effect::Nodes(changes)]
            }
            Gesture::Connect { press, moved } => {
                *moved |= past_threshold(*press, at, DRAG_THRESHOLD);
                if let Some(pending) = self.pending.as_mut() {
                    pending.pointer = flow;
                    let hover =
                        scene.closest_handle(&pending.from, flow, self.options.connection_radius);
                    pending.valid = hover
                        .as_ref()
                        .and_then(|hover| pending.from.connect_to(hover))
                        .is_some_and(|connection| (scene.valid)(&connection));
                    pending.hover = hover;
                }
                Vec::new()
            }
            Gesture::Resize {
                id,
                grip,
                from,
                start,
                min,
            } => {
                let pointer = self.options.snap.map_or(flow, |grid| grid.snap(flow));
                let anchor = self.options.snap.map_or(*from, |grid| grid.snap(*from));
                let limits = Limits {
                    min: Size::new(
                        self.options.resize_limits.min.width.max(min.width),
                        self.options.resize_limits.min.height.max(min.height),
                    ),
                    max: self.options.resize_limits.max,
                };
                let frame = resized(*start, *grip, anchor, pointer, limits, false);
                vec![Effect::Nodes(vec![NodeChange::Size {
                    id: id.clone(),
                    position: frame.origin(),
                    size: frame.size(),
                    resizing: true,
                }])]
            }
        }
    }

    pub fn release<T, E>(&mut self, scene: &Scene<'_, T, E>, at: Point) -> Vec<Effect> {
        let Some(gesture) = self.gesture.take() else {
            return Vec::new();
        };
        self.selection_box = None;
        match gesture {
            Gesture::Pan { moved, .. } => {
                if moved {
                    Vec::new()
                } else {
                    let mut effects = Vec::new();
                    let nodes = deselect_all(scene, None);
                    let edges = deselect_edges(scene);
                    let changed = !nodes.is_empty() || !edges.is_empty();
                    if !nodes.is_empty() {
                        effects.push(Effect::Nodes(nodes));
                    }
                    if !edges.is_empty() {
                        effects.push(Effect::Edges(edges));
                    }
                    if changed {
                        effects.push(Effect::SelectionChanged);
                    }
                    effects.push(Effect::PaneClick(self.viewport.to_flow(at)));
                    effects
                }
            }
            Gesture::Select { moved, .. } => {
                if moved {
                    vec![Effect::SelectionChanged]
                } else {
                    vec![Effect::PaneClick(self.viewport.to_flow(at))]
                }
            }
            Gesture::Drag { origins, moved, .. } => {
                if moved {
                    let ids: Vec<Id> = origins.into_iter().map(|(id, _)| id).collect();
                    let settled = ids
                        .iter()
                        .filter_map(|id| scene.node(id))
                        .map(|node| NodeChange::Position {
                            id: node.id.clone(),
                            position: node.position,
                            dragging: false,
                        })
                        .collect();
                    vec![Effect::Nodes(settled), Effect::DragStop(ids)]
                } else {
                    origins
                        .into_iter()
                        .next()
                        .map(|(id, _)| Effect::NodeClick(id))
                        .into_iter()
                        .collect()
                }
            }
            Gesture::Connect { moved, .. } => self.finish_connect(scene, moved),
            Gesture::Resize { id, .. } => vec![Effect::ResizeStop(id)],
        }
    }

    fn finish_connect<T, E>(&mut self, scene: &Scene<'_, T, E>, moved: bool) -> Vec<Effect> {
        if !moved && self.options.connect_on_click {
            return Vec::new();
        }
        let Some(pending) = self.pending.take() else {
            return Vec::new();
        };
        if let Some(connection) = pending
            .hover
            .as_ref()
            .and_then(|hover| pending.from.connect_to(hover))
            .filter(|connection| (scene.valid)(connection))
        {
            return vec![Effect::Connect(connection)];
        }
        vec![Effect::ConnectEnd {
            from: pending.from,
            at: pending.pointer,
        }]
    }

    pub fn cancel(&mut self) {
        self.gesture = None;
        self.pending = None;
        self.selection_box = None;
    }

    pub fn key<T, E>(
        &mut self,
        scene: &Scene<'_, T, E>,
        key: Key,
        modifiers: Modifiers,
    ) -> Vec<Effect> {
        match key {
            Key::Escape => {
                let had = self.pending.is_some() || self.gesture.is_some();
                self.cancel();
                if had {
                    return Vec::new();
                }
                let mut effects = Vec::new();
                let nodes = deselect_all(scene, None);
                let edges = deselect_edges(scene);
                if !nodes.is_empty() || !edges.is_empty() {
                    effects.push(Effect::Nodes(nodes));
                    effects.push(Effect::Edges(edges));
                    effects.push(Effect::SelectionChanged);
                }
                effects
            }
            Key::SelectAll => {
                let nodes = scene
                    .nodes
                    .iter()
                    .filter(|node| node.selectable && !node.hidden && !node.selected)
                    .map(|node| NodeChange::Select {
                        id: node.id.clone(),
                        selected: true,
                    })
                    .collect();
                vec![Effect::Nodes(nodes), Effect::SelectionChanged]
            }
            Key::Delete => deletion(scene),
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let step = self
                    .options
                    .snap
                    .map_or(self.options.keyboard_step, |grid| grid.x.max(1.0))
                    * if modifiers.shift { 4.0 } else { 1.0 };
                let by = match key {
                    Key::Left => Vec2::new(-step, 0.0),
                    Key::Right => Vec2::new(step, 0.0),
                    Key::Up => Vec2::new(0.0, -step),
                    _ => Vec2::new(0.0, step),
                };
                let moved: Vec<NodeChange> = scene
                    .nodes
                    .iter()
                    .filter(|node| node.selected && node.draggable)
                    .map(|node| NodeChange::Position {
                        id: node.id.clone(),
                        position: node.position + by,
                        dragging: false,
                    })
                    .collect();
                if moved.is_empty() {
                    return Vec::new();
                }
                let ids = scene.selected();
                vec![Effect::Nodes(moved), Effect::DragStop(ids)]
            }
        }
    }
}

fn deselect_all<T, E>(scene: &Scene<'_, T, E>, except: Option<&Id>) -> Vec<NodeChange> {
    scene
        .nodes
        .iter()
        .filter(|node| node.selected && Some(&node.id) != except)
        .map(|node| NodeChange::Select {
            id: node.id.clone(),
            selected: false,
        })
        .collect()
}

fn deselect_edges<T, E>(scene: &Scene<'_, T, E>) -> Vec<EdgeChange> {
    scene
        .edges
        .iter()
        .filter(|edge| edge.selected)
        .map(|edge| EdgeChange::Select {
            id: edge.id.clone(),
            selected: false,
        })
        .collect()
}

fn box_select<T, E>(
    scene: &Scene<'_, T, E>,
    area: Rect,
    keep: &[Id],
    mode: SelectionMode,
) -> Vec<Effect> {
    let nodes: Vec<NodeChange> = scene
        .nodes
        .iter()
        .filter(|node| node.selectable && !node.hidden)
        .filter_map(|node| {
            let inside = keep.contains(&node.id) || caught(node.frame(), area, mode);
            (inside != node.selected).then(|| NodeChange::Select {
                id: node.id.clone(),
                selected: inside,
            })
        })
        .collect();
    let edges: Vec<EdgeChange> = scene
        .edges
        .iter()
        .filter(|edge| edge.selectable)
        .filter_map(|edge| {
            let ends = scene.endpoints(edge)?;
            let inside = path_crosses(&EdgeShape::Straight.route(ends).path, area)
                && caught_end(scene, &edge.source, area, keep, mode)
                && caught_end(scene, &edge.target, area, keep, mode);
            (inside != edge.selected).then(|| EdgeChange::Select {
                id: edge.id.clone(),
                selected: inside,
            })
        })
        .collect();
    let mut effects = Vec::new();
    if !nodes.is_empty() {
        effects.push(Effect::Nodes(nodes));
    }
    if !edges.is_empty() {
        effects.push(Effect::Edges(edges));
    }
    effects
}

fn caught_end<T, E>(
    scene: &Scene<'_, T, E>,
    node: &str,
    area: Rect,
    keep: &[Id],
    mode: SelectionMode,
) -> bool {
    scene
        .node(node)
        .is_some_and(|node| keep.contains(&node.id) || caught(node.frame(), area, mode))
}

fn deletion<T, E>(scene: &Scene<'_, T, E>) -> Vec<Effect> {
    let nodes: Vec<Id> = scene
        .nodes
        .iter()
        .filter(|node| node.selected && node.deletable)
        .map(|node| node.id.clone())
        .collect();
    let edges: Vec<Id> = scene
        .edges
        .iter()
        .filter(|edge| {
            (edge.selected && edge.deletable) || nodes.iter().any(|node| edge.touches(node))
        })
        .map(|edge| edge.id.clone())
        .collect();
    if nodes.is_empty() && edges.is_empty() {
        return Vec::new();
    }
    vec![Effect::Delete { nodes, edges }]
}

#[cfg(test)]
mod tests;
