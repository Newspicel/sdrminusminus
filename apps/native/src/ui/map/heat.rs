pub const CELL_PX: f64 = 4.0;
pub const LEVELS: usize = 14;

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub values: Vec<f64>,
}

#[must_use]
pub fn density(
    points: &[(f64, f64, f64)],
    width: f64,
    height: f64,
    radius: f64,
    intensity: f64,
) -> Grid {
    let columns = (width / CELL_PX).ceil().max(0.0) as usize;
    let rows = (height / CELL_PX).ceil().max(0.0) as usize;
    let mut values = vec![0.0; columns * rows];
    let radius = radius.max(1.0);
    let reach = (radius / CELL_PX).ceil() as i64;
    for &(x, y, weight) in points {
        if !(x.is_finite() && y.is_finite()) || x < -radius || y < -radius {
            continue;
        }
        if x > width + radius || y > height + radius {
            continue;
        }
        let column = (x / CELL_PX).floor() as i64;
        let row = (y / CELL_PX).floor() as i64;
        for dy in -reach..=reach {
            let r = row + dy;
            if r < 0 || r >= rows as i64 {
                continue;
            }
            for dx in -reach..=reach {
                let c = column + dx;
                if c < 0 || c >= columns as i64 {
                    continue;
                }
                let cx = (c as f64 + 0.5) * CELL_PX - x;
                let cy = (r as f64 + 0.5) * CELL_PX - y;
                let d = (cx * cx + cy * cy) / (radius * radius);
                if d > 1.0 {
                    continue;
                }
                values[r as usize * columns + c as usize] += weight * intensity * (-4.5 * d).exp();
            }
        }
    }
    Grid {
        columns,
        rows,
        values,
    }
}

#[must_use]
pub fn level(value: f64) -> Option<usize> {
    let scaled = (value.clamp(0.0, 1.0) * LEVELS as f64).floor() as usize;
    (scaled > 0).then_some(scaled.min(LEVELS))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ramp {
    pub stops: Vec<(f64, u32, f32)>,
}

impl Ramp {
    #[must_use]
    pub fn at(&self, position: f64) -> (u32, f32) {
        let Some(first) = self.stops.first() else {
            return (0, 0.0);
        };
        if position <= first.0 {
            return (first.1, first.2);
        }
        for pair in self.stops.windows(2) {
            let (low, high) = (pair[0], pair[1]);
            if position <= high.0 {
                let fraction =
                    ((position - low.0) / (high.0 - low.0).max(f64::EPSILON)).clamp(0.0, 1.0);
                return (
                    mix(low.1, high.1, fraction),
                    low.2 + (high.2 - low.2) * fraction as f32,
                );
            }
        }
        self.stops.last().map_or((0, 0.0), |last| (last.1, last.2))
    }
}

#[must_use]
pub fn mix(from: u32, to: u32, fraction: f64) -> u32 {
    let fraction = fraction.clamp(0.0, 1.0);
    [16u32, 8, 0].into_iter().fold(0, |out, shift| {
        let a = f64::from((from >> shift) & 0xff);
        let b = f64::from((to >> shift) & 0xff);
        out | (((a + (b - a) * fraction).round() as u32) << shift)
    })
}

#[must_use]
pub fn interpolate(stops: &[(f64, f64)], at: f64) -> f64 {
    let Some(first) = stops.first() else {
        return 0.0;
    };
    if at <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        if at <= pair[1].0 {
            let fraction = (at - pair[0].0) / (pair[1].0 - pair[0].0).max(f64::EPSILON);
            return pair[0].1 + (pair[1].1 - pair[0].1) * fraction;
        }
    }
    stops.last().map_or(0.0, |last| last.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_warms_its_own_cell_most() {
        let grid = density(&[(10.0, 10.0, 1.0)], 40.0, 40.0, 12.0, 1.0);
        assert_eq!((grid.columns, grid.rows), (10, 10));
        let at = |c: usize, r: usize| grid.values[r * grid.columns + c];
        assert!(at(2, 2) > at(3, 2));
        assert!(at(3, 2) > at(4, 2));
        assert_eq!(at(9, 9), 0.0);
    }

    #[test]
    fn nearby_points_add_up() {
        let one = density(&[(10.0, 10.0, 1.0)], 40.0, 40.0, 12.0, 1.0);
        let two = density(
            &[(10.0, 10.0, 1.0), (10.0, 10.0, 1.0)],
            40.0,
            40.0,
            12.0,
            1.0,
        );
        assert!((two.values[2 * 10 + 2] - 2.0 * one.values[2 * 10 + 2]).abs() < 1e-12);
    }

    #[test]
    fn nothing_is_nothing_and_full_is_the_top_level() {
        assert_eq!(level(0.0), None);
        assert_eq!(level(1.0), Some(LEVELS));
        assert_eq!(level(7.0), Some(LEVELS));
    }

    #[test]
    fn a_ramp_pins_its_ends_and_blends_between() {
        let ramp = Ramp {
            stops: vec![(0.0, 0x000000, 0.0), (1.0, 0xffffff, 1.0)],
        };
        assert_eq!(ramp.at(-1.0), (0x000000, 0.0));
        assert_eq!(ramp.at(2.0), (0xffffff, 1.0));
        assert_eq!(ramp.at(0.5).0, 0x808080);
        assert_eq!(interpolate(&[(0.0, 4.0), (6.0, 14.0)], 3.0), 9.0);
        assert_eq!(interpolate(&[(0.0, 4.0), (6.0, 14.0)], 9.0), 14.0);
    }
}
