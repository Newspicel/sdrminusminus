use super::view::above;
use zgui::{
    geom::{Css, CssPx, Point},
    prelude::*,
};

use super::{
    MenuAt, ScopeCx,
    actions::{
        select_channel, tunable_now, tune_centre, tune_channel, untracked_radio, update_readout,
    },
    live::now_ms,
    pick::{drag_tune_hz, pick_at},
    radio::marker_at,
    view::{FULL_VIEW, SpectrumView, span_to_offset},
};

const DRAG_SLOP_PX: f64 = 4.0;
const GRAB_PX: f64 = 12.0;
const TUNE_THROTTLE_MS: f64 = 150.0;
const DOUBLE_CLICK_MS: f64 = 400.0;
const TRACE_MIN: f64 = 0.15;
const TRACE_MAX: f64 = 0.75;
const LINE_PX: f64 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gesture {
    pointer_x: f64,
    at: f64,
    view: SpectrumView,
    channel: Option<u32>,
    moved: bool,
    centre_hz: f64,
    span_hz: f64,
    sent_hz: Option<f64>,
    sent_at: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    at: f64,
    down: f64,
    width: f64,
    height: f64,
}

fn spot(cx: ScopeCx, position: Point<CssPx, Css>) -> Option<Spot> {
    let bounds = cx.plot.window_bounds()?;
    let scale = f64::from(cx.plot.scale().max(0.01));
    let left = f64::from(bounds.origin.x.0) / scale;
    let top = f64::from(bounds.origin.y.0) / scale;
    let width = f64::from(bounds.size.width.0) / scale;
    let height = f64::from(bounds.size.height.0) / scale;
    (width > 0.0 && height > 0.0).then(|| Spot {
        at: (f64::from(position.x.0) - left) / width,
        down: (f64::from(position.y.0) - top) / height,
        width,
        height,
    })
}

fn active_now(cx: ScopeCx) -> bool {
    cx.store.pane.get_untracked() == crate::store::Pane::Rack
        || cx.store.selected.get_untracked().as_deref() == Some(cx.node.get_value().as_str())
}

pub fn handlers(cx: ScopeCx) -> Attrs {
    Attrs::new()
        .listener(
            events::POINTER_DOWN,
            ListenerOptions::DEFAULT,
            move |ev: &mut EventCx<'_, events::PointerDown>| press(cx, ev),
        )
        .listener(
            events::POINTER_MOVE,
            ListenerOptions::DEFAULT,
            move |ev: &mut EventCx<'_, events::PointerMove>| drag(cx, ev.position),
        )
        .listener(
            events::POINTER_UP,
            ListenerOptions::DEFAULT,
            move |ev: &mut EventCx<'_, events::PointerUp>| {
                ev.release_pointer();
                release(cx, ev.position, true);
            },
        )
        .listener(
            events::POINTER_CANCEL,
            ListenerOptions::DEFAULT,
            move |ev: &mut EventCx<'_, events::PointerCancel>| {
                ev.release_pointer();
                release(cx, ev.position, false);
            },
        )
        .listener(
            events::POINTER_LEAVE,
            ListenerOptions::DEFAULT,
            move |_: &mut EventCx<'_, events::PointerLeave>| {
                if cx.gesture.with_value(Option::is_none) {
                    cx.hover.set(None);
                    update_readout(cx);
                }
            },
        )
        .listener(
            events::WHEEL,
            ListenerOptions::DEFAULT,
            move |ev: &mut EventCx<'_, events::Wheel>| wheel(cx, ev),
        )
}

