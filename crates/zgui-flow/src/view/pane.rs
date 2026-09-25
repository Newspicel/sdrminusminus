use std::{cell::RefCell, rc::Rc, time::Duration};

use kurbo::{Point, Size, Vec2};
use zgui::prelude::*;

use super::{
    FlowHandle, FlowStyle, Listener, Valid,
    background::background,
    edges::{Layer, edge_labels, edge_layer},
    node::{NodeCx, node_view},
    style::FLOW_SHEET,
};
use crate::{
    drag::auto_pan,
    interaction::{Button, Effect, Key as FlowKey, Modifiers, Scene, Target},
    model::{Connection, Node},
};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_REACH: f64 = 6.0;
const AUTO_PAN_SPEED: f64 = 12.0;
const AUTO_PAN_MARGIN: f64 = 36.0;
const FRAME: Duration = Duration::from_millis(16);

pub(super) fn modifiers(state: zgui::prelude::Modifiers) -> Modifiers {
    Modifiers {
        shift: state.shift(),
        ctrl: state.control(),
        alt: state.alt(),
        meta: state.meta(),
    }
}

pub(super) fn button(pressed: Option<PointerButton>) -> Option<Button> {
    match pressed {
        Some(PointerButton::Primary) | None => Some(Button::Primary),
        Some(PointerButton::Secondary) => Some(Button::Secondary),
        Some(PointerButton::Middle) => Some(Button::Middle),
        _ => None,
    }
}

impl<T: Send + Sync + 'static, E: Send + Sync + 'static> FlowHandle<T, E> {
    pub(super) fn press(self, target: Target, x: f32, y: f32, pressed: Button, held: Modifiers) {
        let at = self.local(x, y);
        self.stop_animation();
        let valid = self.valid();
        let effects = self.nodes.with_untracked(|nodes| {
            self.edges.with_untracked(|edges| {
                let scene = Scene {
                    nodes,
                    edges,
                    valid: &*valid,
                };
                let mut out = Vec::new();
                self.state
                    .update(|state| out = state.press(&scene, target, at, pressed, held));
                out
            })
        });
        self.emit(effects);
    }

    pub(super) fn motion_at(self, at: Point) {
        let valid = self.valid();
        let effects = self.nodes.with_untracked(|nodes| {
            self.edges.with_untracked(|edges| {
                let scene = Scene {
                    nodes,
                    edges,
                    valid: &*valid,
                };
                let idle = self
                    .state
                    .with_untracked(|state| !state.busy() && state.pending.is_none());
                if idle {
                    let hit = self.state.with_untracked(|state| {
                        scene.edge_at(
                            state.viewport.to_flow(at),
                            state.options.edge_shape,
                            state.options.edge_reach / state.viewport.zoom,
                        )
                    });
                    if self.hovered_edge.get_untracked() != hit {
                        self.hovered_edge.set(hit);
                    }
                    return Vec::new();
                }
                let mut out = Vec::new();
                self.state.update(|state| out = state.motion(&scene, at));
                out
            })
        });
        self.emit(effects);
    }

    pub(super) fn release_at(self, at: Point) -> Vec<Effect> {
        let valid = self.valid();
        self.nodes.with_untracked(|nodes| {
            self.edges.with_untracked(|edges| {
                let scene = Scene {
                    nodes,
                    edges,
                    valid: &*valid,
                };
                let mut out = Vec::new();
                self.state.update(|state| out = state.release(&scene, at));
                out
            })
        })
    }

    fn key(self, key: FlowKey, held: Modifiers) -> bool {
        let valid = self.valid();
        let effects = self.nodes.with_untracked(|nodes| {
            self.edges.with_untracked(|edges| {
                let scene = Scene {
                    nodes,
                    edges,
                    valid: &*valid,
                };
                let mut out = Vec::new();
                self.state
                    .update(|state| out = state.key(&scene, key, held));
                out
            })
        });
        let handled = !effects.is_empty();
        self.emit(effects);
        handled
    }
}

