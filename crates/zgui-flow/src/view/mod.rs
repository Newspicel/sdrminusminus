mod background;
mod controls;
mod edges;
mod handle;
mod minimap;
mod node;
mod pane;
mod style;

use std::{cell::RefCell, rc::Rc, time::Duration};

use kurbo::{Point, Rect};
use zgui::prelude::*;

pub use background::{Background, Pattern, background};
pub use controls::controls;
pub use minimap::minimap;
pub use node::NodeCx;
pub use pane::flow_view;
pub use style::{FLOW_SHEET, FlowStyle};

use crate::{
    interaction::{Effect, Flow, Options},
    model::{Connection, Edge, Id, Node, apply_edge_changes, apply_node_changes},
    viewport::{Viewport, bounds_of, ease_in_out_cubic},
};

const ZOOM_STEP: f64 = 1.2;
const ANIMATION: Duration = Duration::from_millis(220);

type Valid = Rc<dyn Fn(&Connection) -> bool>;
type Listener = Rc<dyn Fn(&Effect)>;

pub struct FlowHandle<T: Send + Sync + 'static, E: Send + Sync + 'static> {
    pub nodes: RwSignal<Vec<Node<T>>>,
    pub edges: RwSignal<Vec<Edge<E>>>,
    pub state: RwSignal<Flow>,
    pub hovered_edge: RwSignal<Option<Id>>,
    pub clock: RwSignal<f64>,
    root: NodeRef,
    runtime: StoredValue<Runtime, LocalStorage>,
}

impl<T: Send + Sync + 'static, E: Send + Sync + 'static> Clone for FlowHandle<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static, E: Send + Sync + 'static> Copy for FlowHandle<T, E> {}

#[derive(Default)]
struct Runtime {
    timers: Option<Timers>,
    animation: Rc<RefCell<Option<zgui::view::time::FrameHandle>>>,
    valid: Option<Valid>,
    listener: Option<Listener>,
    claimed: Option<crate::interaction::Target>,
    ticking: Option<zgui::view::time::IntervalHandle>,
    fitted: bool,
}

impl<T: Send + Sync + 'static, E: Send + Sync + 'static> FlowHandle<T, E> {
    #[must_use]
    pub fn new(options: Options) -> Self {
        Self {
            nodes: RwSignal::new(Vec::new()),
            edges: RwSignal::new(Vec::new()),
            state: RwSignal::new(Flow::new(options)),
            hovered_edge: RwSignal::new(None),
            clock: RwSignal::new(0.0),
            root: NodeRef::new(),
            runtime: StoredValue::new_local(Runtime::default()),
        }
    }

    #[must_use]
    pub fn viewport(self) -> Viewport {
        self.state.with(|state| state.viewport)
    }

    #[must_use]
    pub fn selected(self) -> Vec<Id> {
        self.nodes.with_untracked(|nodes| {
            nodes
                .iter()
                .filter(|node| node.selected)
                .map(|node| node.id.clone())
                .collect()
        })
    }

    #[must_use]
    pub fn window_to_flow(self, x: f32, y: f32) -> Point {
        let local = self.local(x, y);
        self.state
            .with_untracked(|state| state.viewport.to_flow(local))
    }

    #[must_use]
    pub fn flow_to_window(self, flow: Point) -> Point {
        let local = self
            .state
            .with_untracked(|state| state.viewport.to_screen(flow));
        let origin = self.origin();
        Point::new(local.x + origin.x, local.y + origin.y)
    }

    #[must_use]
    pub fn pane_to_window(self, local: Point) -> Point {
        let origin = self.origin();
        Point::new(local.x + origin.x, local.y + origin.y)
    }

    #[must_use]
    pub fn pane_to_flow(self, local: Point) -> Point {
        self.state
            .with_untracked(|state| state.viewport.to_flow(local))
    }

    #[must_use]
    pub fn is_pane(self, node: NodeId) -> bool {
        self.root.get_untracked() == Some(node)
    }

    #[must_use]
    pub fn visible_centre(self) -> Point {
        self.state
            .with_untracked(|state| state.viewport.visible(state.screen).center())
    }

    fn origin(self) -> Point {
        let scale = f64::from(self.root.scale().max(0.01));
        self.root.window_bounds().map_or(Point::ZERO, |bounds| {
            Point::new(
                f64::from(bounds.origin.x.0) / scale,
                f64::from(bounds.origin.y.0) / scale,
            )
        })
    }

    fn local(self, x: f32, y: f32) -> Point {
        let origin = self.origin();
        Point::new(f64::from(x) - origin.x, f64::from(y) - origin.y)
    }

