use kurbo::{Point, Rect, Size, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomRange {
    pub min: f64,
    pub max: f64,
}

impl Default for ZoomRange {
    fn default() -> Self {
        Self { min: 0.5, max: 2.0 }
    }
}

impl ZoomRange {
    #[must_use]
    pub fn clamp(self, zoom: f64) -> f64 {
        zoom.clamp(self.min, self.max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Padding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Padding {
    #[must_use]
    pub const fn uniform(pixels: f64) -> Self {
        Self {
            top: pixels,
            right: pixels,
            bottom: pixels,
            left: pixels,
        }
    }

    #[must_use]
    pub fn fraction(fraction: f64, screen: Size) -> Self {
        let along = |extent: f64| ((extent - extent / (1.0 + fraction)) * 0.5).floor();
        let x = along(screen.width);
        let y = along(screen.height);
        Self {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }
}

impl Viewport {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
    };

    #[must_use]
    pub fn to_flow(self, screen: Point) -> Point {
        Point::new(
            (screen.x - self.x) / self.zoom,
            (screen.y - self.y) / self.zoom,
        )
    }

    #[must_use]
    pub fn to_screen(self, flow: Point) -> Point {
        Point::new(flow.x * self.zoom + self.x, flow.y * self.zoom + self.y)
    }

    #[must_use]
    pub fn rect_to_flow(self, screen: Rect) -> Rect {
        Rect::from_points(
            self.to_flow(screen.origin()),
            self.to_flow(Point::new(screen.x1, screen.y1)),
        )
    }

    #[must_use]
    pub fn rect_to_screen(self, flow: Rect) -> Rect {
        Rect::from_points(
            self.to_screen(flow.origin()),
            self.to_screen(Point::new(flow.x1, flow.y1)),
        )
    }

    #[must_use]
    pub fn visible(self, screen: Size) -> Rect {
        self.rect_to_flow(screen.to_rect())
    }

    #[must_use]
    pub fn panned(self, by: Vec2) -> Self {
        Self {
            x: self.x + by.x,
            y: self.y + by.y,
            zoom: self.zoom,
        }
    }

    #[must_use]
    pub fn zoomed_at(self, anchor: Point, zoom: f64, range: ZoomRange) -> Self {
        let zoom = range.clamp(zoom);
        let flow = self.to_flow(anchor);
        Self {
            x: anchor.x - flow.x * zoom,
            y: anchor.y - flow.y * zoom,
            zoom,
        }
    }

    #[must_use]
    pub fn scaled_at(self, anchor: Point, factor: f64, range: ZoomRange) -> Self {
        self.zoomed_at(anchor, self.zoom * factor, range)
    }

    #[must_use]
    pub fn centred_on(self, flow: Point, screen: Size) -> Self {
        Self {
            x: screen.width / 2.0 - flow.x * self.zoom,
            y: screen.height / 2.0 - flow.y * self.zoom,
            zoom: self.zoom,
        }
    }

    #[must_use]
    pub fn fitting(bounds: Rect, screen: Size, range: ZoomRange, padding: Padding) -> Self {
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return Self::IDENTITY.centred_on(bounds.center(), screen);
        }
        let x_zoom = (screen.width - padding.left - padding.right) / bounds.width();
        let y_zoom = (screen.height - padding.top - padding.bottom) / bounds.height();
        let zoom = range.clamp(x_zoom.min(y_zoom));
        let centre = bounds.center();
        let fitted = Self {
            x: screen.width / 2.0 - centre.x * zoom,
            y: screen.height / 2.0 - centre.y * zoom,
            zoom,
        };
        let drawn = fitted.rect_to_screen(bounds);
        let short = |room: f64, needed: f64| (room.floor() - needed).min(0.0);
        let left = short(drawn.x0, padding.left);
        let top = short(drawn.y0, padding.top);
        let right = short(screen.width - drawn.x1, padding.right);
        let bottom = short(screen.height - drawn.y1, padding.bottom);
        Self {
            x: fitted.x - left + right,
            y: fitted.y - top + bottom,
            zoom,
        }
    }

    #[must_use]
    pub fn lerp(self, to: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            x: self.x + (to.x - self.x) * t,
            y: self.y + (to.y - self.y) * t,
            zoom: self.zoom + (to.zoom - self.zoom) * t,
        }
    }
}

#[must_use]
pub fn ease_in_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0) * 2.0;
    if t <= 1.0 {
        t * t * t / 2.0
    } else {
        let t = t - 2.0;
        (t * t * t + 2.0) / 2.0
    }
}

#[must_use]
pub fn wheel_zoom_factor(delta_y: f64, pixels: bool, pinch: bool) -> f64 {
    let scale = if pixels { 0.002 } else { 0.05 };
    let boost = if pinch { 10.0 } else { 1.0 };
    2f64.powf(-delta_y * scale * boost)
}

