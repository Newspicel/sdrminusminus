use super::view::{above, at_least};
use super::{
    colormap::Colormap,
    traces::{DbWindow, trace_unit},
    view::{SpectrumView, span_to_offset},
};

pub const AXIS_H: f64 = 16.0;
pub const DENSITY_WIDTH: usize = 480;
pub const DENSITY_HEIGHT: usize = 192;
pub const DENSITY_DECAY: f32 = 0.92;
pub const DENSITY_GAIN: f32 = 0.17;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TracePoints {
    pub xs: Vec<f64>,
    pub low: Vec<f32>,
    pub high: Vec<f32>,
}

impl TracePoints {
    #[must_use]
    pub fn count(&self) -> usize {
        self.xs.len()
    }

    fn clear(&mut self) {
        self.xs.clear();
        self.low.clear();
        self.high.clear();
    }

    fn push(&mut self, x: f64, low: f32, high: f32) {
        self.xs.push(x);
        self.low.push(low);
        self.high.push(high);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Readout {
    pub hz: f64,
    pub db: f64,
    pub bin: usize,
}

#[must_use]
pub fn readout_at(
    centre_hz: f64,
    span_hz: f64,
    db: &[f32],
    view: SpectrumView,
    at: f64,
) -> Option<Readout> {
    if !(0.0..=1.0).contains(&at) || db.is_empty() || !above(span_hz, 0.0) {
        return None;
    }
    let fraction = view.to_span(at);
    let last = db.len() - 1;
    let bin = (fraction * last as f64).round().clamp(0.0, last as f64) as usize;
    Some(Readout {
        hz: centre_hz + span_to_offset(fraction, span_hz),
        db: f64::from(db[bin]),
        bin,
    })
}

pub fn trace_points(db: &[f32], view: SpectrumView, width: f64, points: &mut TracePoints) {
    points.clear();
    let n = db.len();
    if n < 2 || width < 1.0 {
        return;
    }
    let first = view.start * (n - 1) as f64;
    let last = view.end * (n - 1) as f64;
    if !above(last, first) {
        return;
    }
    if last - first < width {
        bin_points(db, first, last, width, points);
    } else {
        pixel_points(db, first, last, width, points);
    }
}

fn bin_points(db: &[f32], first: f64, last: f64, width: f64, points: &mut TracePoints) {
    let low = first.floor().max(0.0) as usize;
    let high = (last.ceil() as usize).min(db.len() - 1);
    for (offset, value) in db[low..=high].iter().enumerate() {
        let at = (low + offset) as f64;
        points.push((at - first) / (last - first) * width, *value, *value);
    }
}

fn pixel_points(db: &[f32], first: f64, last: f64, width: f64, points: &mut TracePoints) {
    let n = db.len();
    let columns = width.ceil() as usize;
    for x in 0..columns {
        let from = first + (last - first) * x as f64 / width;
        let to = first + (last - first) * (x + 1) as f64 / width;
        let low_bin = from.floor().max(0.0) as usize;
        let high_bin = (to.ceil() as usize)
            .saturating_sub(1)
            .max(low_bin)
            .min(n - 1);
        let (low, high) = db[low_bin.min(n - 1)..=high_bin]
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
                (low.min(*value), high.max(*value))
            });
        points.push(x as f64 + 0.5, low, high);
    }
}

