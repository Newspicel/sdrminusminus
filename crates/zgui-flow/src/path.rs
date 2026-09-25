use kurbo::{BezPath, Point, Vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Side {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    #[must_use]
    pub const fn direction(self) -> Vec2 {
        match self {
            Self::Left => Vec2::new(-1.0, 0.0),
            Self::Right => Vec2::new(1.0, 0.0),
            Self::Top => Vec2::new(0.0, -1.0),
            Self::Bottom => Vec2::new(0.0, 1.0),
        }
    }

    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    #[must_use]
    pub const fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Endpoints {
    pub source: Point,
    pub source_side: Side,
    pub target: Point,
    pub target_side: Side,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgePath {
    pub path: BezPath,
    pub label: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeShape {
    Bezier { curvature: f64 },
    SmoothStep { radius: f64, offset: f64 },
    Step { offset: f64 },
    Straight,
}

impl Default for EdgeShape {
    fn default() -> Self {
        Self::Bezier { curvature: 0.25 }
    }
}

impl EdgeShape {
    #[must_use]
    pub fn route(self, ends: Endpoints) -> EdgePath {
        match self {
            Self::Bezier { curvature } => bezier(ends, curvature),
            Self::SmoothStep { radius, offset } => smooth_step(ends, radius, offset),
            Self::Step { offset } => smooth_step(ends, 0.0, offset),
            Self::Straight => straight(ends.source, ends.target),
        }
    }
}

fn control_offset(distance: f64, curvature: f64) -> f64 {
    if distance >= 0.0 {
        0.5 * distance
    } else {
        curvature * 25.0 * (-distance).sqrt()
    }
}

fn control(side: Side, from: Point, to: Point, curvature: f64) -> Point {
    match side {
        Side::Left => Point::new(from.x - control_offset(from.x - to.x, curvature), from.y),
        Side::Right => Point::new(from.x + control_offset(to.x - from.x, curvature), from.y),
        Side::Top => Point::new(from.x, from.y - control_offset(from.y - to.y, curvature)),
        Side::Bottom => Point::new(from.x, from.y + control_offset(to.y - from.y, curvature)),
    }
}

#[must_use]
pub fn bezier(ends: Endpoints, curvature: f64) -> EdgePath {
    let first = control(ends.source_side, ends.source, ends.target, curvature);
    let second = control(ends.target_side, ends.target, ends.source, curvature);
    let mut path = BezPath::new();
    path.move_to(ends.source);
    path.curve_to(first, second, ends.target);
    let label = Point::new(
        ends.source.x * 0.125 + first.x * 0.375 + second.x * 0.375 + ends.target.x * 0.125,
        ends.source.y * 0.125 + first.y * 0.375 + second.y * 0.375 + ends.target.y * 0.125,
    );
    EdgePath { path, label }
}

#[must_use]
pub fn straight(source: Point, target: Point) -> EdgePath {
    let mut path = BezPath::new();
    path.move_to(source);
    path.line_to(target);
    EdgePath {
        path,
        label: source.midpoint(target),
    }
}

fn along(point: Point, horizontal: bool) -> f64 {
    if horizontal { point.x } else { point.y }
}

fn step_points(ends: Endpoints, offset: f64) -> (Vec<Point>, Point) {
    let source_dir = ends.source_side.direction();
    let target_dir = ends.target_side.direction();
    let source_gap = ends.source + source_dir * offset;
    let target_gap = ends.target + target_dir * offset;
    let horizontal = ends.source_side.horizontal();
    let heading = if horizontal {
        if source_gap.x < target_gap.x {
            1.0
        } else {
            -1.0
        }
    } else if source_gap.y < target_gap.y {
        1.0
    } else {
        -1.0
    };
    let component = |dir: Vec2| if horizontal { dir.x } else { dir.y };
    let mut source_shift = Vec2::ZERO;
    let mut target_shift = Vec2::ZERO;
    let bends: Vec<Point>;
    let label: Point;

    if component(source_dir) * component(target_dir) == -1.0 {
        let centre = source_gap.midpoint(target_gap);
        let vertical_split = vec![
            Point::new(centre.x, source_gap.y),
            Point::new(centre.x, target_gap.y),
        ];
        let horizontal_split = vec![
            Point::new(source_gap.x, centre.y),
            Point::new(target_gap.x, centre.y),
        ];
        bends = if component(source_dir) == heading {
            if horizontal {
                vertical_split
            } else {
                horizontal_split
            }
        } else if horizontal {
            horizontal_split
        } else {
            vertical_split
        };
        label = centre;
    } else {
        let source_target = vec![Point::new(source_gap.x, target_gap.y)];
        let target_source = vec![Point::new(target_gap.x, source_gap.y)];
        let mut chosen = if horizontal {
            if source_dir.x == heading {
                target_source.clone()
            } else {
                source_target.clone()
            }
        } else if source_dir.y == heading {
            source_target.clone()
        } else {
            target_source.clone()
        };
        if ends.source_side == ends.target_side {
            let gap = (along(ends.source, horizontal) - along(ends.target, horizontal)).abs();
            if gap <= offset {
                let shift = (offset - 1.0).min(offset - gap);
                let axis = |amount: f64| {
                    if horizontal {
                        Vec2::new(amount, 0.0)
                    } else {
                        Vec2::new(0.0, amount)
                    }
                };
                if component(source_dir) == heading {
                    let sign = if along(source_gap, horizontal) > along(ends.source, horizontal) {
                        -1.0
                    } else {
                        1.0
                    };
                    source_shift = axis(sign * shift);
                } else {
                    let sign = if along(target_gap, horizontal) > along(ends.target, horizontal) {
                        -1.0
                    } else {
                        1.0
                    };
                    target_shift = axis(sign * shift);
                }
            }
        } else {
            let across = |point: Point| if horizontal { point.y } else { point.x };
            let same = component(source_dir)
                == if horizontal {
                    target_dir.y
                } else {
                    target_dir.x
                };
            let greater = across(source_gap) > across(target_gap);
            let less = across(source_gap) < across(target_gap);
            let flip = (component(source_dir) == 1.0 && ((!same && greater) || (same && less)))
                || (component(source_dir) != 1.0 && ((!same && less) || (same && greater)));
            if flip {
                chosen = if horizontal {
                    source_target
                } else {
                    target_source
                };
            }
        }
        let source_point = source_gap + source_shift;
        let target_point = target_gap + target_shift;
        let corner = chosen[0];
        let x_reach = (source_point.x - corner.x)
            .abs()
            .max((target_point.x - corner.x).abs());
        let y_reach = (source_point.y - corner.y)
            .abs()
            .max((target_point.y - corner.y).abs());
        label = if x_reach >= y_reach {
            Point::new((source_point.x + target_point.x) / 2.0, corner.y)
        } else {
            Point::new(corner.x, (source_point.y + target_point.y) / 2.0)
        };
        bends = chosen;
    }

    let gapped_source = source_gap + source_shift;
    let gapped_target = target_gap + target_shift;
    let mut points = vec![ends.source];
    if bends.first() != Some(&gapped_source) {
        points.push(gapped_source);
    }
    points.extend(bends.iter().copied());
    if bends.last() != Some(&gapped_target) {
        points.push(gapped_target);
    }
    points.push(ends.target);
    (points, label)
}

#[must_use]
pub fn smooth_step(ends: Endpoints, radius: f64, offset: f64) -> EdgePath {
    let (points, label) = step_points(ends, offset);
    let mut path = BezPath::new();
    path.move_to(points[0]);
    for window in points.windows(3) {
        bend(&mut path, window[0], window[1], window[2], radius);
    }
    if let Some(last) = points.last() {
        path.line_to(*last);
    }
    EdgePath { path, label }
}

fn bend(path: &mut BezPath, a: Point, b: Point, c: Point, radius: f64) {
    let size = (a.distance(b) / 2.0).min(b.distance(c) / 2.0).min(radius);
    if (a.x == b.x && b.x == c.x) || (a.y == b.y && b.y == c.y) || size <= 0.0 {
        path.line_to(b);
        return;
    }
    if a.y == b.y {
        let x_dir = if a.x < c.x { -1.0 } else { 1.0 };
        let y_dir = if a.y < c.y { 1.0 } else { -1.0 };
        path.line_to((b.x + size * x_dir, b.y));
        path.quad_to(b, Point::new(b.x, b.y + size * y_dir));
    } else {
        let x_dir = if a.x < c.x { 1.0 } else { -1.0 };
        let y_dir = if a.y < c.y { -1.0 } else { 1.0 };
        path.line_to((b.x, b.y + size * y_dir));
        path.quad_to(b, Point::new(b.x + size * x_dir, b.y));
    }
}

#[cfg(test)]
mod tests {
    use kurbo::{PathEl, Shape};

    use super::*;

    fn ends(source: (f64, f64), target: (f64, f64)) -> Endpoints {
        Endpoints {
            source: source.into(),
            source_side: Side::Right,
            target: target.into(),
            target_side: Side::Left,
        }
    }

    #[test]
    fn a_forward_bezier_reaches_half_the_gap_from_each_end() {
        let routed = bezier(ends((0.0, 0.0), (200.0, 100.0)), 0.25);
        let PathEl::CurveTo(first, second, end) = routed.path.elements()[1] else {
            panic!("one cubic");
        };
        assert_eq!(first, Point::new(100.0, 0.0));
        assert_eq!(second, Point::new(100.0, 100.0));
        assert_eq!(end, Point::new(200.0, 100.0));
        assert_eq!(routed.label, Point::new(100.0, 50.0));
    }

    #[test]
    fn a_backward_bezier_loops_out_by_the_square_root_of_the_gap() {
        let routed = bezier(ends((100.0, 0.0), (0.0, 0.0)), 0.25);
        let PathEl::CurveTo(first, second, _) = routed.path.elements()[1] else {
            panic!("one cubic");
        };
        assert!((first.x - (100.0 + 0.25 * 25.0 * 10.0)).abs() < 1e-9);
        assert!((second.x + 0.25 * 25.0 * 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_straight_edge_is_one_line_labelled_in_the_middle() {
        let routed = straight(Point::new(0.0, 0.0), Point::new(10.0, 20.0));
        assert_eq!(routed.path.elements().len(), 2);
        assert_eq!(routed.label, Point::new(5.0, 10.0));
    }

    #[test]
    fn a_step_edge_only_runs_along_the_axes() {
        let routed = smooth_step(ends((0.0, 0.0), (200.0, 120.0)), 0.0, 20.0);
        let mut last = Point::ZERO;
        for element in routed.path.elements() {
            match *element {
                PathEl::MoveTo(point) => last = point,
                PathEl::LineTo(point) => {
                    assert!(
                        point.x == last.x || point.y == last.y,
                        "{last:?} -> {point:?}"
                    );
                    last = point;
                }
                _ => {}
            }
        }
        assert_eq!(last, Point::new(200.0, 120.0));
    }

    #[test]
    fn a_smooth_step_rounds_its_corners() {
        let routed = smooth_step(ends((0.0, 0.0), (200.0, 120.0)), 8.0, 20.0);
        let rounded = routed
            .path
            .elements()
            .iter()
            .filter(|element| matches!(element, PathEl::QuadTo(..)))
            .count();
        assert_eq!(rounded, 2);
        assert_eq!(routed.label, Point::new(100.0, 60.0));
    }

    #[test]
    fn a_backward_step_detours_around_both_ends() {
        let routed = smooth_step(ends((200.0, 0.0), (0.0, 100.0)), 0.0, 20.0);
        let bounds = routed.path.bounding_box();
        assert!(bounds.x1 >= 220.0 - 1e-9);
        assert!(bounds.x0 <= -20.0 + 1e-9);
    }

    #[test]
    fn every_shape_starts_and_ends_on_its_handles() {
        let points = ends((10.0, 20.0), (300.0, -40.0));
        for shape in [
            EdgeShape::default(),
            EdgeShape::Straight,
            EdgeShape::Step { offset: 20.0 },
            EdgeShape::SmoothStep {
                radius: 5.0,
                offset: 20.0,
            },
        ] {
            let routed = shape.route(points);
            let elements = routed.path.elements();
            assert_eq!(elements[0], PathEl::MoveTo(points.source));
            assert_eq!(
                elements.last().and_then(PathEl::end_point),
                Some(points.target)
            );
        }
    }

    #[test]
    fn opposite_sides_face_each_other() {
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            assert_eq!(side.direction(), -side.opposite().direction());
        }
    }
}