#[must_use]
pub fn bounds_of(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    rects.into_iter().reduce(|joined, rect| joined.union(rect))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: ZoomRange = ZoomRange { min: 0.1, max: 4.0 };

    #[test]
    fn a_point_survives_the_trip_to_the_flow_and_back() {
        let viewport = Viewport {
            x: 120.0,
            y: -40.0,
            zoom: 1.5,
        };
        let screen = Point::new(310.0, 77.0);
        let back = viewport.to_screen(viewport.to_flow(screen));
        assert!((back - screen).hypot() < 1e-9);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_still() {
        let viewport = Viewport {
            x: 30.0,
            y: 10.0,
            zoom: 1.0,
        };
        let anchor = Point::new(200.0, 150.0);
        let before = viewport.to_flow(anchor);
        let after = viewport.scaled_at(anchor, 1.7, RANGE);
        assert!((after.to_flow(anchor) - before).hypot() < 1e-9);
        assert!((after.zoom - 1.7).abs() < 1e-12);
    }

    #[test]
    fn zoom_stops_at_its_limits() {
        let viewport = Viewport::IDENTITY.scaled_at(Point::ZERO, 100.0, RANGE);
        assert_eq!(viewport.zoom, RANGE.max);
        let viewport = Viewport::IDENTITY.scaled_at(Point::ZERO, 0.0001, RANGE);
        assert_eq!(viewport.zoom, RANGE.min);
    }

    #[test]
    fn fitting_centres_the_bounds_inside_the_padding() {
        let bounds = Rect::new(100.0, 100.0, 500.0, 300.0);
        let screen = Size::new(1000.0, 600.0);
        let padding = Padding::uniform(50.0);
        let viewport = Viewport::fitting(
            bounds,
            screen,
            ZoomRange {
                min: 0.1,
                max: 10.0,
            },
            padding,
        );
        let drawn = viewport.rect_to_screen(bounds);
        assert!(drawn.x0 >= padding.left - 1.0);
        assert!(drawn.y0 >= padding.top - 1.0);
        assert!(drawn.x1 <= screen.width - padding.right + 1.0);
        assert!(drawn.y1 <= screen.height - padding.bottom + 1.0);
        let centre = drawn.center();
        assert!((centre.x - 500.0).abs() < 1.0 && (centre.y - 300.0).abs() < 1.0);
    }

    #[test]
    fn fitting_respects_the_largest_zoom() {
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let viewport = Viewport::fitting(
            bounds,
            Size::new(800.0, 600.0),
            ZoomRange { min: 0.1, max: 1.0 },
            Padding::uniform(0.0),
        );
        assert_eq!(viewport.zoom, 1.0);
    }

    #[test]
    fn fitting_nothing_centres_on_the_point() {
        let viewport = Viewport::fitting(
            Rect::new(50.0, 50.0, 50.0, 50.0),
            Size::new(200.0, 100.0),
            RANGE,
            Padding::uniform(0.0),
        );
        assert_eq!(
            viewport.to_screen(Point::new(50.0, 50.0)),
            Point::new(100.0, 50.0)
        );
    }

    #[test]
    fn a_fractional_padding_matches_react_flow() {
        let padding = Padding::fraction(0.1, Size::new(1100.0, 550.0));
        assert_eq!(padding.left, 50.0);
        assert_eq!(padding.top, 25.0);
    }

    #[test]
    fn a_wheel_notch_up_zooms_in_and_down_zooms_out() {
        assert!(wheel_zoom_factor(-100.0, true, false) > 1.0);
        assert!(wheel_zoom_factor(100.0, true, false) < 1.0);
        let pinch = wheel_zoom_factor(-10.0, true, true);
        let wheel = wheel_zoom_factor(-10.0, true, false);
        assert!(pinch > wheel);
    }

    #[test]
    fn easing_starts_slow_ends_slow_and_passes_the_middle() {
        assert_eq!(ease_in_out_cubic(0.0), 0.0);
        assert_eq!(ease_in_out_cubic(1.0), 1.0);
        assert!((ease_in_out_cubic(0.5) - 0.5).abs() < 1e-12);
        assert!(ease_in_out_cubic(0.1) < 0.1);
        assert!(ease_in_out_cubic(0.9) > 0.9);
    }

    #[test]
    fn bounds_join_every_rect() {
        let joined = bounds_of([
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(-5.0, 4.0, 3.0, 20.0),
        ]);
        assert_eq!(joined, Some(Rect::new(-5.0, 0.0, 10.0, 20.0)));
        assert_eq!(bounds_of([]), None);
    }
}