fn flow_key(key: &Key, held: Modifiers) -> Option<FlowKey> {
    match key {
        Key::Named(NamedKey::Delete | NamedKey::Backspace) => Some(FlowKey::Delete),
        Key::Named(NamedKey::Escape) => Some(FlowKey::Escape),
        Key::Named(NamedKey::ArrowLeft) => Some(FlowKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(FlowKey::Right),
        Key::Named(NamedKey::ArrowUp) => Some(FlowKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(FlowKey::Down),
        Key::Character(text) if held.command() && text.eq_ignore_ascii_case("a") => {
            Some(FlowKey::SelectAll)
        }
        _ => None,
    }
}

#[derive(Default)]
struct Clicks {
    last: Option<(Duration, Point)>,
}

impl Clicks {
    fn double(&mut self, now: Duration, at: Point) -> bool {
        let twice = self.last.is_some_and(|(then, where_)| {
            now.saturating_sub(then) <= DOUBLE_CLICK && (at - where_).hypot() <= DOUBLE_CLICK_REACH
        });
        self.last = if twice { None } else { Some((now, at)) };
        twice
    }
}

struct Pump {
    frame: Option<zgui::view::time::IntervalHandle>,
    pointer: Point,
}

fn pump<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    timers: &Timers,
    held: Rc<RefCell<Pump>>,
) {
    let again = held.clone();
    let ticking = timers.set_interval(FRAME, move || {
        let pointer = again.borrow().pointer;
        let (screen, moving) = flow.state.with_untracked(|state| {
            (
                state.screen,
                state.busy()
                    && (state.dragging()
                        || state.selection_box.is_some()
                        || state.pending.is_some()),
            )
        });
        if !moving {
            again.borrow_mut().frame = None;
            return;
        }
        let shift = auto_pan(pointer, screen, AUTO_PAN_SPEED, AUTO_PAN_MARGIN);
        if shift != Vec2::ZERO {
            flow.state
                .update(|state| state.viewport = state.viewport.panned(shift));
            flow.motion_at(pointer);
        }
    });
    held.borrow_mut().frame = Some(ticking);
}

pub fn flow_view<T, E, V, O>(
    flow: FlowHandle<T, E>,
    style: FlowStyle,
    valid: impl Fn(&Connection) -> bool + 'static,
    on_effect: impl Fn(&Effect) + 'static,
    render: impl Fn(NodeCx<T, E>) -> V + 'static,
    overlay: O,
) -> impl IntoView
where
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
    V: IntoView + 'static,
    O: IntoView + 'static,
{
    install_stylesheet("zgui-flow", FLOW_SHEET);
    let timers = Timers::current();
    flow.runtime.update_value(|runtime| {
        runtime.timers.clone_from(&timers);
        runtime.valid = Some(Rc::new(valid) as Valid);
        runtime.listener = Some(Rc::new(on_effect) as Listener);
    });
    let root = flow.root;
    let border = root.observe_border_box();
    let sizing = zgui::reactive::RenderEffect::new(move |_| {
        if let Some(bounds) = border.get() {
            let scale = f64::from(root.scale().max(0.01));
            let screen = Size::new(
                f64::from(bounds.size.width.0) / scale,
                f64::from(bounds.size.height.0) / scale,
            );
            if flow.state.with_untracked(|state| state.screen) != screen {
                flow.state.update(|state| state.screen = screen);
                fit_once(flow);
            }
        }
    });
    on_cleanup_local(move || drop(sizing));
    let animating = zgui::reactive::RenderEffect::new(move |_| {
        let moving = flow
            .edges
            .with(|edges| edges.iter().any(|edge| edge.animated));
        if !moving {
            flow.runtime.update_value(|runtime| runtime.ticking = None);
            return;
        }
        let idle = flow.runtime.with_value(|runtime| runtime.ticking.is_none());
        if let (true, Some(timers)) = (
            idle,
            flow.runtime.with_value(|runtime| runtime.timers.clone()),
        ) {
            let started = std::time::Instant::now();
            let ticking = timers.set_interval(FRAME, move || {
                flow.clock.set(started.elapsed().as_secs_f64());
            });
            flow.runtime
                .update_value(|runtime| runtime.ticking = Some(ticking));
        }
    });
    on_cleanup_local(move || drop(animating));

    let clicks = Rc::new(RefCell::new(Clicks::default()));
    let held = Rc::new(RefCell::new(Pump {
        frame: None,
        pointer: Point::ZERO,
    }));
    let render = Rc::new(render);

    let down = {
        let held = held.clone();
        let timers = timers.clone();
        move |ev: &mut EventCx<'_, events::PointerDown>| {
            let target = flow.claimed();
            let Some(pressed) = button(ev.button) else {
                return;
            };
            ev.capture_pointer();
            flow.press(
                target,
                ev.position.x.0,
                ev.position.y.0,
                pressed,
                modifiers(ev.modifiers),
            );
            start_pump(
                flow,
                timers.clone(),
                &held,
                ev.position.x.0,
                ev.position.y.0,
            );
        }
    };
    let motion = {
        let held = held.clone();
        move |ev: &mut EventCx<'_, events::PointerMove>| {
            let at = flow.local(ev.position.x.0, ev.position.y.0);
            held.borrow_mut().pointer = at;
            flow.motion_at(at);
        }
    };
    let up = {
        let clicks = clicks.clone();
        move |ev: &mut EventCx<'_, events::PointerUp>| {
            ev.release_pointer();
            let at = flow.local(ev.position.x.0, ev.position.y.0);
            let mut effects = flow.release_at(at);
            let now = ev.timestamp.since_origin();
            let doubled: Vec<Effect> = effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::PaneClick(point) if clicks.borrow_mut().double(now, at) => {
                        Some(Effect::PaneDoubleClick(*point))
                    }
                    _ => None,
                })
                .collect();
            effects.extend(doubled);
            flow.emit(effects);
        }
    };
    let wheel = move |ev: &mut EventCx<'_, events::Wheel>| {
        let (delta, pixels) = match ev.delta {
            ScrollDelta::Lines { x, y } => (Vec2::new(f64::from(x), f64::from(y)), false),
            ScrollDelta::Pixels(size) => (
                Vec2::new(f64::from(size.width.0), f64::from(size.height.0)),
                true,
            ),
            _ => return,
        };
        ev.prevent_default();
        flow.stop_animation();
        let at = flow.local(ev.position.x.0, ev.position.y.0);
        let held = modifiers(ev.modifiers);
        flow.state
            .update(|state| state.wheel(at, delta, pixels, held));
    };
    let keys = move |ev: &mut EventCx<'_, events::KeyDown>| {
        if root.get_untracked() != Some(ev.target) {
            return;
        }
        let held = modifiers(ev.modifiers);
        if let Some(key) = flow_key(&ev.key, held)
            && flow.key(key, held)
        {
            ev.prevent_default();
        }
    };

    let dragging = move || flow.state.with(|state| state.dragging());
    let panning = move || flow.state.with(|state| state.panning());
    let connecting = move || flow.state.with(|state| state.pending.is_some());
    let over_edge = move || flow.hovered_edge.with(Option::is_some);
    let world = move || {
        let viewport = flow.state.with(|state| state.viewport);
        Some(format!(
            "translate({:.3}px, {:.3}px) scale({:.5})",
            viewport.x, viewport.y, viewport.zoom
        ))
    };
    let marquee = move || flow.state.with(|state| state.selection_box);
    let still = edge_layer(flow, style, Layer::Still);
    let live = edge_layer(flow, style, Layer::Live);
    let pattern = style
        .background
        .map(|look| AnyView::new(background(flow, look)));
    let cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
        ev.release_pointer();
        flow.cancel();
    };

    view! {
        box(
            node_ref = root,
            class = "flow",
            tabindex = Focus::Programmatic,
            class:dragging = dragging,
            class:panning = panning,
            class:connecting = connecting,
            class:over-edge = over_edge,
            on:pointer_down = down,
            on:pointer_move = motion,
            on:pointer_up = up,
            on:pointer_cancel = cancel,
            on:wheel = wheel,
            on:key_down = keys
        ) {
            {pattern}
            {still}
            {live}
            box(class = "flow__world", style:transform = world) {
                {edge_labels(flow)}
                for key in move || keyed(flow), key = |key: &String| key.clone() {
                    {node_view(flow, key, render.clone())}
                }
            }
            {move || marquee().map(|area| AnyView::new(view! {
                box(
                    class = "flow__selection",
                    style:left = Some(format!("{:.1}px", area.x0)),
                    style:top = Some(format!("{:.1}px", area.y0)),
                    style:width = Some(format!("{:.1}px", area.width())),
                    style:height = Some(format!("{:.1}px", area.height()))
                )
            }))}
            {overlay}
        }
    }
}