    pub fn set_viewport(self, target: Viewport, animate: bool) {
        self.stop_animation();
        if !animate {
            self.state.update(|state| state.viewport = target);
            return;
        }
        let start = self.viewport_untracked();
        let Some(timers) = self.runtime.with_value(|runtime| runtime.timers.clone()) else {
            self.state.update(|state| state.viewport = target);
            return;
        };
        let slot = self.runtime.with_value(|runtime| runtime.animation.clone());
        animate_frame(self, timers, slot, start, target, None);
    }

    fn viewport_untracked(self) -> Viewport {
        self.state.with_untracked(|state| state.viewport)
    }

    fn stop_animation(self) {
        self.runtime.with_value(|runtime| {
            runtime.animation.borrow_mut().take();
        });
    }

    pub fn fit_view(self, animate: bool) {
        let bounds = self.nodes.with_untracked(|nodes| {
            bounds_of(nodes.iter().filter(|node| !node.hidden).map(Node::frame))
        });
        if let Some(bounds) = bounds {
            self.fit_bounds(bounds, animate);
        }
    }

    pub fn fit_bounds(self, bounds: Rect, animate: bool) {
        let target = self.state.with_untracked(|state| state.fitted(bounds));
        self.set_viewport(target, animate);
    }

    pub fn zoom_in(self) {
        self.zoom_by(ZOOM_STEP);
    }

    pub fn zoom_out(self) {
        self.zoom_by(1.0 / ZOOM_STEP);
    }

    fn zoom_by(self, factor: f64) {
        let target = self.state.with_untracked(|state| {
            let centre = Point::new(state.screen.width / 2.0, state.screen.height / 2.0);
            state.viewport.scaled_at(centre, factor, state.options.zoom)
        });
        self.set_viewport(target, true);
    }

    pub fn centre_on(self, flow: Point, animate: bool) {
        let target = self
            .state
            .with_untracked(|state| state.viewport.centred_on(flow, state.screen));
        self.set_viewport(target, animate);
    }

    pub fn select_only(self, ids: &[Id]) {
        self.nodes.update(|nodes| {
            for node in nodes.iter_mut() {
                node.selected = ids.contains(&node.id);
            }
        });
        self.edges.update(|edges| {
            for edge in edges.iter_mut() {
                edge.selected = false;
            }
        });
    }

    fn claim(self, target: crate::interaction::Target) {
        self.runtime.update_value(|runtime| {
            if runtime.claimed.is_none() {
                runtime.claimed = Some(target);
            }
        });
    }

    fn claimed(self) -> crate::interaction::Target {
        let mut taken = None;
        self.runtime
            .update_value(|runtime| taken = runtime.claimed.take());
        taken.unwrap_or(crate::interaction::Target::Pane)
    }

    pub fn cancel(self) {
        self.state.update(Flow::cancel);
    }

    fn valid(self) -> Valid {
        self.runtime
            .with_value(|runtime| runtime.valid.clone())
            .unwrap_or_else(|| Rc::new(|_: &Connection| true))
    }

    fn emit(self, effects: Vec<Effect>) {
        if effects.is_empty() {
            return;
        }
        for effect in &effects {
            match effect {
                Effect::Nodes(changes) => {
                    self.nodes
                        .update(|nodes| apply_node_changes(nodes, changes));
                }
                Effect::Edges(changes) => {
                    self.edges
                        .update(|edges| apply_edge_changes(edges, changes));
                }
                _ => {}
            }
        }
        if let Some(listener) = self.runtime.with_value(|runtime| runtime.listener.clone()) {
            for effect in &effects {
                listener(effect);
            }
        }
    }
}

fn animate_frame<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    timers: Timers,
    slot: Rc<RefCell<Option<zgui::view::time::FrameHandle>>>,
    start: Viewport,
    target: Viewport,
    began: Option<Duration>,
) {
    let again = slot.clone();
    let clock = timers.clone();
    let handle = timers.request_frame(move |now| {
        let now = now.since_origin();
        let began = began.unwrap_or(now);
        let progress =
            (now.saturating_sub(began).as_secs_f64() / ANIMATION.as_secs_f64()).clamp(0.0, 1.0);
        let at = start.lerp(target, ease_in_out_cubic(progress));
        flow.state.update(|state| state.viewport = at);
        if progress < 1.0 {
            animate_frame(flow, clock, again, start, target, Some(began));
        }
    });
    *slot.borrow_mut() = Some(handle);
}
