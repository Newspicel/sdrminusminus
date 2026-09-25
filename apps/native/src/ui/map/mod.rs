pub mod geo;
pub mod heat;
pub mod mvt;
pub mod overlay;
pub mod paint;
pub mod pmtiles;
pub mod source;
mod tiles;

use std::sync::Arc;

use zgui::reactive::RenderEffect;
use zgui::{
    geom::{Css, CssPx},
    prelude::*,
};

use crate::store::Store;

pub use self::{
    geo::{Geo, View},
    overlay::Overlay,
};
use self::{source::Kind, tiles::Shared};

pub const ACCENT: u32 = 0x76_ac_fc;
const DRAG_THRESHOLD_PX: f64 = 3.0;
const WHEEL_LINE_PX: f64 = 40.0;
const WHEEL_ZOOM_PER_PX: f64 = 1.0 / 300.0;
const LABELS_SHOWN: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub points: Vec<Geo>,
    pub max_zoom: f64,
}

#[derive(Clone, Copy)]
pub struct MapProps {
    pub overlay: Signal<Arc<Overlay>>,
    pub active: Signal<bool>,
    pub frame: Signal<Option<Arc<Frame>>>,
    pub on_pick: Option<UnsyncCallback<Option<String>>>,
}

#[derive(Clone, Copy)]
struct Drag {
    x: f64,
    y: f64,
    start: View,
    moved: bool,
}

const SHEET: &str = css!(
    r#"
.map {
    position: relative;
    overflow: hidden;
    flex: 1 1 auto;
    min-height: 240px;
    background-color: #1a1c20;
    cursor: grab;
}
.map.dragging, .map.dragging * { cursor: grabbing; }
.map__layer {
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
}
.map__tile {
    position: absolute;
    left: 0;
    top: 0;
    width: 512px;
    height: 512px;
    transform-origin: 0 0;
    pointer-events: none;
}
.map__marks { position: absolute; left: 0; top: 0; width: 100%; height: 100%; pointer-events: none; }
.map__place, .map__label {
    position: absolute;
    white-space: nowrap;
    pointer-events: none;
    font-size: 10px;
    line-height: 1.2;
}
.map__place { color: #8a909b; transform: translate(-50%, -50%); }
.map__place[data-class="country"] { color: #6f7580; letter-spacing: 0.12em; text-transform: uppercase; font-size: 9px; }
.map__place[data-class="state"] { color: #5f656f; font-size: 9px; }
.map__place[data-class="city"] { color: #b7bcc5; font-size: 11px; }
.map__label { font-family: var(--mono); transform: translate(-50%, 0); }
.map__zoom {
    position: absolute;
    top: 8px;
    right: 8px;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 6px;
    background-color: var(--panel);
    overflow: hidden;
}
.map__zoom-step {
    width: 24px;
    height: 24px;
    align-items: center;
    justify-content: center;
    display: flex;
    color: var(--ink-dim);
    font-family: var(--mono);
    font-size: 14px;
    cursor: pointer;
}
.map__zoom-step:hover { background-color: var(--panel-3); color: var(--ink); }
.map__credit {
    position: absolute;
    right: 4px;
    bottom: 2px;
    font-size: 9px;
    color: var(--ink-faint);
    pointer-events: none;
}
.map__badge {
    position: absolute;
    left: 8px;
    bottom: 8px;
    padding: 1px 6px;
    border: 1px solid var(--line);
    border-radius: 4px;
    background-color: var(--bg);
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-dim);
    pointer-events: none;
}
"#
);

pub fn map(store: Store, props: MapProps, chrome: AnyView) -> impl IntoView {
    install_stylesheet("map", SHEET);
    let shared = Shared::get();
    shared.discover(store);
    let root = NodeRef::new();
    let view = RwSignal::new(View::default());
    let drag = StoredValue::new(None::<Drag>);
    let dragging = RwSignal::new(false);
    let framed = StoredValue::new(false);

    let size = root.observe_content_size();
    let sizing = RenderEffect::new(move |_| {
        let measured = size.get();
        let scale = f64::from(root.scale()).max(0.01);
        let (width, height) = (
            f64::from(measured.width.0) / scale,
            f64::from(measured.height.0) / scale,
        );
        if (width, height) != view.with_untracked(|view| (view.width, view.height)) {
            view.update(|view| {
                view.width = width;
                view.height = height;
            });
        }
    });
    on_cleanup_local(move || drop(sizing));

    let framing = RenderEffect::new(move |_| {
        let wanted = props.frame.get();
        let ready = view.with(|view| view.width > 0.0 && view.height > 0.0);
        if framed.get_value() || !ready {
            return;
        }
        if let Some(frame) = wanted.filter(|frame| !frame.points.is_empty()) {
            framed.set_value(true);
            view.update(|view| *view = view.fitted(&frame.points, 56.0, frame.max_zoom));
        }
    });
    on_cleanup_local(move || drop(framing));

    let local = move |at: zgui::geom::Point<CssPx, Css>| -> Option<(f64, f64)> {
        let bounds = root.window_bounds()?;
        let scale = f64::from(root.scale()).max(0.01);
        let current = view.get_untracked();
        let shown = f64::from(bounds.width().0) / scale;
        let squeeze = if current.width > 0.0 && shown > 0.0 {
            shown / current.width
        } else {
            1.0
        };
        Some((
            (f64::from(at.x.0) - f64::from(bounds.left().0) / scale) / squeeze,
            (f64::from(at.y.0) - f64::from(bounds.top().0) / scale) / squeeze,
        ))
    };

    let down = move |ev: &mut EventCx<'_, events::PointerDown>| {
        if !props.active.get_untracked() || ev.button != Some(PointerButton::Primary) {
            return;
        }
        let Some((x, y)) = local(ev.position) else {
            return;
        };
        ev.stop_propagation();
        ev.capture_pointer();
        drag.set_value(Some(Drag {
            x,
            y,
            start: view.get_untracked(),
            moved: false,
        }));
    };
    let moving = move |ev: &mut EventCx<'_, events::PointerMove>| {
        let (Some(mut held), Some((x, y))) = (drag.get_value(), local(ev.position)) else {
            return;
        };
        let (dx, dy) = (x - held.x, y - held.y);
        if !held.moved && dx.hypot(dy) < DRAG_THRESHOLD_PX {
            return;
        }
        if !held.moved {
            held.moved = true;
            drag.set_value(Some(held));
            dragging.set(true);
            framed.set_value(true);
        }
        view.set(held.start.panned(dx, dy));
    };
    let up = move |ev: &mut EventCx<'_, events::PointerUp>| {
        let Some(held) = drag.get_value() else {
            return;
        };
        ev.release_pointer();
        drag.set_value(None);
        dragging.set(false);
        if held.moved {
            return;
        }
        if let (Some(pick), Some((x, y))) = (props.on_pick, local(ev.position)) {
            let hit = overlay::pick(&props.overlay.get_untracked(), &view.get_untracked(), x, y);
            pick.run(hit);
        }
    };
    let wheel = move |ev: &mut EventCx<'_, events::Wheel>| {
        if !props.active.get_untracked() {
            return;
        }
        let pixels = match ev.delta {
            ScrollDelta::Lines { y, .. } => f64::from(y) * WHEEL_LINE_PX,
            ScrollDelta::Pixels(size) => f64::from(size.height.0),
            _ => 0.0,
        };
        let Some((x, y)) = local(ev.position) else {
            return;
        };
        ev.prevent_default();
        ev.stop_propagation();
        framed.set_value(true);
        view.update(|current| {
            *current = current.zoomed_at(x, y, current.zoom - pixels * WHEEL_ZOOM_PER_PX)
        });
    };

    let step = move |by: f64| {
        framed.set_value(true);
        view.update(|current| {
            *current = current.zoomed_at(
                current.width / 2.0,
                current.height / 2.0,
                (current.zoom + by).round(),
            );
        });
    };

    let marks = zgui::elements::canvas()
        .class("map__marks")
        .draw(move |cx| {
            let mut shown = view.get();
            shown.width = f64::from(cx.size.width.0);
            shown.height = f64::from(cx.size.height.0);
            if shown.width <= 1.0 || shown.height <= 1.0 {
                return;
            }
            overlay::draw(cx.scene, &shown, &props.overlay.get());
        })
        .into_view();

    let credit = {
        let shared = shared.clone();
        move || shared.credit()
    };
    view! {
        box(
            node_ref = root,
            class = "map",
            class:dragging = dragging,
            on:pointer_down = down,
            on:pointer_move = moving,
            on:pointer_up = up,
            on:pointer_cancel = move |_| {
                drag.set_value(None);
                dragging.set(false);
            },
            on:wheel = wheel
        ) {
            {tiles::layer(shared.clone(), view)}
            {marks}
            {labels(props.overlay, view)}
            {chrome}
            column(class = "map__zoom") {
                control(
                    class = "map__zoom-step",
                    a11y:label = "Zoom in",
                    on:pointer_down:stop = |_| {},
                    on:click:stop = move |_| step(1.0)
                ) {"+"}
                control(
                    class = "map__zoom-step",
                    a11y:label = "Zoom out",
                    on:pointer_down:stop = |_| {},
                    on:click:stop = move |_| step(-1.0)
                ) {"\u{2212}"}
            }
            {badge(shared.clone())}
            text(class = "map__credit") {{credit}}
        }
    }
}

