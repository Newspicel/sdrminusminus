use kurbo::{Point, Rect, Size};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Grip {
    Top,
    Right,
    Bottom,
    Left,
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Grip {
    pub const ALL: [Self; 8] = [
        Self::Top,
        Self::Right,
        Self::Bottom,
        Self::Left,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomRight,
        Self::BottomLeft,
    ];

    const fn horizontal(self) -> bool {
        !matches!(self, Self::Top | Self::Bottom)
    }

    const fn vertical(self) -> bool {
        !matches!(self, Self::Left | Self::Right)
    }

    const fn moves_left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    const fn moves_top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    #[must_use]
    pub const fn cursor(self) -> &'static str {
        match self {
            Self::Top | Self::Bottom => "ns-resize",
            Self::Left | Self::Right => "ew-resize",
            Self::TopLeft | Self::BottomRight => "nwse-resize",
            Self::TopRight | Self::BottomLeft => "nesw-resize",
        }
    }

    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomRight => "bottom-right",
            Self::BottomLeft => "bottom-left",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub min: Size,
    pub max: Size,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            min: Size::new(10.0, 10.0),
            max: Size::new(f64::MAX, f64::MAX),
        }
    }
}

fn clamp_amount(size: f64, min: f64, max: f64) -> f64 {
    (min - size).max(size - max).max(0.0)
}

#[must_use]
pub fn resized(
    start: Rect,
    grip: Grip,
    from: Point,
    to: Point,
    limits: Limits,
    keep_aspect: bool,
) -> Rect {
    let mut dx = if grip.horizontal() {
        (to.x - from.x).floor()
    } else {
        0.0
    };
    let mut dy = if grip.vertical() {
        (to.y - from.y).floor()
    } else {
        0.0
    };
    let mut moves_left = grip.moves_left();
    let mut moves_top = grip.moves_top();
    let width = start.width() + if moves_left { -dx } else { dx };
    let height = start.height() + if moves_top { -dy } else { dy };
    let aspect = if start.height() > 0.0 {
        start.width() / start.height()
    } else {
        1.0
    };

    let mut clamp_x = clamp_amount(width, limits.min.width, limits.max.width);
    let mut clamp_y = clamp_amount(height, limits.min.height, limits.max.height);
    if keep_aspect {
        if grip.horizontal() {
            clamp_x = clamp_x
                .max(clamp_amount(width / aspect, limits.min.height, limits.max.height) * aspect);
        }
        if grip.vertical() {
            clamp_y = clamp_y
                .max(clamp_amount(height * aspect, limits.min.width, limits.max.width) / aspect);
        }
    }
    dx += if dx < 0.0 { clamp_x } else { -clamp_x };
    dy += if dy < 0.0 { clamp_y } else { -clamp_y };

    if keep_aspect {
        if grip.horizontal() && grip.vertical() {
            if width > height * aspect {
                dy = if moves_left != moves_top { -dx } else { dx } / aspect;
            } else {
                dx = if moves_left != moves_top { -dy } else { dy } * aspect;
            }
        } else if grip.horizontal() {
            dy = dx / aspect;
            moves_top = moves_left;
        } else {
            dx = dy * aspect;
            moves_left = moves_top;
        }
    }

    let x = if moves_left { start.x0 + dx } else { start.x0 };
    let y = if moves_top { start.y0 + dy } else { start.y0 };
    let width = start.width() + if moves_left { -dx } else { dx };
    let height = start.height() + if moves_top { -dy } else { dy };
    Rect::from_origin_size((x, y), (width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: Rect = Rect::new(100.0, 100.0, 300.0, 200.0);

    fn limits(min: (f64, f64), max: (f64, f64)) -> Limits {
        Limits {
            min: min.into(),
            max: max.into(),
        }
    }

    #[test]
    fn the_bottom_right_grip_grows_without_moving_the_origin() {
        let out = resized(
            START,
            Grip::BottomRight,
            Point::new(300.0, 200.0),
            Point::new(350.0, 260.0),
            Limits::default(),
            false,
        );
        assert_eq!(out, Rect::new(100.0, 100.0, 350.0, 260.0));
    }

    #[test]
    fn the_top_left_grip_moves_the_origin_and_keeps_the_far_corner() {
        let out = resized(
            START,
            Grip::TopLeft,
            Point::new(100.0, 100.0),
            Point::new(80.0, 70.0),
            Limits::default(),
            false,
        );
        assert_eq!(out, Rect::new(80.0, 70.0, 300.0, 200.0));
    }

    #[test]
    fn an_edge_grip_changes_one_dimension_only() {
        let out = resized(
            START,
            Grip::Right,
            Point::new(300.0, 150.0),
            Point::new(340.0, 999.0),
            Limits::default(),
            false,
        );
        assert_eq!(out, Rect::new(100.0, 100.0, 340.0, 200.0));
    }

    #[test]
    fn a_node_never_shrinks_below_its_minimum() {
        let out = resized(
            START,
            Grip::Left,
            Point::new(100.0, 150.0),
            Point::new(290.0, 150.0),
            limits((50.0, 50.0), (1000.0, 1000.0)),
            false,
        );
        assert_eq!(out.width(), 50.0);
        assert_eq!(out.x1, 300.0);
    }

    #[test]
    fn a_node_never_grows_past_its_maximum() {
        let out = resized(
            START,
            Grip::Bottom,
            Point::new(200.0, 200.0),
            Point::new(200.0, 900.0),
            limits((10.0, 10.0), (500.0, 250.0)),
            false,
        );
        assert_eq!(out.height(), 250.0);
    }

    #[test]
    fn keeping_the_aspect_scales_both_sides_together() {
        let out = resized(
            START,
            Grip::Right,
            Point::new(300.0, 150.0),
            Point::new(500.0, 150.0),
            Limits::default(),
            true,
        );
        assert!((out.width() / out.height() - 2.0).abs() < 1e-9);
        assert_eq!(out.width(), 400.0);
    }

    #[test]
    fn every_grip_has_its_own_cursor_class() {
        for grip in Grip::ALL {
            assert!(!grip.class().is_empty());
            assert!(grip.cursor().ends_with("-resize"));
        }
    }
}
