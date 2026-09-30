use crate::special::{brent_max, norm_deg, wrap_deg};

pub const MAX_CANDIDATES: usize = 16;
pub const REFINE_TOL_DEG: f64 = 0.01;
pub const REFINE_ITERATIONS: u32 = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Candidate {
    pub point: usize,
    pub power: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridShape {
    pub azimuths: usize,
    pub elevations: usize,
    pub wraps: bool,
}

pub fn local_maxima(
    spectrum: &[f32],
    shape: GridShape,
    range_db: f32,
    out: &mut [Candidate],
) -> usize {
    let points = (shape.azimuths * shape.elevations).min(spectrum.len());
    let spectrum = &spectrum[..points];
    let top = spectrum
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    if !top.is_finite() {
        return 0;
    }
    let floor = top * 10f32.powf(-range_db.max(0.0) / 10.0);
    let mut count = 0;
    for point in 0..points {
        let power = spectrum[point];
        if power.is_finite() && power >= floor && is_local_max(spectrum, shape, point) {
            count = insert(out, count, Candidate { point, power });
        }
    }
    count
}

fn is_local_max(spectrum: &[f32], shape: GridShape, point: usize) -> bool {
    let power = spectrum[point];
    let azimuth = point % shape.azimuths;
    let elevation = point / shape.azimuths;
    for d_el in -1i64..=1 {
        let Some(row) = step(elevation, d_el, shape.elevations, false) else {
            continue;
        };
        for d_az in -1i64..=1 {
            if d_el == 0 && d_az == 0 {
                continue;
            }
            let Some(column) = step(azimuth, d_az, shape.azimuths, shape.wraps) else {
                continue;
            };
            let other = row * shape.azimuths + column;
            if other == point {
                continue;
            }
            let neighbour = spectrum[other];
            if neighbour > power || (neighbour == power && other < point) {
                return false;
            }
        }
    }
    true
}

fn step(index: usize, delta: i64, len: usize, wraps: bool) -> Option<usize> {
    let moved = index as i64 + delta;
    let len = len as i64;
    if wraps {
        Some(moved.rem_euclid(len) as usize)
    } else {
        (0..len).contains(&moved).then_some(moved as usize)
    }
}

fn insert(out: &mut [Candidate], count: usize, candidate: Candidate) -> usize {
    let capacity = out.len();
    let mut slot = count.min(capacity);
    while slot > 0 && out[slot - 1].power < candidate.power {
        slot -= 1;
    }
    if slot >= capacity {
        return count;
    }
    let end = count.min(capacity - 1);
    out.copy_within(slot..end, slot + 1);
    out[slot] = candidate;
    (count + 1).min(capacity)
}

#[must_use]
pub fn mirror_deg(azimuth_deg: f64, axis_deg: f64) -> f64 {
    norm_deg(2.0 * axis_deg - azimuth_deg)
}

#[must_use]
pub fn on_side(azimuth_deg: f64, centre_deg: f64) -> bool {
    wrap_deg(azimuth_deg - centre_deg).abs() <= 90.0
}

#[must_use]
pub fn apart_deg(a: f64, b: f64) -> f64 {
    wrap_deg(a - b).abs()
}

pub fn refine(f: impl FnMut(f64) -> f64, centre: f64, half_width: f64) -> (f64, f64) {
    brent_max(
        f,
        centre - half_width,
        centre + half_width,
        REFINE_TOL_DEG,
        REFINE_ITERATIONS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RING: GridShape = GridShape {
        azimuths: 8,
        elevations: 1,
        wraps: true,
    };

    #[test]
    fn ring_maxima_wrap_and_sort_by_power() {
        let spectrum = [9.0, 1.0, 2.0, 5.0, 2.0, 1.0, 3.0, 4.0];
        let mut out = [Candidate::default(); 4];
        let count = local_maxima(&spectrum, RING, 30.0, &mut out);
        assert_eq!(count, 2);
        assert_eq!(
            out[0],
            Candidate {
                point: 0,
                power: 9.0
            }
        );
        assert_eq!(
            out[1],
            Candidate {
                point: 3,
                power: 5.0
            }
        );
        let half = GridShape {
            wraps: false,
            ..RING
        };
        let edges = [3.0, 2.0, 1.0, 1.5, 0.5, 0.2, 0.3, 4.0];
        let count = local_maxima(&edges, half, 30.0, &mut out);
        assert_eq!(count, 3);
        assert_eq!(out[0].point, 7);
        assert_eq!(out[1].point, 0);
        assert_eq!(out[2].point, 3);
    }

    #[test]
    fn range_and_capacity_limit_the_candidates() {
        let spectrum = [100.0, 1.0, 50.0, 1.0, 2.0, 1.0, 20.0, 1.0];
        let mut out = [Candidate::default(); 4];
        assert_eq!(local_maxima(&spectrum, RING, 5.0, &mut out), 2);
        assert_eq!(local_maxima(&spectrum, RING, 30.0, &mut out), 4);
        let mut two = [Candidate::default(); 2];
        assert_eq!(local_maxima(&spectrum, RING, 30.0, &mut two), 2);
        assert_eq!(two[0].point, 0);
        assert_eq!(two[1].point, 2);
        assert_eq!(local_maxima(&[f32::NAN; 8], RING, 30.0, &mut out), 0);
    }

    #[test]
    fn plateaus_give_one_maximum() {
        let spectrum = [1.0, 3.0, 3.0, 1.0, 1.0, 1.0, 1.0, 0.0];
        let mut out = [Candidate::default(); 4];
        let count = local_maxima(&spectrum, RING, 3.0, &mut out);
        assert_eq!(count, 1);
        assert_eq!(out[0].point, 1);
    }

    #[test]
    fn elevation_rows_use_eight_neighbours() {
        let shape = GridShape {
            azimuths: 4,
            elevations: 3,
            wraps: true,
        };
        let mut spectrum = [0.0f32; 12];
        spectrum[5] = 4.0;
        spectrum[10] = 3.0;
        spectrum[3] = 2.0;
        let mut out = [Candidate::default(); 4];
        let count = local_maxima(&spectrum, shape, 30.0, &mut out);
        assert_eq!(count, 2);
        assert_eq!(out[0].point, 5);
        assert_eq!(out[1].point, 3);
    }

    #[test]
    fn mirror_and_side_follow_the_axis() {
        assert!((mirror_deg(30.0, 90.0) - 150.0).abs() < 1e-12);
        assert!((mirror_deg(60.0, 180.0) - 300.0).abs() < 1e-12);
        assert!(on_side(30.0, 0.0));
        assert!(on_side(270.0, 0.0));
        assert!(!on_side(150.0, 0.0));
        assert!((apart_deg(359.0, 1.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn refinement_finds_an_off_grid_maximum() {
        let (x, fx) = refine(|x| -(x - 37.37) * (x - 37.37), 37.0, 1.0);
        assert!((x - 37.37).abs() < 0.01, "{x}");
        assert!(fx > -1e-4);
    }
}