fn badge(shared: std::rc::Rc<Shared>) -> impl IntoView {
    move || {
        let text = match shared.kind() {
            Kind::Offline => "offline basemap",
            Kind::Blank => "no basemap (offline)",
            Kind::Pending | Kind::Online if shared.failures() > 0 => "some tiles failed",
            Kind::Pending | Kind::Online => return None,
        };
        Some(view! { text(class = "map__badge") {{text}} })
    }
}

fn labels(overlay: Signal<Arc<Overlay>>, view: RwSignal<View>) -> impl IntoView {
    let spots =
        Memo::new(move |_| overlay::placed_labels(&overlay.get(), &view.get(), LABELS_SHOWN));
    let keys = Memo::new(move |_| {
        spots.with(|spots| {
            spots
                .iter()
                .map(|spot| (spot.key.clone(), spot.text.clone(), spot.colour))
                .collect::<Vec<_>>()
        })
    });
    view! {
        box(class = "map__layer") {
            for spot in move || keys.get(), key = |spot: &(String, String, u32)| spot.0.clone() {
                {label_view(spot, spots)}
            }
        }
    }
}

fn label_view(spot: (String, String, u32), spots: Memo<Vec<overlay::Placed>>) -> impl IntoView {
    let (key, text, colour) = spot;
    let at = move || {
        spots.with(|spots| {
            spots
                .iter()
                .find(|spot| spot.key == key)
                .map(|spot| (spot.x, spot.y))
        })
    };
    let left = at.clone();
    view! {
        text(
            class = "map__label",
            style:color = Some(format!("#{colour:06x}")),
            style:left = move || left().map(|(x, _)| format!("{x:.1}px")),
            style:top = move || at().map(|(_, y)| format!("{y:.1}px"))
        ) {{text}}
    }
}
