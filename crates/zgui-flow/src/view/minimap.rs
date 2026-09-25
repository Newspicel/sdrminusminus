use kurbo::{Point, Rect, Shape, Size};
use zgui::{
    canvas::{Brush, ShapeBuilder},
    prelude::*,
};

use super::{FlowHandle, FlowStyle, edges::colour};
use crate::{model::Node, viewport::bounds_of};

const MARGIN: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    world: Rect,
    scale: f64,
    offset: Point,
}

impl Fit {
    fn new(world: Rect, canvas: Size) -> Option<Self> {
        if world.width() <= 0.0
            || world.height() <= 0.0
            || canvas.width <= 0.0
            || canvas.height <= 0.0
        {
            return None;
        }
        let scale = (canvas.width / world.width()).min(canvas.height / world.height());
        let offset = Point::new(
            (canvas.width - world.width() * scale) / 2.0,
            (canvas.height - world.height() * scale) / 2.0,
        );
        Some(Self {
            world,
            scale,
            offset,
        })
    }

    fn to_map(self, flow: Rect) -> Rect {
        Rect::new(
            (flow.x0 - self.world.x0) * self.scale + self.offset.x,
            (flow.y0 - self.world.y0) * self.scale + self.offset.y,
            (flow.x1 - self.world.x0) * self.scale + self.offset.x,
            (flow.y1 - self.world.y0) * self.scale + self.offset.y,
        )
    }

    fn to_flow(self, map: Point) -> Point {
        Point::new(
            (map.x - self.offset.x) / self.scale + self.world.x0,
            (map.y - self.offset.y) / self.scale + self.world.y0,
        )
    }
}

fn world<T>(nodes: &[Node<T>], visible: Rect) -> Rect {
    let joined = bounds_of(nodes.iter().filter(|node| !node.hidden).map(Node::frame))
        .map_or(visible, |bounds| bounds.union(visible));
    let pad = joined.width().max(joined.height()) * MARGIN;
    joined.inflate(pad, pad)
}

pub fn minimap<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    style: FlowStyle,
) -> impl IntoView {
    let own = NodeRef::new();
    let size = RwSignal::new(Size::ZERO);
    let fit = move || {
        let (visible, _) = flow
            .state
            .with(|state| (state.viewport.visible(state.screen), state.screen));
        flow.nodes
            .with(|nodes| Fit::new(world(nodes, visible), size.get()))
    };
    let painter = zgui::elements::canvas()
        .class("flow__minimap-canvas")
        .draw(move |cx| {
            let canvas = Size::new(f64::from(cx.size.width.0), f64::from(cx.size.height.0));
            if size.get_untracked() != canvas {
                size.set(canvas);
            }
            let visible = flow
                .state
                .with(|state| state.viewport.visible(state.screen));
            flow.nodes.with(|nodes| {
                let Some(fit) = Fit::new(world(nodes, visible), canvas) else {
                    return;
                };
                let mut shapes = kurbo::BezPath::new();
                for node in nodes.iter().filter(|node| !node.hidden) {
                    shapes.extend(fit.to_map(node.frame()).to_rounded_rect(2.0).to_path(0.1));
                }
                cx.scene.push(
                    ShapeBuilder::new(shapes)
                        .fill(Brush::Solid(colour(style.minimap_node)))
                        .build(),
                );
                let mut mask = canvas.to_rect().to_path(0.1);
                mask.extend(fit.to_map(visible).to_path(0.1));
                cx.scene.push(
                    ShapeBuilder::new(mask)
                        .fill_even_odd(Brush::Solid(colour(style.minimap_mask)))
                        .build(),
                );
            });
        });
    let steer = move |x: f32, y: f32| {
        let Some(fit) = fit() else {
            return;
        };
        let Some(bounds) = own.window_bounds() else {
            return;
        };
        let scale = f64::from(own.scale().max(0.01));
        let local = Point::new(
            f64::from(x) - f64::from(bounds.origin.x.0) / scale,
            f64::from(y) - f64::from(bounds.origin.y.0) / scale,
        );
        flow.centre_on(fit.to_flow(local), false);
    };
    let held = RwSignal::new(false);
    view! {
        box(
            node_ref = own,
            class = "flow__minimap",
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                if ev.button != Some(PointerButton::Primary) {
                    return;
                }
                ev.stop_propagation();
                ev.capture_pointer();
                held.set(true);
                steer(ev.position.x.0, ev.position.y.0);
            },
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| {
                if held.get_untracked() {
                    ev.stop_propagation();
                    steer(ev.position.x.0, ev.position.y.0);
                }
            },
            on:pointer_up = move |ev: &mut EventCx<'_, events::PointerUp>| {
                ev.release_pointer();
                if held.get_untracked() {
                    ev.stop_propagation();
                    held.set(false);
                }
            },
            on:wheel = move |ev: &mut EventCx<'_, events::Wheel>| {
                ev.stop_propagation();
                ev.prevent_default();
                let y = match ev.delta {
                    ScrollDelta::Lines { y, .. } => f64::from(y) * 20.0,
                    ScrollDelta::Pixels(size) => f64::from(size.height.0),
                    _ => return,
                };
                flow.state.update(|state| {
                    let centre = Point::new(state.screen.width / 2.0, state.screen.height / 2.0);
                    state.viewport = state.viewport.scaled_at(centre, 2f64.powf(-y * 0.004), state.options.zoom);
                });
            }
        ) {
            {painter.into_view()}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_and_the_flow_agree_on_a_point() {
        let fit = Fit::new(
            Rect::new(-100.0, 0.0, 300.0, 200.0),
            Size::new(200.0, 140.0),
        )
        .expect("a fit");
        let flow = Point::new(120.0, 80.0);
        let map = fit
            .to_map(Rect::from_origin_size(flow, (0.0, 0.0)))
            .origin();
        assert!((fit.to_flow(map) - flow).hypot() < 1e-9);
    }

    #[test]
    fn the_world_fits_inside_the_map_and_is_centred() {
        let fit =
            Fit::new(Rect::new(0.0, 0.0, 400.0, 100.0), Size::new(200.0, 140.0)).expect("a fit");
        let drawn = fit.to_map(Rect::new(0.0, 0.0, 400.0, 100.0));
        assert_eq!(drawn.width(), 200.0);
        assert!((drawn.center().y - 70.0).abs() < 1e-9);
    }

    #[test]
    fn nothing_to_draw_gives_no_fit() {
        assert!(Fit::new(Rect::ZERO, Size::new(200.0, 140.0)).is_none());
    }
}
