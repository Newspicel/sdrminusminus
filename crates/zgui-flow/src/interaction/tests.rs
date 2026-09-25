use kurbo::{Point, Size, Vec2};

use super::*;
use crate::{
    model::{Handle, HandleKind, apply_edge_changes, apply_node_changes},
    path::Side,
};

struct World {
    nodes: Vec<Node<()>>,
    edges: Vec<Edge<()>>,
    flow: Flow,
}

fn anything(_: &Connection) -> bool {
    true
}

fn box_node(id: &str, x: f64, y: f64) -> Node<()> {
    let mut node = Node::new(id, Point::new(x, y), Size::new(100.0, 60.0), ());
    node.handles = vec![
        Handle::new("in", HandleKind::Target, Side::Left, 30.0),
        Handle::new("out", HandleKind::Source, Side::Right, 30.0),
    ];
    node
}

impl World {
    fn new() -> Self {
        let mut flow = Flow::new(Options::default());
        flow.screen = Size::new(800.0, 600.0);
        Self {
            nodes: vec![
                box_node("a", 0.0, 0.0),
                box_node("b", 300.0, 0.0),
                box_node("c", 0.0, 300.0),
            ],
            edges: Vec::new(),
            flow,
        }
    }

    fn apply(&mut self, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::Nodes(changes) => apply_node_changes(&mut self.nodes, changes),
                Effect::Edges(changes) => apply_edge_changes(&mut self.edges, changes),
                Effect::Connect(connection) => self.edges.push(Edge::new(connection, ())),
                _ => {}
            }
        }
    }

    fn run(
        &mut self,
        act: impl FnOnce(&mut Flow, &Scene<'_, (), ()>) -> Vec<Effect>,
    ) -> Vec<Effect> {
        let nodes = self.nodes.clone();
        let edges = self.edges.clone();
        let scene = Scene {
            nodes: &nodes,
            edges: &edges,
            valid: &anything,
        };
        let effects = act(&mut self.flow, &scene);
        self.apply(&effects);
        effects
    }

    fn press(&mut self, target: Target, at: (f64, f64), modifiers: Modifiers) -> Vec<Effect> {
        self.run(|flow, scene| flow.press(scene, target, at.into(), Button::Primary, modifiers))
    }

    fn motion(&mut self, at: (f64, f64)) -> Vec<Effect> {
        self.run(|flow, scene| flow.motion(scene, at.into()))
    }

    fn release(&mut self, at: (f64, f64)) -> Vec<Effect> {
        self.run(|flow, scene| flow.release(scene, at.into()))
    }

    fn selected(&self) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|node| node.selected)
            .map(|node| &*node.id)
            .collect()
    }

    fn position(&self, id: &str) -> Point {
        self.nodes
            .iter()
            .find(|node| &*node.id == id)
            .map_or(Point::ZERO, |node| node.position)
    }

    fn link(&mut self, from: &str, to: &str) {
        self.edges.push(Edge::new(
            &Connection {
                source: from.into(),
                source_handle: "out".into(),
                target: to.into(),
                target_handle: "in".into(),
            },
            (),
        ));
    }
}

fn node(id: &str) -> Target {
    Target::Node(id.into())
}

fn handle(node: &str, handle: &str, kind: HandleKind) -> Target {
    Target::Handle(HandleRef {
        node: node.into(),
        handle: handle.into(),
        kind,
    })
}

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    meta: false,
};

const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
    meta: false,
};

#[test]
fn clicking_a_node_selects_only_it() {
    let mut world = World::new();
    world.nodes[1].selected = true;
    world.press(node("a"), (10.0, 10.0), NONE);
    let effects = world.release((10.0, 10.0));
    assert_eq!(world.selected(), vec!["a"]);
    assert!(effects.contains(&Effect::NodeClick("a".into())));
}

