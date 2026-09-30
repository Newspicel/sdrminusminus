use sdrmm_wire::DfEstimate;

use super::{
    grid::{CELLS, LogGrid, POSTERIOR_SPAN, SHIFT_STEP},
    votes::Candidate,
};

pub(crate) const BASIN_M: f64 = 3_000.0;
pub(crate) const CONVERGED_MAJOR_M: f64 = 400.0;
pub(crate) const CONVERGED_MASS: f32 = 0.6;
pub(crate) const CONVERGED_SAMPLES: u32 = 6;
pub(crate) const MIN_ESTIMATE_SAMPLES: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Located {
    pub(crate) estimate: DfEstimate,
    pub(crate) east_m: f64,
    pub(crate) north_m: f64,
}

#[derive(Default)]
struct Moments {
    total: f64,
    east: f64,
    north: f64,
    ee: f64,
    nn: f64,
    en: f64,
}

impl Moments {
    fn add(&mut self, east: f64, north: f64, weight: f64) {
        self.total += weight;
        self.east += weight * east;
        self.north += weight * north;
        self.ee += weight * east * east;
        self.nn += weight * north * north;
        self.en += weight * east * north;
    }
}

pub(crate) fn converged(estimate: &DfEstimate) -> bool {
    estimate.ellipse_major_m <= CONVERGED_MAJOR_M
        && estimate.mass >= CONVERGED_MASS
        && estimate.samples >= CONVERGED_SAMPLES
}

fn locate(
    grid: &LogGrid,
    moments: &Moments,
    origin: (f64, f64),
    mass: f32,
    samples: u32,
) -> Option<Located> {
    if moments.total.is_nan() || moments.total <= 0.0 {
        return None;
    }
    let mean_east = moments.east / moments.total;
    let mean_north = moments.north / moments.total;
    let cell_variance = grid.cell_m() * grid.cell_m() / 12.0;
    let cee = (moments.ee / moments.total - mean_east * mean_east).max(0.0) + cell_variance;
    let cnn = (moments.nn / moments.total - mean_north * mean_north).max(0.0) + cell_variance;
    let cen = moments.en / moments.total - mean_east * mean_north;
    let half_trace = (cee + cnn) / 2.0;
    let root = ((cee - cnn) / 2.0).hypot(cen);
    let major = 2.0 * (half_trace + root).max(0.0).sqrt();
    let minor = 2.0 * (half_trace - root).max(0.0).sqrt();
    let axis = (2.0 * cen).atan2(cee - cnn).to_degrees() / 2.0;
    let east_m = origin.0 + mean_east;
    let north_m = origin.1 + mean_north;
    let at = grid.enu().to_latlon(east_m, north_m);
    let mut estimate = DfEstimate {
        lat: at.lat,
        lon: at.lon,
        ellipse_major_m: major,
        ellipse_minor_m: minor,
        ellipse_bearing_deg: (90.0 - axis).rem_euclid(180.0),
        converged: false,
        samples,
        mass: mass.clamp(0.0, 1.0),
    };
    estimate.converged = converged(&estimate);
    Some(Located {
        estimate,
        east_m,
        north_m,
    })
}

pub(crate) fn global(grid: &LogGrid, samples: u32) -> Option<Located> {
    if samples < MIN_ESTIMATE_SAMPLES {
        return None;
    }
    let (peak, top) = grid.max();
    if !top.is_finite() {
        return None;
    }
    let origin = grid.index_centre(peak);
    let mut all = 0.0f64;
    let mut basin = Moments::default();
    for (index, &value) in grid.values().iter().enumerate() {
        let weight = f64::from(value - top).exp();
        all += weight;
        if value < top - POSTERIOR_SPAN {
            continue;
        }
        let (east, north) = grid.index_centre(index);
        let (de, dn) = (east - origin.0, north - origin.1);
        if de.hypot(dn) <= BASIN_M {
            basin.add(de, dn, weight);
        }
    }
    let mass = if all > 0.0 { basin.total / all } else { 0.0 };
    locate(grid, &basin, origin, mass as f32, samples)
}

struct Window {
    centre: (f64, f64),
    radius_m: f64,
    votes: f32,
}

