use std::collections::HashMap;

use sdrmm_wire::frame::FusionGridFrame;

use super::views::HeatBand;
use crate::records::LatLon;

pub(crate) const HEAT_LEVELS: [f32; 3] = [0.5, 0.8, 0.95];
pub(crate) const MAX_RING_POINTS: usize = 256;
const DECADES_LN: f64 = 9.21;

type Corner = (i32, i32);

pub(crate) fn weights(cells: &[u8]) -> Vec<f64> {
    cells
        .iter()
        .map(|cell| match *cell {
            0 => 0.0,
            value => ((f64::from(value) / 255.0 - 1.0) * DECADES_LN).exp(),
        })
        .collect()
}

pub(crate) fn bands(frame: &FusionGridFrame<'_>) -> Vec<HeatBand> {
    let (rows, cols) = (usize::from(frame.rows), usize::from(frame.cols));
    if rows == 0 || cols == 0 || frame.cells.len() != rows * cols {
        return Vec::new();
    }
    let weights = weights(frame.cells);
    let grid = Grid {
        rows,
        cols,
        south: frame.south,
        west: frame.west,
        north: frame.north,
        east: frame.east,
    };
    HEAT_LEVELS
        .iter()
        .filter_map(|level| {
            let threshold = threshold(&weights, f64::from(*level))?;
            let inside: Vec<bool> = weights
                .iter()
                .map(|weight| *weight > 0.0 && *weight >= threshold)
                .collect();
            let rings: Vec<Vec<LatLon>> = outer_loops(&inside, rows, cols)
                .into_iter()
                .map(|ring| grid.ring(&simplify(ring)))
                .collect();
            (!rings.is_empty()).then_some(HeatBand {
                level: *level,
                rings,
            })
        })
        .collect()
}

fn threshold(weights: &[f64], level: f64) -> Option<f64> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return None;
    }
    let mut sorted: Vec<f64> = weights
        .iter()
        .copied()
        .filter(|weight| *weight > 0.0)
        .collect();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let mut sum = 0.0;
    for weight in sorted {
        sum += weight;
        if sum >= level * total {
            return Some(weight);
        }
    }
    None
}

struct Grid {
    rows: usize,
    cols: usize,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
}

impl Grid {
    fn ring(&self, corners: &[Corner]) -> Vec<LatLon> {
        let lat_step = (self.north - self.south) / self.rows as f64;
        let lon_step = (self.east - self.west) / self.cols as f64;
        corners
            .iter()
            .map(|(x, y)| LatLon {
                lat: f64::from(*y).mul_add(-lat_step, self.north),
                lon: f64::from(*x).mul_add(lon_step, self.west),
            })
            .collect()
    }
}

fn boundary_edges(inside: &[bool], rows: usize, cols: usize) -> Vec<(Corner, Corner)> {
    let at = |row: i32, col: i32| {
        row >= 0
            && col >= 0
            && (row as usize) < rows
            && (col as usize) < cols
            && inside[row as usize * cols + col as usize]
    };
    let mut edges = Vec::new();
    for row in 0..rows as i32 {
        for col in 0..cols as i32 {
            if !at(row, col) {
                continue;
            }
            let (x, y) = (col, row);
            if !at(row - 1, col) {
                edges.push(((x, y), (x + 1, y)));
            }
            if !at(row, col + 1) {
                edges.push(((x + 1, y), (x + 1, y + 1)));
            }
            if !at(row + 1, col) {
                edges.push(((x + 1, y + 1), (x, y + 1)));
            }
            if !at(row, col - 1) {
                edges.push(((x, y + 1), (x, y)));
            }
        }
    }
    edges
}