fn press(cx: ScopeCx, ev: &mut EventCx<'_, events::PointerDown>) {
    let grabbed = cx.grabbed.get_value();
    cx.grabbed.set_value(None);
    match ev.button {
        Some(PointerButton::Secondary) => {
            if open_menu(cx, ev.position) {
                ev.stop_propagation();
            }
            return;
        }
        Some(PointerButton::Primary) => {}
        _ => return,
    }
    if !active_now(cx) {
        return;
    }
    let (Some(meta), Some(spot)) = (cx.meta.get_untracked(), spot(cx, ev.position)) else {
        return;
    };
    if !above(meta.span_hz, 0.0) {
        return;
    }
    cx.menu.set(None);
    cx.picker.set(None);
    cx.settings_open.set(false);
    let view = cx.view.get_untracked();
    let radio = untracked_radio(cx);
    let channel = grabbed
        .filter(|id| radio.channel(*id).is_some())
        .or_else(|| {
            marker_at(
                &radio.channels,
                view,
                meta.centre_hz,
                meta.span_hz,
                spot.at,
                GRAB_PX / spot.width,
            )
        });
    if let Some(channel) = channel {
        select_channel(cx, channel);
    }
    cx.gesture.set_value(Some(Gesture {
        pointer_x: f64::from(ev.position.x.0),
        at: spot.at,
        view,
        channel,
        moved: false,
        centre_hz: meta.centre_hz,
        span_hz: meta.span_hz,
        sent_hz: None,
        sent_at: 0.0,
    }));
    ev.capture_pointer();
    ev.stop_propagation();
}

fn hover(cx: ScopeCx, at: Option<f64>) {
    let next = at.filter(|at| (0.0..=1.0).contains(at) && active_now(cx));
    if cx.hover.get_untracked() != next {
        cx.hover.set(next);
        update_readout(cx);
    }
}

fn drag(cx: ScopeCx, position: Point<CssPx, Css>) {
    let spot = spot(cx, position);
    hover(cx, spot.map(|spot| spot.at));
    let (Some(mut gesture), Some(spot)) = (cx.gesture.get_value(), spot) else {
        return;
    };
    let x = f64::from(position.x.0);
    if !gesture.moved && (x - gesture.pointer_x).abs() < DRAG_SLOP_PX {
        return;
    }
    gesture.moved = true;
    let radio = untracked_radio(cx);
    if let Some(channel) = gesture.channel {
        if !radio.held(channel) {
            let offset = span_to_offset(gesture.view.to_span(spot.at), gesture.span_hz).round();
            cx.preview.set(Some((channel, offset)));
        }
        cx.gesture.set_value(Some(gesture));
        return;
    }
    if gesture.view.is_full() && (radio.centre_held || radio.on_auto) {
        cx.gesture.set_value(Some(gesture));
        return;
    }
    cx.panning.set(true);
    if gesture.view.is_full() {
        let hz = drag_tune_hz(
            gesture.centre_hz,
            gesture.span_hz,
            gesture.view,
            gesture.pointer_x - x,
            spot.width,
        );
        let now = now_ms();
        if Some(hz) != gesture.sent_hz && now - gesture.sent_at >= TUNE_THROTTLE_MS {
            gesture.sent_hz = Some(hz);
            gesture.sent_at = now;
            tune_centre(cx, &radio, hz);
        }
    } else {
        cx.view
            .set(gesture.view.pan((gesture.pointer_x - x) / spot.width));
    }
    cx.gesture.set_value(Some(gesture));
}

fn release(cx: ScopeCx, position: Point<CssPx, Css>, finished: bool) {
    let gesture = cx.gesture.get_value();
    cx.gesture.set_value(None);
    cx.panning.set(false);
    let preview = cx.preview.get_untracked();
    cx.preview.set(None);
    let (Some(gesture), true) = (gesture, finished) else {
        return;
    };
    let radio = untracked_radio(cx);
    let x = f64::from(position.x.0);
    if gesture.moved {
        match (gesture.channel, preview) {
            (Some(channel), Some((_, offset))) => {
                tune_channel(cx, &radio, channel, gesture.centre_hz + offset);
            }
            (None, _) if gesture.view.is_full() && !radio.on_auto && !radio.centre_held => {
                let width = spot(cx, position).map_or(1.0, |spot| spot.width);
                let hz = drag_tune_hz(
                    gesture.centre_hz,
                    gesture.span_hz,
                    gesture.view,
                    gesture.pointer_x - x,
                    width,
                );
                if Some(hz) != gesture.sent_hz {
                    tune_centre(cx, &radio, hz);
                }
            }
            _ => {}
        }
        return;
    }
    if gesture.channel.is_some() {
        return;
    }
    let pick = pick_at(gesture.centre_hz, gesture.span_hz, gesture.view, gesture.at);
    match tunable_now(cx, &radio) {
        Some(channel) => tune_channel(cx, &radio, channel, pick.hz),
        None => tune_centre(cx, &radio, pick.hz),
    }
    double_click(cx, x, pick.hz);
}

