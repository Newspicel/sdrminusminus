use sdrmm_wire::{
    FusionGridOwned,
    fusion::{FUSION_FRAME_CELLS, FUSION_GRID_CELLS},
    geo::{Enu, LatLon},
};

use super::observation::{BANK_WIDTHS_DEG, BANKS, RING, Rings};

pub(crate) const CELLS: usize = FUSION_GRID_CELLS as usize;
pub(crate) const FRAME_CELLS: usize = FUSION_FRAME_CELLS as usize;
pub(crate) const SHIFT_STEP: usize = 4;
pub(crate) const EDGE_BAND: usize = CELLS / 8;
pub(crate) const POSTERIOR_SPAN: f32 = 9.21;

const SAME_PLACE_M: f64 = 1.0;
const SAME_ACCURACY_M: f32 = 1.0;
const FLAT_SPAN: f32 = 1e-3;

pub(crate) struct Wedge {
    pub(crate) ring: Box<Rings>,
    pub(crate) centre_value: f32,
    pub(crate) east_m: f64,
    pub(crate) north_m: f64,
    pub(crate) accuracy_m: f32,
}

impl Wedge {
    pub(crate) fn same_place(&self, other: &Self) -> bool {
        (self.east_m - other.east_m).hypot(self.north_m - other.north_m) < SAME_PLACE_M
            && (self.accuracy_m - other.accuracy_m).abs() < SAME_ACCURACY_M
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Look {
    centre: bool,
    bin: usize,
    next: usize,
    frac: f32,
    bank: usize,
    blend: f32,
}

pub(crate) fn look(east_m: f64, north_m: f64, accuracy_m: f32, cell_m: f64) -> Look {
    let range = east_m.hypot(north_m);
    if range < cell_m / 2.0 {
        return Look {
            centre: true,
            bin: 0,
            next: 0,
            frac: 0.0,
            bank: BANKS - 1,
            blend: 0.0,
        };
    }
    let bearing = east_m.atan2(north_m).to_degrees().rem_euclid(360.0) as f32;
    let spread = ((f64::from(accuracy_m) + cell_m / 2.0) / range)
        .atan()
        .to_degrees() as f32;
    let (bank, blend) = bank_for(spread);
    let floor = bearing.floor();
    let bin = (floor as usize) % RING;
    Look {
        centre: false,
        bin,
        next: (bin + 1) % RING,
        frac: bearing - floor,
        bank,
        blend,
    }
}

fn bank_for(spread_deg: f32) -> (usize, f32) {
    for bank in 0..BANKS - 1 {
        let (low, high) = (BANK_WIDTHS_DEG[bank], BANK_WIDTHS_DEG[bank + 1]);
        if spread_deg < high {
            return (bank, ((spread_deg - low) / (high - low)).clamp(0.0, 1.0));
        }
    }
    (BANKS - 1, 0.0)
}

impl Look {
    pub(crate) fn sample(&self, rings: &Rings, centre_value: f32) -> f32 {
        if self.centre {
            return centre_value;
        }
        let at = |ring: &[f32; RING]| {
            (ring[self.next] - ring[self.bin]).mul_add(self.frac, ring[self.bin])
        };
        let here = at(&rings[self.bank]);
        if self.blend > 0.0 && self.bank + 1 < BANKS {
            (at(&rings[self.bank + 1]) - here).mul_add(self.blend, here)
        } else {
            here
        }
    }
}

pub(crate) struct LogGrid {
    enu: Enu,
    cell_m: f64,
    centre_east_m: f64,
    centre_north_m: f64,
    g: Vec<f32>,
}

impl LogGrid {
    pub(crate) fn new(anchor: LatLon, extent_km: f64) -> Self {
        Self {
            enu: Enu::new(anchor),
            cell_m: 2.0 * extent_km * 1_000.0 / CELLS as f64,
            centre_east_m: 0.0,
            centre_north_m: 0.0,
            g: vec![0.0; CELLS * CELLS],
        }
    }

    pub(crate) fn enu(&self) -> &Enu {
        &self.enu
    }

    pub(crate) fn cell_m(&self) -> f64 {
        self.cell_m
    }

    pub(crate) fn values(&self) -> &[f32] {
        &self.g
    }

    #[cfg(test)]
    pub(crate) fn values_mut(&mut self) -> &mut [f32] {
        &mut self.g
    }

    pub(crate) fn centre(&self) -> (f64, f64) {
        (self.centre_east_m, self.centre_north_m)
    }

    fn offset(&self, index: usize) -> f64 {
        (index as f64 - (CELLS as f64 - 1.0) / 2.0) * self.cell_m
    }

    pub(crate) fn cell_centre(&self, row: usize, col: usize) -> (f64, f64) {
        (
            self.centre_east_m + self.offset(col),
            self.centre_north_m - self.offset(row),
        )
    }

    pub(crate) fn index_centre(&self, index: usize) -> (f64, f64) {
        self.cell_centre(index / CELLS, index % CELLS)
    }

    pub(crate) fn clear(&mut self) {
        self.g.fill(0.0);
    }

    pub(crate) fn paint(&mut self, old: Option<&Wedge>, new: Option<&Wedge>) {
        match (old, new) {
            (Some(old), Some(new)) if old.same_place(new) => {
                self.apply(new, |look| {
                    look.sample(&new.ring, new.centre_value)
                        - look.sample(&old.ring, old.centre_value)
                });
            }
            _ => {
                if let Some(old) = old {
                    self.apply(old, |look| -look.sample(&old.ring, old.centre_value));
                }
                if let Some(new) = new {
                    self.apply(new, |look| look.sample(&new.ring, new.centre_value));
                }
            }
        }
    }

    fn apply(&mut self, at: &Wedge, change: impl Fn(&Look) -> f32) {
        let cell_m = self.cell_m;
        for row in 0..CELLS {
            let north = self.centre_north_m - self.offset(row) - at.north_m;
            let east0 = self.centre_east_m - at.east_m;
            let cells = &mut self.g[row * CELLS..(row + 1) * CELLS];
            for (col, value) in cells.iter_mut().enumerate() {
                let east = east0 + (col as f64 - (CELLS as f64 - 1.0) / 2.0) * cell_m;
                *value += change(&look(east, north, at.accuracy_m, cell_m));
            }
        }
    }

    pub(crate) fn max(&self) -> (usize, f32) {
        self.g
            .iter()
            .copied()
            .enumerate()
            .fold((0, f32::NEG_INFINITY), |best, (index, value)| {
                if value > best.1 { (index, value) } else { best }
            })
    }

    fn half_width_m(&self) -> f64 {
        CELLS as f64 / 2.0 * self.cell_m
    }

    pub(crate) fn in_edge_band_around(&self, centre: (f64, f64), point: (f64, f64)) -> bool {
        let inner = self.half_width_m() - EDGE_BAND as f64 * self.cell_m;
        (point.0 - centre.0).abs() > inner || (point.1 - centre.1).abs() > inner
    }

    pub(crate) fn in_edge_band(&self, point: (f64, f64)) -> bool {
        self.in_edge_band_around(self.centre(), point)
    }

    pub(crate) fn snapped_centre(&self, east_m: f64, north_m: f64) -> (f64, f64) {
        let step = SHIFT_STEP as f64 * self.cell_m;
        let snap = |from: f64, to: f64| from + ((to - from) / step).round() * step;
        (
            snap(self.centre_east_m, east_m),
            snap(self.centre_north_m, north_m),
        )
    }

    pub(crate) fn recentre(&mut self, east_m: f64, north_m: f64) -> bool {
        let (east, north) = self.snapped_centre(east_m, north_m);
        if (east - self.centre_east_m).abs() < self.cell_m / 2.0
            && (north - self.centre_north_m).abs() < self.cell_m / 2.0
        {
            return false;
        }
        self.centre_east_m = east;
        self.centre_north_m = north;
        true
    }

    pub(crate) fn bounds(&self) -> (LatLon, LatLon) {
        let half = self.half_width_m();
        let south_west = self
            .enu
            .to_latlon(self.centre_east_m - half, self.centre_north_m - half);
        let north_east = self
            .enu
            .to_latlon(self.centre_east_m + half, self.centre_north_m + half);
        (south_west, north_east)
    }

    fn flat(&self) -> bool {
        let (low, high) = self
            .g
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), &value| {
                (low.min(value), high.max(value))
            });
        (high - low).is_nan() || high - low <= FLAT_SPAN
    }

