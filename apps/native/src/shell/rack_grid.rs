use sdrmm_wire::patch::{PatchGraph, RACK_COLS, RACK_ROWS, RackCell, RackLayout, RackSlot};

pub const DEFAULT_W: i32 = 6;
pub const DEFAULT_H: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    North,
    East,
    South,
    West,
}

impl Edge {
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::East => Self::West,
            Self::South => Self::North,
            Self::West => Self::East,
        }
    }

    #[must_use]
    pub fn vertical(self) -> bool {
        matches!(self, Self::North | Self::South)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub min: i32,
    pub max: i32,
}

impl Cell {
    #[must_use]
    pub fn of(cell: RackCell) -> Self {
        Self {
            x: i32::from(cell.x),
            y: i32::from(cell.y),
            w: i32::from(cell.w),
            h: i32::from(cell.h),
        }
    }

    fn wire(self) -> Option<RackCell> {
        Some(RackCell {
            x: u16::try_from(self.x).ok()?,
            y: u16::try_from(self.y).ok()?,
            w: u16::try_from(self.w).ok()?,
            h: u16::try_from(self.h).ok()?,
        })
    }

    #[must_use]
    pub fn inside(self) -> bool {
        self.w >= 1
            && self.h >= 1
            && self.x >= 0
            && self.y >= 0
            && self.x + self.w <= i32::from(RACK_COLS)
            && self.y + self.h <= i32::from(RACK_ROWS)
    }

    fn overlaps(self, other: Self) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    fn moved_edge(self, edge: Edge, cells: i32) -> Self {
        match edge {
            Edge::North => Self {
                y: self.y + cells,
                h: self.h - cells,
                ..self
            },
            Edge::South => Self {
                h: self.h + cells,
                ..self
            },
            Edge::West => Self {
                x: self.x + cells,
                w: self.w - cells,
                ..self
            },
            Edge::East => Self {
                w: self.w + cells,
                ..self
            },
        }
    }

    fn abuts(self, other: Self, edge: Edge) -> bool {
        let spans_x = other.x < self.x + self.w && self.x < other.x + other.w;
        let spans_y = other.y < self.y + self.h && self.y < other.y + other.h;
        match edge {
            Edge::North => spans_x && other.y + other.h == self.y,
            Edge::South => spans_x && other.y == self.y + self.h,
            Edge::West => spans_y && other.x + other.w == self.x,
            Edge::East => spans_y && other.x == self.x + self.w,
        }
    }

    fn room(self, edge: Edge) -> Room {
        let cols = i32::from(RACK_COLS);
        let rows = i32::from(RACK_ROWS);
        match edge {
            Edge::North => Room {
                min: -self.y,
                max: self.h - 1,
            },
            Edge::South => Room {
                min: 1 - self.h,
                max: rows - self.y - self.h,
            },
            Edge::West => Room {
                min: -self.x,
                max: self.w - 1,
            },
            Edge::East => Room {
                min: 1 - self.w,
                max: cols - self.x - self.w,
            },
        }
    }
}

fn cells(rack: &RackLayout) -> Vec<(String, Cell)> {
    rack.slots
        .iter()
        .map(|slot| (slot.node.clone(), Cell::of(slot.cell)))
        .collect()
}

