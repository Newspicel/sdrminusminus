use sdrmm_wire::patch::{RACK_COLS, RACK_ROWS, RackLayout, RackSlot};
use zgui::prelude::*;

use crate::{
    shell::rack_grid::{self, Cell, Edge},
    store::Store,
    ui::{
        faces,
        kit_shell::{glyph, icon_button},
        node,
    },
};

const SHEET: &str = css!(
    r#"
.rackgrid { flex: 1 1 auto; position: relative; margin: 1px; min-width: 0; min-height: 0; }
.rackgrid__empty { flex: 1 1 auto; align-items: center; justify-content: center; }
.rackslot { position: absolute; padding: 1px; }
.rackslot__card { position: relative; width: 100%; height: 100%; flex-direction: column; border: 1px solid var(--line); border-radius: 6px; background-color: var(--panel); overflow: hidden; }
.rackslot.sel .rackslot__card { border-color: var(--accent); }
.rackslot__face { flex: 1 1 auto; min-height: 0; overflow: auto; }
.rackgrip { position: absolute; z-index: 4; }
.rackgrip.n { left: 0; right: 0; top: 0; height: 6px; cursor: ns-resize; }
.rackgrip.s { left: 0; right: 0; bottom: 0; height: 6px; cursor: ns-resize; }
.rackgrip.w { top: 0; bottom: 0; left: 0; width: 6px; cursor: ew-resize; }
.rackgrip.e { top: 0; bottom: 0; right: 0; width: 6px; cursor: ew-resize; }
.rackgrip.move { top: 4px; left: 6px; right: 56px; height: 20px; cursor: move; }
.rackgrip.corner { right: 0; bottom: 0; width: 14px; height: 14px; cursor: nwse-resize; border-right: 2px solid var(--line-strong); border-bottom: 2px solid var(--line-strong); }
.rackgrip:hover { background-color: color-mix(in oklab, var(--accent) 25%, transparent); }
.rackgrip.corner:hover { border-color: var(--accent); }
"#
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Move,
    Corner,
    Edge(Edge),
}

#[derive(Clone, Debug)]
struct Gesture {
    node: String,
    mode: Mode,
    origin: (f32, f32),
    base: RackLayout,
}

#[must_use]
pub fn apply_gesture(base: &RackLayout, node: &str, mode: Mode, dx: i32, dy: i32) -> RackLayout {
    let Some(slot) = rack_grid::slot_of(base, node) else {
        return base.clone();
    };
    let cols = i32::from(RACK_COLS);
    let rows = i32::from(RACK_ROWS);
    match mode {
        Mode::Move => rack_grid::move_to(
            base,
            node,
            (slot.x + dx).clamp(0, (cols - slot.w).max(0)),
            (slot.y + dy).clamp(0, (rows - slot.h).max(0)),
        ),
        Mode::Corner => rack_grid::place(
            base,
            node,
            Cell {
                w: (slot.w + dx).clamp(1, (cols - slot.x).max(1)),
                h: (slot.h + dy).clamp(1, (rows - slot.y).max(1)),
                ..slot
            },
        ),
        Mode::Edge(edge) => {
            let cells = if edge.vertical() { dy } else { dx };
            let room = rack_grid::room(base, node, edge);
            rack_grid::resize(base, node, edge, rack_grid::clamp_cells(room, cells))
        }
    }
}

fn percent(value: u16, of: u16) -> Option<String> {
    Some(format!("{}%", f32::from(value) * 100.0 / f32::from(of)))
}

pub fn pane(store: Store) -> impl IntoView {
    install_stylesheet("shell-rack", SHEET);
    let host = NodeRef::new();
    let preview = RwSignal::new(None::<RackLayout>);
    let shown =
        Signal::derive(move || preview.get().unwrap_or_else(|| (*store.rack.get()).clone()));
    view! {
        if move || shown.get().slots.is_empty() {
            row(class = "rackgrid__empty") {
                text(class = "sk-text") {"Nothing pinned. Select a node and press p."}
            }
        } else {
            box(class = "rackgrid", node_ref = host) {
                for slot in move || shown.get().slots, key = |slot: &RackSlot| slot.node.clone() {
                    {slot_view(store, host, shown, preview, slot.node)}
                }
            }
        }
    }
}