pub fn columns_of(points: &TracePoints, columns: usize, high: &mut Vec<f32>, low: &mut Vec<f32>) {
    high.clear();
    low.clear();
    let count = points.count();
    if count == 0 {
        high.resize(columns, f32::NAN);
        low.resize(columns, f32::NAN);
        return;
    }
    let mut segment = 0;
    for column in 0..columns {
        let x = column as f64 + 0.5;
        while segment + 1 < count - 1 && points.xs[segment + 1] < x {
            segment += 1;
        }
        let (h, l) = if count == 1 || x <= points.xs[0] {
            (points.high[0], points.low[0])
        } else if x >= points.xs[count - 1] {
            (points.high[count - 1], points.low[count - 1])
        } else {
            let (x0, x1) = (points.xs[segment], points.xs[segment + 1]);
            let t = if x1 > x0 {
                ((x - x0) / (x1 - x0)) as f32
            } else {
                0.0
            };
            (
                lerp(points.high[segment], points.high[segment + 1], t),
                lerp(points.low[segment], points.low[segment + 1], t),
            )
        };
        high.push(h);
        low.push(l);
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if a.is_finite() && b.is_finite() {
        a + (b - a) * t
    } else if t < 0.5 {
        a
    } else {
        b
    }
}

#[must_use]
pub fn level_y(db: f32, height: f64, window: DbWindow) -> f32 {
    ((1.0 - trace_unit(f64::from(db), window)) * height) as f32
}

#[must_use]
pub fn format_tick(hz: f64, visible_hz: f64) -> String {
    let decimals = if visible_hz >= 5e6 {
        1
    } else if visible_hz >= 5e5 {
        2
    } else if visible_hz >= 5e4 {
        3
    } else {
        4
    };
    format!("{:.decimals$}", hz / 1e6)
}

pub struct DensityGrid {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<f32>,
    lo: Vec<f32>,
    hi: Vec<f32>,
}

impl Default for DensityGrid {
    fn default() -> Self {
        Self::new(DENSITY_WIDTH, DENSITY_HEIGHT)
    }
}

impl DensityGrid {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0.0; width * height],
            lo: vec![0.0; width],
            hi: vec![0.0; width],
        }
    }

    pub fn clear(&mut self) {
        self.cells.fill(0.0);
    }

    pub fn decay(&mut self, factor: f32) {
        for cell in &mut self.cells {
            let value = *cell * factor;
            *cell = if value < 0.002 { 0.0 } else { value };
        }
    }

    pub fn add(&mut self, db: &[f32], view: SpectrumView, window: DbWindow, gain: f32) {
        let n = db.len();
        if n == 0 || self.height < 1 {
            return;
        }
        self.column_ranges(db, view);
        let top = (self.height - 1) as f64;
        for x in 0..self.width {
            let (mut low, mut high) = (self.lo[x], self.hi[x]);
            if !at_least(f64::from(high), f64::from(low)) {
                continue;
            }
            if x > 0 {
                low = low.min(self.hi[x - 1]);
                high = high.max(self.lo[x - 1]);
            }
            let y_low = ((1.0 - trace_unit(f64::from(high), window)) * top).round() as usize;
            let y_high = ((1.0 - trace_unit(f64::from(low), window)) * top).round() as usize;
            for y in y_low..=y_high {
                let at = y * self.width + x;
                self.cells[at] = (self.cells[at] + gain).min(1.0);
            }
        }
    }

    fn column_ranges(&mut self, db: &[f32], view: SpectrumView) {
        let n = db.len();
        let first = view.start * (n - 1) as f64;
        let last = view.end * (n - 1) as f64;
        let width = self.width as f64;
        for x in 0..self.width {
            let from = first + (last - first) * x as f64 / width;
            let to = first + (last - first) * (x + 1) as f64 / width;
            let start = from.floor().max(0.0) as usize;
            let stop = (to.floor().max(0.0) as usize).max(start).min(n - 1);
            let (low, high) = db[start.min(n - 1)..=stop]
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
                    (low.min(*value), high.max(*value))
                });
            self.lo[x] = low;
            self.hi[x] = high;
        }
    }

    pub fn to_image(&self, lut: &[u8], out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.cells.len() * 4);
        for value in &self.cells {
            if *value <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let entry = (value * 255.0).round().min(255.0) as usize * 3;
            out.extend_from_slice(&[
                lut[entry],
                lut[entry + 1],
                lut[entry + 2],
                (10.0 + value * 245.0).round().min(255.0) as u8,
            ]);
        }
    }
}

pub struct DensityLayer {
    pub grid: DensityGrid,
    lut: Vec<u8>,
    colormap: Colormap,
    pub revision: u64,
}

