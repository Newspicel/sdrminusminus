use kurbo::{BezPath, Line, ParamCurve, ParamCurveNearest, PathSeg, Point, Rect, Shape};

const ACCURACY: f64 = 0.05;

#[must_use]
pub fn distance_to_path(path: &BezPath, point: Point) -> f64 {
    path.segments()
        .map(|segment| distance_to_segment(segment, point))
        .fold(f64::INFINITY, f64::min)
}

fn distance_to_segment(segment: PathSeg, point: Point) -> f64 {
    segment.nearest(point, ACCURACY).distance_sq.sqrt()
}

#[must_use]
pub fn path_hit(path: &BezPath, point: Point, reach: f64) -> bool {
    path.bounding_box().inflate(reach, reach).contains(point)
        && distance_to_path(path, point) <= reach
}

#[must_use]
pub fn path_crosses(path: &BezPath, area: Rect) -> bool {
    let bounds = path.bounding_box();
    if bounds.x1 < area.x0 || bounds.x0 > area.x1 || bounds.y1 < area.y0 || bounds.y0 > area.y1 {
        return false;
    }
    let corners = [
        Point::new(area.x0, area.y0),
        Point::new(area.x1, area.y0),
        Point::new(area.x1, area.y1),
        Point::new(area.x0, area.y1),
    ];
    let sides: Vec<Line> = (0..4)
        .map(|at| Line::new(corners[at], corners[(at + 1) % 4]))
        .collect();
    path.segments().any(|segment| {
        area.contains(segment.start())
            || area.contains(segment.end())
            || sides
                .iter()
                .any(|side| !segment.intersect_line(*side).is_empty())
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Partial,
    Full,
}

#[must_use]
pub fn caught(node: Rect, area: Rect, mode: SelectionMode) -> bool {
    let overlap = node.intersect(area);
    let overlapping = overlap.width() > 0.0 && overlap.height() > 0.0;
    match mode {
        SelectionMode::Partial => overlapping,
        SelectionMode::Full => overlapping && overlap.area() >= node.area() - 1e-9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{Endpoints, Side, bezier};

    fn wire() -> BezPath {
        bezier(
            Endpoints {
                source: Point::new(0.0, 0.0),
                source_side: Side::Right,
                target: Point::new(200.0, 0.0),
                target_side: Side::Left,
            },
            0.25,
        )
        .path
    }

    #[test]
    fn a_point_on_a_wire_is_on_it_and_one_beside_it_is_not() {
        let path = wire();
        assert!(distance_to_path(&path, Point::new(100.0, 0.0)) < 1e-6);
        assert!(path_hit(&path, Point::new(100.0, 4.0), 5.0));
        assert!(!path_hit(&path, Point::new(100.0, 20.0), 5.0));
    }

    #[test]
    fn a_box_across_a_wire_catches_it() {
        let path = wire();
        assert!(path_crosses(&path, Rect::new(90.0, -10.0, 110.0, 10.0)));
        assert!(!path_crosses(&path, Rect::new(90.0, 20.0, 110.0, 40.0)));
    }

    #[test]
    fn partial_selection_needs_any_overlap_and_full_needs_all_of_it() {
        let node = Rect::new(0.0, 0.0, 100.0, 50.0);
        let half = Rect::new(50.0, -10.0, 200.0, 100.0);
        let around = Rect::new(-10.0, -10.0, 200.0, 100.0);
        let apart = Rect::new(150.0, 0.0, 200.0, 50.0);
        assert!(caught(node, half, SelectionMode::Partial));
        assert!(!caught(node, half, SelectionMode::Full));
        assert!(caught(node, around, SelectionMode::Full));
        assert!(!caught(node, apart, SelectionMode::Partial));
    }

    #[test]
    fn touching_edges_do_not_count_as_overlap() {
        let node = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert!(!caught(
            node,
            Rect::new(100.0, 0.0, 150.0, 50.0),
            SelectionMode::Partial
        ));
    }
}