    pub(crate) fn frame(&self, seq: u32, timestamp: u64) -> FusionGridOwned {
        if self.flat() {
            return self.blank_frame(seq, timestamp);
        }
        let (_, top) = self.max();
        let pool = CELLS / FRAME_CELLS;
        let mut cells = Vec::with_capacity(FRAME_CELLS * FRAME_CELLS);
        for frame_row in 0..FRAME_CELLS {
            for frame_col in 0..FRAME_CELLS {
                let mut best = f32::NEG_INFINITY;
                for row in frame_row * pool..(frame_row + 1) * pool {
                    for col in frame_col * pool..(frame_col + 1) * pool {
                        best = best.max(self.g[row * CELLS + col]);
                    }
                }
                cells.push(shade(best, top));
            }
        }
        self.framed(seq, timestamp, cells)
    }

    pub(crate) fn blank_frame(&self, seq: u32, timestamp: u64) -> FusionGridOwned {
        self.framed(seq, timestamp, vec![0; FRAME_CELLS * FRAME_CELLS])
    }

    fn framed(&self, seq: u32, timestamp: u64, cells: Vec<u8>) -> FusionGridOwned {
        let (south_west, north_east) = self.bounds();
        FusionGridOwned {
            stream_id: 0,
            seq,
            timestamp,
            south: south_west.lat,
            west: south_west.lon,
            north: north_east.lat,
            east: north_east.lon,
            cols: FUSION_FRAME_CELLS,
            rows: FUSION_FRAME_CELLS,
            cells,
        }
    }
}

fn shade(value: f32, top: f32) -> u8 {
    let level = (255.0 * (1.0 + (value - top) / POSTERIOR_SPAN)).round();
    if level.is_nan() {
        0
    } else {
        level.clamp(0.0, 255.0) as u8
    }
}