impl DensityLayer {
    #[must_use]
    pub fn new(colormap: Colormap) -> Self {
        Self {
            grid: DensityGrid::default(),
            lut: colormap.lut(),
            colormap,
            revision: 0,
        }
    }

    pub fn set_colormap(&mut self, colormap: Colormap) {
        if colormap != self.colormap {
            self.colormap = colormap;
            self.lut = colormap.lut();
            self.revision += 1;
        }
    }

    pub fn add(&mut self, db: &[f32], view: SpectrumView, window: DbWindow) {
        self.grid.decay(DENSITY_DECAY);
        self.grid.add(db, view, window, DENSITY_GAIN);
        self.revision += 1;
    }

    pub fn clear(&mut self) {
        self.grid.clear();
        self.revision += 1;
    }

    pub fn image(&self, out: &mut Vec<u8>) {
        self.grid.to_image(&self.lut, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{super::view::FULL_VIEW, *};

    const WINDOW: DbWindow = DbWindow {
        min: -100.0,
        max: -20.0,
    };

    fn read(db: &[f32], view: SpectrumView, at: f64) -> Option<Readout> {
        readout_at(100e6, 2e6, db, view, at)
    }

    #[test]
    fn the_readout_follows_the_cursor_and_the_zoom() {
        let db = [-90.0, -80.0, -70.0, -60.0, -50.0];
        let middle = read(&db, FULL_VIEW, 0.5).expect("a readout");
        assert_eq!((middle.hz, middle.db), (100e6, -70.0));
        assert_eq!(read(&db[..3], FULL_VIEW, 0.0).map(|r| r.hz), Some(99e6));
        assert_eq!(read(&db[..3], FULL_VIEW, 1.0).map(|r| r.hz), Some(101e6));
        let half = SpectrumView {
            start: 0.5,
            end: 1.0,
        };
        let zoomed = read(&db, half, 0.5).expect("a readout");
        assert_eq!((zoomed.hz, zoomed.db), (100.5e6, -60.0));
    }

    #[test]
    fn the_readout_is_empty_off_the_plot_or_without_a_span() {
        assert_eq!(read(&[-90.0, -80.0], FULL_VIEW, -0.1), None);
        assert_eq!(read(&[-90.0, -80.0], FULL_VIEW, 1.1), None);
        assert_eq!(
            readout_at(100e6, 0.0, &[-90.0, -80.0], FULL_VIEW, 0.5),
            None
        );
    }

    #[test]
    fn a_pixel_keeps_the_peak_and_floor_of_its_bins() {
        let mut points = TracePoints::default();
        trace_points(&[-90.0, -40.0, -80.0, -85.0], FULL_VIEW, 1.5, &mut points);
        assert_eq!(points.count(), 2);
        assert_eq!((points.high[0], points.low[0]), (-40.0, -90.0));
        assert_eq!((points.high[1], points.low[1]), (-80.0, -85.0));
    }

    #[test]
    fn a_zoomed_trace_runs_through_bin_centres() {
        let mut points = TracePoints::default();
        trace_points(&[-90.0, -60.0, -80.0], FULL_VIEW, 100.0, &mut points);
        assert_eq!(points.xs, [0.0, 50.0, 100.0]);
        assert_eq!(points.high, [-90.0, -60.0, -80.0]);
        let half = SpectrumView {
            start: 0.5,
            end: 1.0,
        };
        trace_points(
            &[-90.0, -60.0, -80.0, -70.0, -50.0],
            half,
            100.0,
            &mut points,
        );
        assert_eq!(points.xs[0], 0.0);
        assert_eq!(points.high[0], -80.0);
        assert_eq!(points.xs[points.count() - 1], 100.0);
        trace_points(&[-90.0], FULL_VIEW, 100.0, &mut points);
        assert_eq!(points.count(), 0);
    }

    #[test]
    fn columns_interpolate_between_bins_and_hold_at_the_ends() {
        let points = TracePoints {
            xs: vec![0.0, 4.0],
            low: vec![-80.0, -40.0],
            high: vec![-80.0, -40.0],
        };
        let (mut high, mut low) = (Vec::new(), Vec::new());
        columns_of(&points, 5, &mut high, &mut low);
        assert_eq!(high, [-75.0, -65.0, -55.0, -45.0, -40.0]);
        assert_eq!(low, high);
        columns_of(&TracePoints::default(), 2, &mut high, &mut low);
        assert!(high.iter().all(|value| value.is_nan()));
    }

    #[test]
    fn axis_labels_read_in_the_unit_the_span_needs() {
        assert_eq!(format_tick(100.25e6, 2e6), "100.25");
        assert_eq!(format_tick(100.25e6, 10e6), "100.2");
        assert_eq!(format_tick(100.0125e6, 20_000.0), "100.0125");
    }

    fn painted(grid: &DensityGrid, column: usize) -> Vec<usize> {
        (0..grid.height)
            .filter(|y| grid.cells[y * grid.width + column] > 0.0)
            .collect()
    }

    #[test]
    fn a_flat_trace_paints_its_own_row() {
        let mut grid = DensityGrid::new(4, 101);
        grid.add(&[-60.0; 4], FULL_VIEW, WINDOW, DENSITY_GAIN);
        for x in 0..4 {
            assert_eq!(painted(&grid, x), [50]);
        }
    }

    #[test]
    fn a_steep_edge_paints_a_continuous_segment() {
        let mut grid = DensityGrid::new(2, 101);
        grid.add(&[-100.0, -20.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        let rows = painted(&grid, 1);
        assert_eq!((rows[0], rows[rows.len() - 1], rows.len()), (0, 100, 101));
    }

    #[test]
    fn levels_outside_the_window_land_on_its_edges() {
        let mut grid = DensityGrid::new(1, 101);
        grid.add(&[40.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        assert_eq!(painted(&grid, 0), [0]);
        grid.clear();
        grid.add(&[-500.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        assert_eq!(painted(&grid, 0), [100]);
    }

    #[test]
    fn the_density_follows_a_zoomed_window() {
        let mut grid = DensityGrid::new(2, 101);
        let db = [-100.0, -100.0, -100.0, -20.0, -20.0, -20.0];
        grid.add(
            &db,
            SpectrumView {
                start: 0.6,
                end: 1.0,
            },
            WINDOW,
            DENSITY_GAIN,
        );
        assert_eq!(painted(&grid, 0), [0]);
        assert_eq!(painted(&grid, 1), [0]);
    }

    #[test]
    fn repeated_visits_accumulate_and_saturate() {
        let mut grid = DensityGrid::new(1, 3);
        grid.add(&[-20.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        assert!((grid.cells[0] - DENSITY_GAIN).abs() < 1e-6);
        for _ in 0..20 {
            grid.add(&[-20.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        }
        assert_eq!(grid.cells[0], 1.0);
        let mut empty = DensityGrid::new(2, 3);
        empty.add(&[], FULL_VIEW, WINDOW, DENSITY_GAIN);
        assert!(empty.cells.iter().all(|cell| *cell == 0.0));
    }

    #[test]
    fn decay_fades_a_cell_and_snaps_it_to_zero() {
        let mut grid = DensityGrid::new(1, 1);
        grid.add(&[-20.0], FULL_VIEW, WINDOW, DENSITY_GAIN);
        grid.decay(0.5);
        assert!((grid.cells[0] - DENSITY_GAIN / 2.0).abs() < 1e-6);
        for _ in 0..20 {
            grid.decay(0.5);
        }
        assert_eq!(grid.cells[0], 0.0);
    }

    #[test]
    fn the_image_is_transparent_where_empty_and_brightens_with_density() {
        let mut grid = DensityGrid::new(2, 1);
        let lut = Colormap::Gray.lut();
        let mut out = Vec::new();
        grid.to_image(&lut, &mut out);
        assert_eq!((out[3], out[7]), (0, 0));
        grid.cells[0] = 0.25;
        grid.cells[1] = 1.0;
        grid.to_image(&lut, &mut out);
        assert!(out[0] < out[4]);
        assert!(out[3] < out[7]);
        assert_eq!(out[7], 255);
    }
}