fn double_click(cx: ScopeCx, x: f64, hz: f64) {
    let now = now_ms();
    let previous = cx.last_click.get_value();
    cx.last_click.set_value(Some((now, x)));
    let Some((then, at)) = previous else {
        return;
    };
    if now - then < DOUBLE_CLICK_MS && (x - at).abs() < DRAG_SLOP_PX {
        cx.last_click.set_value(None);
        tune_centre(cx, &untracked_radio(cx), hz);
        cx.view.set(FULL_VIEW);
    }
}

fn open_menu(cx: ScopeCx, position: Point<CssPx, Css>) -> bool {
    if !active_now(cx) {
        return false;
    }
    let (Some(meta), Some(spot)) = (cx.meta.get_untracked(), spot(cx, position)) else {
        return false;
    };
    let view = cx.view.get_untracked();
    cx.settings_open.set(false);
    cx.picker.set(None);
    cx.menu.set(Some(MenuAt {
        pick: pick_at(meta.centre_hz, meta.span_hz, view, spot.at),
        x: spot.at,
        y: spot.down,
        stamp: stamp(meta.centre_hz, meta.span_hz, view),
    }));
    true
}

#[must_use]
pub fn stamp(centre_hz: f64, span_hz: f64, view: SpectrumView) -> (u64, u64, u64, u64) {
    (
        centre_hz.to_bits(),
        span_hz.to_bits(),
        view.start.to_bits(),
        view.end.to_bits(),
    )
}

#[must_use]
pub fn wheel_delta(delta: ScrollDelta) -> (f64, f64) {
    match delta {
        ScrollDelta::Lines { x, y } => (f64::from(x) * LINE_PX, f64::from(y) * LINE_PX),
        ScrollDelta::Pixels(size) => (f64::from(size.width.0), f64::from(size.height.0)),
        _ => (0.0, 0.0),
    }
}

fn wheel(cx: ScopeCx, ev: &mut EventCx<'_, events::Wheel>) {
    if ev.modifiers.control() || ev.modifiers.meta() {
        return;
    }
    let (dx, dy) = wheel_delta(ev.delta);
    ev.prevent_default();
    ev.stop_propagation();
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let Some(spot) = spot(cx, ev.position) else {
        return;
    };
    cx.view
        .update(|view| *view = view.wheel(dx, dy, spot.at, spot.width));
}

#[must_use]
pub fn split_at(down: f64) -> f64 {
    down.clamp(TRACE_MIN, TRACE_MAX)
}

pub fn divider(cx: ScopeCx) -> impl IntoView {
    let held = RwSignal::new(false);
    view! {
        box(
            class = "scope__divider",
            class:held = move || held.get(),
            a11y:role = Role::Splitter,
            a11y:label = "Trace and waterfall split",
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                ev.stop_propagation();
                if ev.button == Some(PointerButton::Primary) {
                    ev.capture_pointer();
                    held.set(true);
                }
            },
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| {
                if !held.get_untracked() {
                    return;
                }
                ev.stop_propagation();
                if let Some(spot) = spot(cx, ev.position) {
                    cx.fraction.set(split_at(spot.down));
                }
            },
            on:pointer_up = move |ev: &mut EventCx<'_, events::PointerUp>| {
                ev.stop_propagation();
                ev.release_pointer();
                held.set(false);
            }
        ) {
            box(class = "scope__divider-line")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_split_stays_between_its_limits() {
        assert_eq!(split_at(0.05), TRACE_MIN);
        assert_eq!(split_at(0.4), 0.4);
        assert_eq!(split_at(0.95), TRACE_MAX);
    }

    #[test]
    fn a_notched_wheel_reads_as_lines_of_pixels() {
        assert_eq!(
            wheel_delta(ScrollDelta::Lines { x: 0.0, y: -1.0 }),
            (0.0, -LINE_PX)
        );
    }

    #[test]
    fn a_menu_goes_stale_when_the_frame_or_view_moves() {
        let held = stamp(100e6, 2e6, FULL_VIEW);
        assert_eq!(held, stamp(100e6, 2e6, FULL_VIEW));
        assert_ne!(held, stamp(100.1e6, 2e6, FULL_VIEW));
        assert_ne!(held, stamp(100e6, 2e6, FULL_VIEW.zoom(0.5, 2.0)));
    }
}