fn fit_once<T: Send + Sync + 'static, E: Send + Sync + 'static>(flow: FlowHandle<T, E>) {
    let wanted = flow
        .state
        .with_untracked(|state| state.options.fit_on_start);
    let first = flow.runtime.with_value(|runtime| !runtime.fitted);
    if !wanted || !first {
        return;
    }
    flow.runtime.update_value(|runtime| runtime.fitted = true);
    let Some(timers) = flow.runtime.with_value(|runtime| runtime.timers.clone()) else {
        flow.fit_view(false);
        return;
    };
    let slot = flow.runtime.with_value(|runtime| runtime.animation.clone());
    let handle = timers.request_frame(move |_| flow.fit_view(false));
    *slot.borrow_mut() = Some(handle);
}

fn start_pump<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    timers: Option<Timers>,
    held: &Rc<RefCell<Pump>>,
    x: f32,
    y: f32,
) {
    held.borrow_mut().pointer = flow.local(x, y);
    let idle = held.borrow().frame.is_none();
    if let (true, Some(timers)) = (idle, timers) {
        pump(flow, &timers, held.clone());
    }
}

fn keyed<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
) -> Vec<String> {
    flow.nodes.with(|nodes| {
        nodes
            .iter()
            .filter(|node| !node.hidden)
            .map(Node::key)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_quick_clicks_on_one_spot_make_a_double_click() {
        let mut clicks = Clicks::default();
        let at = Point::new(10.0, 10.0);
        assert!(!clicks.double(Duration::from_millis(0), at));
        assert!(clicks.double(Duration::from_millis(250), Point::new(12.0, 11.0)));
        assert!(!clicks.double(Duration::from_millis(300), at));
    }

    #[test]
    fn slow_or_distant_clicks_stay_single() {
        let mut clicks = Clicks::default();
        assert!(!clicks.double(Duration::from_millis(0), Point::ZERO));
        assert!(!clicks.double(Duration::from_millis(900), Point::ZERO));
        assert!(!clicks.double(Duration::from_millis(1000), Point::new(50.0, 0.0)));
    }

    #[test]
    fn shortcut_keys_map_to_flow_keys() {
        let none = Modifiers::default();
        let command = Modifiers { meta: true, ..none };
        assert_eq!(
            flow_key(&Key::Named(NamedKey::Backspace), none),
            Some(FlowKey::Delete)
        );
        assert_eq!(
            flow_key(&Key::Character("a".into()), command),
            Some(FlowKey::SelectAll)
        );
        assert_eq!(flow_key(&Key::Character("a".into()), none), None);
    }
}