#[test]
fn shift_clicking_toggles_a_node_in_and_out_of_the_selection() {
    let mut world = World::new();
    world.press(node("a"), (10.0, 10.0), NONE);
    world.release((10.0, 10.0));
    world.press(node("b"), (310.0, 10.0), SHIFT);
    world.release((310.0, 10.0));
    assert_eq!(world.selected(), vec!["a", "b"]);
    world.press(node("a"), (10.0, 10.0), SHIFT);
    world.release((10.0, 10.0));
    assert_eq!(world.selected(), vec!["b"]);
}

#[test]
fn dragging_a_node_moves_it_by_the_pointer_in_flow_units() {
    let mut world = World::new();
    world.flow.viewport.zoom = 2.0;
    world.press(node("a"), (10.0, 10.0), NONE);
    world.motion((50.0, 30.0));
    let effects = world.release((50.0, 30.0));
    assert_eq!(world.position("a"), Point::new(20.0, 10.0));
    assert!(effects.contains(&Effect::DragStop(vec!["a".into()])));
}

#[test]
fn dragging_a_selected_node_carries_the_whole_selection() {
    let mut world = World::new();
    world.nodes[0].selected = true;
    world.nodes[1].selected = true;
    world.press(node("a"), (10.0, 10.0), NONE);
    world.motion((30.0, 40.0));
    world.release((30.0, 40.0));
    assert_eq!(world.position("a"), Point::new(20.0, 30.0));
    assert_eq!(world.position("b"), Point::new(320.0, 30.0));
    assert_eq!(world.position("c"), Point::new(0.0, 300.0));
}

#[test]
fn dragging_snaps_to_the_grid() {
    let mut world = World::new();
    world.flow.options.snap = Some(SnapGrid::square(25.0));
    world.press(node("a"), (0.0, 0.0), NONE);
    world.motion((37.0, 12.0));
    world.release((37.0, 12.0));
    assert_eq!(world.position("a"), Point::new(25.0, 0.0));
}

#[test]
fn a_press_without_movement_is_a_click_not_a_drag() {
    let mut world = World::new();
    world.press(node("a"), (10.0, 10.0), NONE);
    world.motion((10.5, 10.0));
    let effects = world.release((10.5, 10.0));
    assert_eq!(world.position("a"), Point::ZERO);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::DragStop(_)))
    );
}

#[test]
fn dragging_the_pane_pans_the_viewport() {
    let mut world = World::new();
    world.press(Target::Pane, (400.0, 400.0), NONE);
    world.motion((450.0, 380.0));
    let effects = world.release((450.0, 380.0));
    assert_eq!(world.flow.viewport.x, 50.0);
    assert_eq!(world.flow.viewport.y, -20.0);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::PaneClick(_)))
    );
}

#[test]
fn clicking_the_pane_clears_the_selection() {
    let mut world = World::new();
    world.nodes[0].selected = true;
    world.press(Target::Pane, (600.0, 500.0), NONE);
    let effects = world.release((600.0, 500.0));
    assert!(world.selected().is_empty());
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::PaneClick(_)))
    );
}

#[test]
fn shift_dragging_the_pane_draws_a_selection_box() {
    let mut world = World::new();
    world.press(Target::Pane, (-20.0, -20.0), SHIFT);
    world.motion((350.0, 50.0));
    assert!(world.flow.selection_box.is_some());
    world.release((350.0, 50.0));
    assert_eq!(world.selected(), vec!["a", "b"]);
    assert!(world.flow.selection_box.is_none());
}

#[test]
fn a_selection_box_in_full_mode_needs_the_whole_node() {
    let mut world = World::new();
    world.flow.options.selection_mode = SelectionMode::Full;
    world.press(Target::Pane, (-20.0, -20.0), SHIFT);
    world.motion((350.0, 50.0));
    world.release((350.0, 50.0));
    assert!(world.selected().is_empty());
}