fn windows(candidates: &[Candidate]) -> Vec<Window> {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let nearest = candidates
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, other)| {
                    (other.east_m - candidate.east_m).hypot(other.north_m - candidate.north_m)
                })
                .fold(f64::INFINITY, f64::min);
            Window {
                centre: (candidate.east_m, candidate.north_m),
                radius_m: BASIN_M.min(nearest / 2.0),
                votes: candidate.votes,
            }
        })
        .collect()
}

fn refine(grid: &LogGrid, window: &Window, mass: f32, samples: u32) -> Option<Located> {
    let reach = (window.radius_m / grid.cell_m()).ceil() as usize + 1;
    let inside: Vec<(usize, f64, f64)> = window_cells(grid, window.centre, reach)
        .filter_map(|index| {
            let (east, north) = grid.index_centre(index);
            let (de, dn) = (east - window.centre.0, north - window.centre.1);
            (de.hypot(dn) <= window.radius_m).then_some((index, east, north))
        })
        .collect();
    let values = grid.values();
    let (_, local) = inside
        .iter()
        .map(|&(index, _, _)| (index, values[index]))
        .fold((0, f32::NEG_INFINITY), |best, next| {
            if next.1 > best.1 { next } else { best }
        });
    if !local.is_finite() {
        return None;
    }
    let mut moments = Moments::default();
    for &(index, east, north) in &inside {
        let value = values[index];
        if value >= local - POSTERIOR_SPAN {
            moments.add(
                east - window.centre.0,
                north - window.centre.1,
                f64::from(value - local).exp(),
            );
        }
    }
    locate(grid, &moments, window.centre, mass, samples)
}

fn window_cells(grid: &LogGrid, centre: (f64, f64), reach: usize) -> impl Iterator<Item = usize> {
    let (grid_east, grid_north) = grid.centre();
    let middle = (CELLS as f64 - 1.0) / 2.0;
    let col = ((centre.0 - grid_east) / grid.cell_m() + middle).round();
    let row = (middle - (centre.1 - grid_north) / grid.cell_m()).round();
    let span = |at: f64| {
        let low = (at - reach as f64).clamp(0.0, CELLS as f64 - 1.0) as usize;
        let high = (at + reach as f64).clamp(0.0, CELLS as f64 - 1.0) as usize;
        low..=high
    };
    let cols = span(col);
    span(row).flat_map(move |row| cols.clone().map(move |col| row * CELLS + col))
}

pub(crate) fn emitters(
    grid: &LogGrid,
    candidates: &[Candidate],
    global: Option<&Located>,
    max: usize,
    samples: u32,
) -> Vec<DfEstimate> {
    let windows = windows(candidates);
    let total: f32 = windows.iter().map(|window| window.votes).sum();
    let mut found: Vec<(Located, &Window)> = windows
        .iter()
        .filter_map(|window| {
            let mass = if total > 0.0 {
                window.votes / total
            } else {
                0.0
            };
            refine(grid, window, mass, samples).map(|located| (located, window))
        })
        .collect();
    found.sort_by(|a, b| b.0.estimate.mass.total_cmp(&a.0.estimate.mass));
    let distinct = SHIFT_STEP as f64 * grid.cell_m();
    let mut unique: Vec<(Located, &Window)> = Vec::with_capacity(found.len());
    for (located, window) in found {
        if unique.iter().all(|(kept, _)| {
            (kept.east_m - located.east_m).hypot(kept.north_m - located.north_m) > distinct
        }) {
            unique.push((located, window));
        }
    }
    let mut list: Vec<DfEstimate> = Vec::with_capacity(unique.len() + 1);
    if let Some(global) = global {
        let holder = unique.iter().position(|(_, window)| {
            (global.east_m - window.centre.0).hypot(global.north_m - window.centre.1)
                <= window.radius_m
        });
        match holder {
            Some(index) => {
                let (first, _) = unique.remove(index);
                list.push(first.estimate);
            }
            None => list.push(global.estimate),
        }
    }
    list.extend(unique.into_iter().map(|(located, _)| located.estimate));
    list.truncate(max);
    list
}
