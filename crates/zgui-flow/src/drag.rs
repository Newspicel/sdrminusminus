use kurbo::{Point, Size, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapGrid {
    pub x: f64,
    pub y: f64,
}

impl SnapGrid {
    #[must_use]
    pub const fn square(pitch: f64) -> Self {
        Self { x: pitch, y: pitch }
    }

    #[must_use]
    pub fn snap(self, point: Point) -> Point {
        let along = |value: f64, pitch: f64| {
            if pitch > 0.0 {
                (value / pitch).round() * pitch
            } else {
                value
            }
        };
        Point::new(along(point.x, self.x), along(point.y, self.y))
    }
}

pub const DRAG_THRESHOLD: f64 = 1.0;

#[must_use]
pub fn past_threshold(from: Point, to: Point, threshold: f64) -> bool {
    (to - from).hypot() > threshold
}

#[must_use]
pub fn dragged(origins: &[Point], by: Vec2, grid: Option<SnapGrid>) -> Vec<Point> {
    let Some(first) = origins.first() else {
        return Vec::new();
    };
    let shift = grid.map_or(Vec2::ZERO, |grid| {
        let moved = *first + by;
        grid.snap(moved) - moved
    });
    origins.iter().map(|origin| *origin + by + shift).collect()
}

fn edge_velocity(value: f64, low: f64, high: f64) -> f64 {
    if value < low {
        (low - value).clamp(1.0, low) / low
    } else if value > high {
        -(value - high).clamp(1.0, low) / low
    } else {
        0.0
    }
}

#[must_use]
pub fn auto_pan(pointer: Point, screen: Size, speed: f64, margin: f64) -> Vec2 {
    if screen.width <= margin * 2.0 || screen.height <= margin * 2.0 {
        return Vec2::ZERO;
    }
    Vec2::new(
        edge_velocity(pointer.x, margin, screen.width - margin) * speed,
        edge_velocity(pointer.y, margin, screen.height - margin) * speed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_rounds_to_the_nearest_grid_point() {
        let grid = SnapGrid::square(20.0);
        assert_eq!(grid.snap(Point::new(29.0, 31.0)), Point::new(20.0, 40.0));
        assert_eq!(
            SnapGrid { x: 0.0, y: 10.0 }.snap(Point::new(3.3, 4.0)),
            Point::new(3.3, 0.0)
        );
    }

    #[test]
    fn a_group_moves_together_and_snaps_by_its_first_node() {
        let origins = [Point::new(3.0, 3.0), Point::new(103.0, 53.0)];
        let moved = dragged(
            &origins,
            Vec2::new(10.0, 10.0),
            Some(SnapGrid::square(10.0)),
        );
        assert_eq!(moved[0], Point::new(10.0, 10.0));
        assert_eq!(moved[1] - moved[0], origins[1] - origins[0]);
    }

    #[test]
    fn a_jitter_is_not_a_drag() {
        let from = Point::new(10.0, 10.0);
        assert!(!past_threshold(
            from,
            Point::new(10.5, 10.5),
            DRAG_THRESHOLD
        ));
        assert!(past_threshold(from, Point::new(12.0, 10.0), DRAG_THRESHOLD));
    }

    #[test]
    fn the_pane_pans_towards_the_edge_the_pointer_is_near() {
        let screen = Size::new(800.0, 600.0);
        let still = auto_pan(Point::new(400.0, 300.0), screen, 15.0, 40.0);
        assert_eq!(still, Vec2::ZERO);
        let left = auto_pan(Point::new(5.0, 300.0), screen, 15.0, 40.0);
        assert!(left.x > 0.0 && left.y == 0.0);
        let bottom = auto_pan(Point::new(400.0, 599.0), screen, 15.0, 40.0);
        assert!(bottom.y < 0.0);
    }
}