#[test]
fn dragging_from_an_output_to_an_input_connects_them() {
    let mut world = World::new();
    world.press(handle("a", "out", HandleKind::Source), (100.0, 30.0), NONE);
    world.motion((200.0, 30.0));
    world.motion((305.0, 32.0));
    let pending = world
        .flow
        .pending
        .clone()
        .expect("a connection in progress");
    assert!(pending.valid);
    let effects = world.release((305.0, 32.0));
    assert!(matches!(&effects[..], [Effect::Connect(connection)] if &*connection.target == "b"));
    assert_eq!(world.edges.len(), 1);
    assert!(world.flow.pending.is_none());
}

#[test]
fn dragging_from_an_input_back_to_an_output_connects_the_right_way_round() {
    let mut world = World::new();
    world.press(handle("b", "in", HandleKind::Target), (300.0, 30.0), NONE);
    world.motion((101.0, 29.0));
    world.release((101.0, 29.0));
    assert_eq!(&*world.edges[0].source, "a");
    assert_eq!(&*world.edges[0].target, "b");
}

#[test]
fn dropping_a_connection_on_empty_space_reports_where() {
    let mut world = World::new();
    world.press(handle("a", "out", HandleKind::Source), (100.0, 30.0), NONE);
    world.motion((200.0, 200.0));
    let effects = world.release((200.0, 200.0));
    assert!(
        matches!(&effects[..], [Effect::ConnectEnd { at, .. }] if *at == Point::new(200.0, 200.0))
    );
    assert!(world.edges.is_empty());
}

#[test]
fn an_invalid_connection_is_never_made() {
    let mut world = World::new();
    let nodes = world.nodes.clone();
    let refuse = |_: &Connection| false;
    let scene = Scene {
        nodes: &nodes,
        edges: &[] as &[Edge<()>],
        valid: &refuse,
    };
    world.flow.press(
        &scene,
        handle("a", "out", HandleKind::Source),
        Point::new(100.0, 30.0),
        Button::Primary,
        NONE,
    );
    world.flow.motion(&scene, Point::new(300.0, 30.0));
    assert!(
        world
            .flow
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.valid)
    );
    let effects = world.flow.release(&scene, Point::new(300.0, 30.0));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Connect(_)))
    );
}

#[test]
fn clicking_one_handle_then_another_connects_them() {
    let mut world = World::new();
    world.press(handle("a", "out", HandleKind::Source), (100.0, 30.0), NONE);
    world.release((100.0, 30.0));
    assert!(world.flow.pending.is_some());
    world.press(handle("b", "in", HandleKind::Target), (300.0, 30.0), NONE);
    world.release((300.0, 30.0));
    assert_eq!(world.edges.len(), 1);
}

#[test]
fn escape_abandons_a_connection_before_touching_the_selection() {
    let mut world = World::new();
    world.nodes[0].selected = true;
    world.press(handle("a", "out", HandleKind::Source), (100.0, 30.0), NONE);
    world.release((100.0, 30.0));
    world.run(|flow, scene| flow.key(scene, Key::Escape, NONE));
    assert!(world.flow.pending.is_none());
    assert_eq!(world.selected(), vec!["a"]);
    world.run(|flow, scene| flow.key(scene, Key::Escape, NONE));
    assert!(world.selected().is_empty());
}

#[test]
fn clicking_near_a_wire_selects_it() {
    let mut world = World::new();
    world.link("a", "b");
    let effects = world.press(Target::Pane, (200.0, 32.0), NONE);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::EdgeClick(_)))
    );
    assert!(world.edges[0].selected);
}

#[test]
fn delete_removes_the_selection_and_every_wire_on_it() {
    let mut world = World::new();
    world.link("a", "b");
    world.link("b", "c");
    world.link("c", "a");
    world.nodes[0].selected = true;
    world.edges[1].selected = true;
    let effects = world.run(|flow, scene| flow.key(scene, Key::Delete, NONE));
    let [Effect::Delete { nodes, edges }] = &effects[..] else {
        panic!("one deletion: {effects:?}");
    };
    assert_eq!(nodes, &vec![Id::from("a")]);
    assert_eq!(edges.len(), 3);
}