fn layout(cells: Vec<(String, Cell)>) -> Option<RackLayout> {
    let slots = cells
        .into_iter()
        .map(|(node, cell)| {
            Some(RackSlot {
                node,
                cell: cell.wire()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(RackLayout { slots })
}

#[must_use]
pub fn slot_of(rack: &RackLayout, node: &str) -> Option<Cell> {
    rack.slots
        .iter()
        .find(|slot| slot.node == node)
        .map(|slot| Cell::of(slot.cell))
}

#[must_use]
pub fn is_pinned(rack: &RackLayout, node: &str) -> bool {
    slot_of(rack, node).is_some()
}

#[must_use]
pub fn pin(rack: &RackLayout, node: &str) -> RackLayout {
    if is_pinned(rack, node) {
        return rack.clone();
    }
    let taken = cells(rack);
    for y in 0..=(i32::from(RACK_ROWS) - DEFAULT_H) {
        for x in 0..=(i32::from(RACK_COLS) - DEFAULT_W) {
            let cell = Cell {
                x,
                y,
                w: DEFAULT_W,
                h: DEFAULT_H,
            };
            if taken.iter().all(|(_, slot)| !slot.overlaps(cell)) {
                let mut next = taken;
                next.push((node.to_owned(), cell));
                return layout(next).unwrap_or_else(|| rack.clone());
            }
        }
    }
    rack.clone()
}

#[must_use]
pub fn unpin(rack: &RackLayout, node: &str) -> RackLayout {
    RackLayout {
        slots: rack
            .slots
            .iter()
            .filter(|slot| slot.node != node)
            .cloned()
            .collect(),
    }
}

#[must_use]
pub fn toggle_pin(rack: &RackLayout, node: &str) -> RackLayout {
    if is_pinned(rack, node) {
        unpin(rack, node)
    } else {
        pin(rack, node)
    }
}

#[must_use]
pub fn place(rack: &RackLayout, node: &str, cell: Cell) -> RackLayout {
    let slots = cells(rack);
    if !cell.inside()
        || !slots
            .iter()
            .all(|(owner, slot)| owner == node || !slot.overlaps(cell))
    {
        return rack.clone();
    }
    let next = slots
        .into_iter()
        .map(|(owner, slot)| {
            let placed = if owner == node { cell } else { slot };
            (owner, placed)
        })
        .collect();
    layout(next).unwrap_or_else(|| rack.clone())
}

#[must_use]
pub fn move_to(rack: &RackLayout, node: &str, x: i32, y: i32) -> RackLayout {
    let slots = cells(rack);
    let Some(from) = slot_of(rack, node) else {
        return rack.clone();
    };
    let cell = Cell { x, y, ..from };
    if !cell.inside() {
        return rack.clone();
    }
    let hit: Vec<&(String, Cell)> = slots
        .iter()
        .filter(|(owner, slot)| owner != node && slot.overlaps(cell))
        .collect();
    let swap = match hit.as_slice() {
        [] => None,
        [other] => Some((other.0.clone(), other.1)),
        _ => return rack.clone(),
    };
    let next = slots
        .iter()
        .map(|(owner, slot)| {
            let placed = match &swap {
                None if owner == node => cell,
                Some((_, other)) if owner == node => *other,
                Some((other, _)) if owner == other => from,
                _ => *slot,
            };
            (owner.clone(), placed)
        })
        .collect();
    layout(next).unwrap_or_else(|| rack.clone())
}

#[must_use]
pub fn resize(rack: &RackLayout, node: &str, edge: Edge, by: i32) -> RackLayout {
    let slots = cells(rack);
    let Some(slot) = slot_of(rack, node) else {
        return rack.clone();
    };
    if by == 0 {
        return rack.clone();
    }
    let next: Vec<(String, Cell)> = slots
        .iter()
        .map(|(owner, cell)| {
            let moved = if owner == node {
                cell.moved_edge(edge, by)
            } else if slot.abuts(*cell, edge) {
                cell.moved_edge(edge.opposite(), by)
            } else {
                *cell
            };
            (owner.clone(), moved)
        })
        .collect();
    let legal = next.iter().enumerate().all(|(at, (_, cell))| {
        cell.inside()
            && next
                .iter()
                .enumerate()
                .all(|(other_at, (_, other))| other_at == at || !cell.overlaps(*other))
    });
    if legal {
        layout(next).unwrap_or_else(|| rack.clone())
    } else {
        rack.clone()
    }
}

#[must_use]
pub fn room(rack: &RackLayout, node: &str, edge: Edge) -> Room {
    let Some(slot) = slot_of(rack, node) else {
        return Room { min: 0, max: 0 };
    };
    let mut rooms = vec![slot.room(edge)];
    rooms.extend(
        cells(rack)
            .into_iter()
            .filter(|(owner, other)| owner != node && slot.abuts(*other, edge))
            .map(|(_, other)| other.room(edge.opposite())),
    );
    Room {
        min: rooms.iter().map(|room| room.min).max().unwrap_or(0),
        max: rooms.iter().map(|room| room.max).min().unwrap_or(0),
    }
}

#[must_use]
pub fn clamp_cells(room: Room, cells: i32) -> i32 {
    cells.max(room.min).min(room.max.max(room.min))
}

#[must_use]
pub fn prune(rack: &RackLayout, graph: &PatchGraph) -> RackLayout {
    let kept: Vec<&RackSlot> = rack
        .slots
        .iter()
        .filter(|slot| graph.node(&slot.node).is_some())
        .collect();
    if kept.len() == rack.slots.len() && kept.iter().all(|slot| Cell::of(slot.cell).inside()) {
        return rack.clone();
    }
    let mut placed = RackLayout {
        slots: kept
            .iter()
            .filter(|slot| Cell::of(slot.cell).inside())
            .map(|slot| (*slot).clone())
            .collect(),
    };
    for slot in kept.iter().filter(|slot| !Cell::of(slot.cell).inside()) {
        placed = pin(&placed, &slot.node);
    }
    placed
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{NodeBody, PatchNode, Position};

    use super::*;

    fn empty() -> RackLayout {
        RackLayout::default()
    }

    fn slot(node: &str, x: u16, y: u16, w: u16, h: u16) -> RackSlot {
        RackSlot {
            node: node.to_owned(),
            cell: RackCell { x, y, w, h },
        }
    }

    fn cell(x: i32, y: i32, w: i32, h: i32) -> Cell {
        Cell { x, y, w, h }
    }

    fn graph(ids: &[&str]) -> PatchGraph {
        PatchGraph {
            nodes: ids
                .iter()
                .map(|id| PatchNode {
                    id: (*id).to_owned(),
                    body: NodeBody::Scope,
                    position: Position { x: 0.0, y: 0.0 },
                    size: None,
                    label: None,
                })
                .collect(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn pins_into_the_first_free_cell_and_unpins() {
        let mut rack = pin(&empty(), "scope");
        assert_eq!(rack.slots, [slot("scope", 0, 0, 6, 4)]);
        rack = pin(&rack, "nfm");
        assert_eq!(rack.slots[1], slot("nfm", 6, 0, 6, 4));
        rack = pin(&rack, "spk");
        assert_eq!(rack.slots[2], slot("spk", 0, 4, 6, 4));

        assert!(is_pinned(&rack, "nfm"));
        rack = unpin(&rack, "nfm");
        assert!(!is_pinned(&rack, "nfm"));
        assert_eq!(pin(&rack, "scope"), rack);
        assert_eq!(toggle_pin(&rack, "scope").slots.len(), 1);
    }

    #[test]
    fn a_full_grid_pins_nothing() {
        let full = (0..4).fold(empty(), |rack, at| pin(&rack, &format!("n{at}")));
        assert_eq!(full.slots.len(), 4);
        assert_eq!(pin(&full, "late"), full);
    }

    #[test]
    fn refuses_a_placement_that_overlaps_or_leaves_the_grid() {
        let rack = pin(&pin(&empty(), "a"), "b");
        assert_eq!(place(&rack, "b", cell(0, 0, 6, 4)), rack);
        assert_eq!(
            place(&rack, "b", cell(i32::from(RACK_COLS) - 2, 0, 6, 4)),
            rack
        );
        assert_eq!(
            place(&rack, "b", cell(0, 4, 6, 4)).slots[1],
            slot("b", 0, 4, 6, 4)
        );
        assert_eq!(place(&rack, "a", cell(0, 0, 6, 8)).slots[0].cell.h, 8);
    }

    #[test]
    fn trades_places_when_a_face_is_dropped_on_another() {
        let rack = place(&pin(&pin(&empty(), "a"), "b"), "b", cell(6, 0, 6, 8));
        let swapped = move_to(&rack, "a", 6, 0);
        assert_eq!(
            swapped.slots,
            [slot("a", 6, 0, 6, 8), slot("b", 0, 0, 6, 4)]
        );
        assert_eq!(move_to(&rack, "a", 0, 4).slots[0], slot("a", 0, 4, 6, 4));
        assert_eq!(move_to(&rack, "a", i32::from(RACK_COLS) - 1, 0), rack);
        let three = place(&pin(&rack, "c"), "c", cell(6, 4, 6, 4));
        assert_eq!(move_to(&three, "a", 5, 2), three);
    }

    #[test]
    fn moves_the_boundary_between_two_faces() {
        let rack = pin(&pin(&empty(), "a"), "b");
        let wider = resize(&rack, "a", Edge::East, 2);
        assert_eq!(wider.slots, [slot("a", 0, 0, 8, 4), slot("b", 8, 0, 4, 4)]);
        assert_eq!(resize(&wider, "b", Edge::West, -2), rack);
        assert_eq!(resize(&rack, "a", Edge::East, 6), rack);
        assert_eq!(resize(&rack, "a", Edge::South, 2).slots[0].cell.h, 6);
        assert_eq!(resize(&rack, "a", Edge::South, 6), rack);
        let stacked = place(&pin(&rack, "c"), "c", cell(6, 4, 6, 4));
        assert_eq!(
            resize(&stacked, "a", Edge::South, 1).slots,
            [
                slot("a", 0, 0, 6, 5),
                slot("b", 6, 0, 6, 4),
                slot("c", 6, 4, 6, 4)
            ]
        );
    }

    #[test]
    fn says_how_far_an_edge_can_travel() {
        let rack = pin(&pin(&empty(), "a"), "b");
        assert_eq!(room(&rack, "a", Edge::East), Room { min: -5, max: 5 });
        assert_eq!(room(&rack, "a", Edge::South), Room { min: -3, max: 4 });
        assert_eq!(room(&rack, "a", Edge::North), Room { min: 0, max: 3 });
        assert_eq!(room(&rack, "a", Edge::West), Room { min: 0, max: 5 });
        assert_eq!(room(&rack, "gone", Edge::East), Room { min: 0, max: 0 });

        let stacked = place(&pin(&rack, "c"), "c", cell(0, 4, 6, 4));
        assert_eq!(room(&stacked, "a", Edge::South), Room { min: -3, max: 3 });

        assert_eq!(clamp_cells(room(&rack, "a", Edge::East), 9), 5);
        assert_eq!(clamp_cells(room(&rack, "a", Edge::East), -9), -5);
        assert_eq!(clamp_cells(room(&rack, "a", Edge::East), 2), 2);
    }

    #[test]
    fn drops_slots_whose_node_is_gone_and_replaces_ones_outside_the_grid() {
        let rack = pin(&empty(), "nfm");
        assert!(prune(&rack, &graph(&["scope"])).slots.is_empty());
        assert_eq!(prune(&rack, &graph(&["nfm"])), rack);

        let stale = RackLayout {
            slots: vec![slot("nfm", 12, 12, 12, 8)],
        };
        assert_eq!(
            prune(&stale, &graph(&["nfm"])).slots,
            [slot("nfm", 0, 0, 6, 4)]
        );
    }
}