fn slot_view(
    store: Store,
    host: NodeRef,
    shown: Signal<RackLayout>,
    preview: RwSignal<Option<RackLayout>>,
    id: String,
) -> impl IntoView {
    let cell = {
        let id = id.clone();
        Signal::derive(move || {
            shown
                .get()
                .slots
                .iter()
                .find(|slot| slot.node == id)
                .map(|slot| slot.cell)
        })
    };
    let Some(found) = store.graph.get_untracked().node(&id).cloned() else {
        return AnyView::new(());
    };
    let title = {
        let id = id.clone();
        move || {
            store
                .graph
                .get()
                .node(&id)
                .map(node::title_of)
                .unwrap_or_default()
        }
    };
    let selected = {
        let id = id.clone();
        move || store.selected.get().as_deref() == Some(id.as_str())
    };
    let pick = {
        let id = id.clone();
        move |_: &mut EventCx<'_, events::PointerDown>| store.selected.set(Some(id.clone()))
    };
    let category = node::category_class(found.body.category());
    let full = id.clone();
    let body = faces::face(store, &found);
    let grips: Vec<AnyView> = [
        (Mode::Edge(Edge::North), "rackgrip n"),
        (Mode::Edge(Edge::South), "rackgrip s"),
        (Mode::Edge(Edge::West), "rackgrip w"),
        (Mode::Edge(Edge::East), "rackgrip e"),
        (Mode::Move, "rackgrip move"),
        (Mode::Corner, "rackgrip corner"),
    ]
    .into_iter()
    .map(|(mode, class)| AnyView::new(grip(store, host, preview, id.clone(), mode, class)))
    .collect();
    AnyView::new(view! {
        box(
            class = "rackslot",
            class:sel = selected,
            style:left = move || cell.get().and_then(|cell| percent(cell.x, RACK_COLS)),
            style:top = move || cell.get().and_then(|cell| percent(cell.y, RACK_ROWS)),
            style:width = move || cell.get().and_then(|cell| percent(cell.w, RACK_COLS)),
            style:height = move || cell.get().and_then(|cell| percent(cell.h, RACK_ROWS)),
            on:pointer_down = pick
        ) {
            column(class = "rackslot__card", attr:data-category = category) {
                row(class = "node__bar") {
                    text(class = "node__title") {{title}}
                    spacer() {}
                    {icon_button(glyph::EXPAND, "Full screen", move || store.expanded.set(Some(full.clone())))}
                }
                column(class = "rackslot__face") {{body}}
                {grips}
            }
        }
    })
}

fn cells_moved(host: NodeRef, origin: (f32, f32), at: (f32, f32)) -> (i32, i32) {
    let Some(bounds) = host.window_bounds() else {
        return (0, 0);
    };
    let scale = host.scale().max(f32::EPSILON);
    let cell_w = (bounds.width().0 / scale / f32::from(RACK_COLS)).max(1.0);
    let cell_h = (bounds.height().0 / scale / f32::from(RACK_ROWS)).max(1.0);
    (
        ((at.0 - origin.0) / cell_w).round() as i32,
        ((at.1 - origin.1) / cell_h).round() as i32,
    )
}

fn grip(
    store: Store,
    host: NodeRef,
    preview: RwSignal<Option<RackLayout>>,
    node: String,
    mode: Mode,
    class: &'static str,
) -> impl IntoView {
    let gesture = StoredValue::new_local(None::<Gesture>);
    let press = move |ev: &mut EventCx<'_, events::PointerDown>| {
        if ev.button != Some(PointerButton::Primary) {
            return;
        }
        ev.stop_propagation();
        ev.prevent_default();
        ev.capture_pointer();
        gesture.set_value(Some(Gesture {
            node: node.clone(),
            mode,
            origin: (ev.position.x.0, ev.position.y.0),
            base: (*store.rack.get_untracked()).clone(),
        }));
    };
    let drag = move |ev: &mut EventCx<'_, events::PointerMove>| {
        let Some(active) = gesture.get_value() else {
            return;
        };
        let (dx, dy) = cells_moved(host, active.origin, (ev.position.x.0, ev.position.y.0));
        let next = apply_gesture(&active.base, &active.node, active.mode, dx, dy);
        if preview.get_untracked().as_ref() != Some(&next) {
            preview.set(Some(next));
        }
    };
    let release = move |ev: &mut EventCx<'_, events::PointerUp>| {
        ev.release_pointer();
        let Some(active) = gesture.get_value() else {
            return;
        };
        gesture.set_value(None);
        preview.set(None);
        let (dx, dy) = cells_moved(host, active.origin, (ev.position.x.0, ev.position.y.0));
        if dx != 0 || dy != 0 {
            store.edit_rack(|rack| apply_gesture(rack, &active.node, active.mode, dx, dy));
        }
    };
    view! {
        box(
            class = class,
            on:pointer_down = press,
            on:pointer_move = drag,
            on:pointer_up = release,
            on:pointer_cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
                ev.release_pointer();
                gesture.set_value(None);
                preview.set(None);
            }
        ) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> RackLayout {
        rack_grid::pin(&rack_grid::pin(&RackLayout::default(), "a"), "b")
    }

    #[test]
    fn a_move_gesture_swaps_with_the_face_it_lands_on() {
        let moved = apply_gesture(&two(), "a", Mode::Move, 6, 0);
        assert_eq!(rack_grid::slot_of(&moved, "a").map(|cell| cell.x), Some(6));
        assert_eq!(rack_grid::slot_of(&moved, "b").map(|cell| cell.x), Some(0));
    }

    #[test]
    fn an_edge_gesture_stops_where_the_grid_does() {
        let wider = apply_gesture(&two(), "a", Mode::Edge(Edge::East), 9, 0);
        assert_eq!(rack_grid::slot_of(&wider, "a").map(|cell| cell.w), Some(11));
        assert_eq!(rack_grid::slot_of(&wider, "b").map(|cell| cell.w), Some(1));
    }

    #[test]
    fn a_corner_gesture_grows_into_free_room_only() {
        let taller = apply_gesture(&two(), "a", Mode::Corner, 0, 3);
        assert_eq!(rack_grid::slot_of(&taller, "a").map(|cell| cell.h), Some(7));
        assert_eq!(apply_gesture(&two(), "a", Mode::Corner, 3, 0), two());
        assert_eq!(apply_gesture(&two(), "gone", Mode::Move, 1, 1), two());
    }
}