#[test]
fn a_node_that_cannot_be_deleted_survives_delete() {
    let mut world = World::new();
    world.nodes[0].selected = true;
    world.nodes[0].deletable = false;
    let effects = world.run(|flow, scene| flow.key(scene, Key::Delete, NONE));
    assert!(effects.is_empty());
}

#[test]
fn select_all_selects_every_node() {
    let mut world = World::new();
    world.run(|flow, scene| flow.key(scene, Key::SelectAll, NONE));
    assert_eq!(world.selected().len(), 3);
}

#[test]
fn arrow_keys_nudge_the_selection() {
    let mut world = World::new();
    world.nodes[2].selected = true;
    world.run(|flow, scene| flow.key(scene, Key::Right, NONE));
    world.run(|flow, scene| flow.key(scene, Key::Up, SHIFT));
    assert_eq!(world.position("c"), Point::new(10.0, 260.0));
}

#[test]
fn a_grip_resizes_its_node() {
    let mut world = World::new();
    world.press(
        Target::Grip("a".into(), Grip::BottomRight),
        (100.0, 60.0),
        NONE,
    );
    world.motion((180.0, 100.0));
    let effects = world.release((180.0, 100.0));
    assert_eq!(world.nodes[0].size, Size::new(180.0, 100.0));
    assert_eq!(effects, vec![Effect::ResizeStop("a".into())]);
}

#[test]
fn the_wheel_pans_and_the_command_wheel_zooms_about_the_pointer() {
    let mut world = World::new();
    world
        .flow
        .wheel(Point::new(100.0, 100.0), Vec2::new(0.0, 40.0), true, NONE);
    assert_eq!(world.flow.viewport.y, -40.0);
    let before = world.flow.viewport.to_flow(Point::new(100.0, 100.0));
    world.flow.wheel(
        Point::new(100.0, 100.0),
        Vec2::new(0.0, -100.0),
        true,
        Modifiers { ctrl: true, ..NONE },
    );
    assert!(world.flow.viewport.zoom > 1.0);
    let after = world.flow.viewport.to_flow(Point::new(100.0, 100.0));
    assert!((after - before).hypot() < 1e-9);
}

#[test]
fn fitting_shows_every_node() {
    let mut world = World::new();
    world.flow.fit(&world.nodes.clone());
    for node in &world.nodes {
        let drawn = world.flow.viewport.rect_to_screen(node.frame());
        assert!(drawn.x0 >= 0.0 && drawn.y0 >= 0.0);
        assert!(drawn.x1 <= 800.0 && drawn.y1 <= 600.0);
    }
    assert!(world.flow.viewport.zoom <= 1.0);
}

#[test]
fn a_right_click_opens_the_menu_for_what_is_under_it() {
    let mut world = World::new();
    world.link("a", "b");
    let on_node = world.run(|flow, scene| {
        flow.press(
            scene,
            node("a"),
            Point::new(5.0, 5.0),
            Button::Secondary,
            NONE,
        )
    });
    assert!(matches!(
        &on_node[..],
        [Effect::Menu {
            target: MenuTarget::Node(_),
            ..
        }]
    ));
    let on_wire = world.run(|flow, scene| {
        flow.press(
            scene,
            Target::Pane,
            Point::new(200.0, 30.0),
            Button::Secondary,
            NONE,
        )
    });
    assert!(matches!(
        &on_wire[..],
        [Effect::Menu {
            target: MenuTarget::Edge(_),
            ..
        }]
    ));
    let on_pane = world.run(|flow, scene| {
        flow.press(
            scene,
            Target::Pane,
            Point::new(600.0, 500.0),
            Button::Secondary,
            NONE,
        )
    });
    assert!(matches!(
        &on_pane[..],
        [Effect::Menu {
            target: MenuTarget::Pane,
            ..
        }]
    ));
}