fn outer_loops(inside: &[bool], rows: usize, cols: usize) -> Vec<Vec<Corner>> {
    let edges = boundary_edges(inside, rows, cols);
    let mut leaving: HashMap<Corner, Vec<usize>> = HashMap::new();
    for (index, (start, _)) in edges.iter().enumerate() {
        leaving.entry(*start).or_default().push(index);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for first in 0..edges.len() {
        if used[first] {
            continue;
        }
        let origin = edges[first].0;
        let mut ring = vec![origin];
        let mut current = first;
        loop {
            used[current] = true;
            let (start, end) = edges[current];
            if end == origin {
                break;
            }
            ring.push(end);
            let heading = (end.0 - start.0, end.1 - start.1);
            let Some(next) = next_edge(&edges, &leaving, &used, end, heading) else {
                break;
            };
            current = next;
        }
        ring.push(origin);
        if doubled_area(&ring) > 0 {
            loops.push(ring);
        }
    }
    loops
}

fn next_edge(
    edges: &[(Corner, Corner)],
    leaving: &HashMap<Corner, Vec<usize>>,
    used: &[bool],
    at: Corner,
    heading: (i32, i32),
) -> Option<usize> {
    let right = (-heading.1, heading.0);
    let candidates = leaving.get(&at)?;
    let direction = |index: usize| {
        let (start, end) = edges[index];
        (end.0 - start.0, end.1 - start.1)
    };
    let open = || candidates.iter().copied().filter(|index| !used[*index]);
    open()
        .find(|index| direction(*index) == right)
        .or_else(|| open().find(|index| direction(*index) == heading))
        .or_else(|| open().next())
}

fn doubled_area(ring: &[Corner]) -> i64 {
    ring.windows(2)
        .map(|pair| {
            i64::from(pair[0].0) * i64::from(pair[1].1)
                - i64::from(pair[1].0) * i64::from(pair[0].1)
        })
        .sum()
}

fn simplify(ring: Vec<Corner>) -> Vec<Corner> {
    let Some(&origin) = ring.first() else {
        return ring;
    };
    let open = &ring[..ring.len().saturating_sub(1)];
    let count = open.len();
    let mut corners: Vec<Corner> = (0..count)
        .filter(|index| {
            let previous = open[(index + count - 1) % count];
            let here = open[*index];
            let next = open[(index + 1) % count];
            (here.0 - previous.0) * (next.1 - here.1) != (here.1 - previous.1) * (next.0 - here.0)
        })
        .map(|index| open[index])
        .collect();
    if corners.is_empty() {
        corners.push(origin);
    }
    let limit = MAX_RING_POINTS - 1;
    if corners.len() > limit {
        let stride = corners.len().div_ceil(limit);
        corners = corners.into_iter().step_by(stride).collect();
    }
    let first = corners[0];
    corners.push(first);
    corners
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::frame::FusionGridFrame;

    use super::*;

    fn frame(rows: u16, cols: u16, cells: &[u8]) -> FusionGridFrame<'_> {
        FusionGridFrame {
            stream_id: 1,
            seq: 1,
            timestamp: 0,
            south: 52.0,
            west: 13.0,
            north: 53.0,
            east: 14.0,
            cols,
            rows,
            cells,
        }
    }

    fn gaussian(size: usize, centre: (f64, f64), sigma: f64) -> Vec<u8> {
        let mut cells = Vec::with_capacity(size * size);
        for row in 0..size {
            for col in 0..size {
                let (dx, dy) = (col as f64 - centre.0, row as f64 - centre.1);
                let log_p = -(dx * dx + dy * dy) / (2.0 * sigma * sigma);
                let value = 255.0 * (1.0 + log_p / DECADES_LN);
                cells.push(value.clamp(0.0, 255.0).round() as u8);
            }
        }
        cells
    }

    fn inside(ring: &[LatLon], point: LatLon) -> bool {
        let mut crossing = false;
        for pair in ring.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if (a.lat > point.lat) != (b.lat > point.lat) {
                let lon = (b.lon - a.lon) * (point.lat - a.lat) / (b.lat - a.lat) + a.lon;
                if point.lon < lon {
                    crossing = !crossing;
                }
            }
        }
        crossing
    }

    #[test]
    fn heat_bands_enclose_their_mass() {
        let size = 64;
        let cells = gaussian(size, (30.0, 20.0), 6.0);
        let frame = frame(size as u16, size as u16, &cells);
        let bands = bands(&frame);
        assert_eq!(
            bands.iter().map(|band| band.level).collect::<Vec<_>>(),
            HEAT_LEVELS
        );
        let weights = weights(&cells);
        let total: f64 = weights.iter().sum();
        let step = 1.0 / size as f64;
        for band in &bands {
            for ring in &band.rings {
                assert!(ring.len() <= MAX_RING_POINTS);
                assert_eq!(ring.first(), ring.last());
            }
            let enclosed: f64 = (0..size * size)
                .filter(|index| {
                    let centre = LatLon {
                        lat: 53.0 - ((index / size) as f64 + 0.5) * step,
                        lon: 13.0 + ((index % size) as f64 + 0.5) * step,
                    };
                    band.rings.iter().any(|ring| inside(ring, centre))
                })
                .map(|index| weights[index])
                .sum();
            let share = enclosed / total;
            let level = f64::from(band.level);
            assert!(share >= level && share < level + 0.05, "{level}: {share}");
        }
    }

    #[test]
    fn rows_run_from_the_north_edge() {
        let mut cells = vec![0u8; 16];
        cells[1] = 255;
        let bands = bands(&frame(4, 4, &cells));
        let ring = &bands[0].rings[0];
        assert_eq!(ring.len(), 5);
        let lats: Vec<f64> = ring.iter().map(|point| point.lat).collect();
        assert!(lats.iter().all(|lat| *lat >= 52.75 - 1e-9));
        assert!(
            ring.iter()
                .all(|point| point.lon >= 13.25 - 1e-9 && point.lon <= 13.5 + 1e-9)
        );
    }

    #[test]
    fn two_blobs_give_two_rings_and_an_empty_grid_none() {
        let mut cells = vec![0u8; 100];
        cells[11] = 255;
        cells[88] = 255;
        let bands = bands(&frame(10, 10, &cells));
        assert!(bands.iter().any(|band| band.rings.len() == 2));
        assert!(super::bands(&frame(10, 10, &[0u8; 100])).is_empty());
        assert!(super::bands(&frame(10, 10, &[0u8; 99])).is_empty());
    }

    #[test]
    fn a_ring_with_a_hole_keeps_only_its_outline() {
        let mut cells = vec![0u8; 25];
        for index in [6, 7, 8, 11, 13, 16, 17, 18] {
            cells[index] = 255;
        }
        let bands = bands(&frame(5, 5, &cells));
        assert!(bands.iter().all(|band| band.rings.len() == 1));
        assert_eq!(bands[0].rings[0].len(), 5);
    }
}
