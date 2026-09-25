use zgui::{
    canvas::{CanvasScene, ShapeBuilder},
    elements::{kurbo, kurbo::Shape as _},
};

use super::{
    geo::{Geo, View},
    heat::{self, Ramp},
    paint::solid,
};

pub const EDGE: u32 = 0x16_18_1b;
pub const WHITE: u32 = 0xff_ff_ff;
pub const HIT_SLOP_PX: f64 = 9.0;

pub type Stops = Vec<(f64, f64)>;

#[derive(Clone, Debug, PartialEq)]
pub struct Heat {
    pub points: Vec<(Geo, f64)>,
    pub radius: Stops,
    pub intensity: Stops,
    pub opacity: Stops,
    pub ramp: Ramp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub points: Vec<Geo>,
    pub colour: u32,
    pub alpha: f32,
    pub width: f64,
    pub dash: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub ring: Vec<Geo>,
    pub fill: u32,
    pub fill_alpha: f32,
    pub stroke: Option<(u32, f32, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dot {
    pub at: Geo,
    pub radius: Stops,
    pub fill: u32,
    pub alpha: f32,
    pub stroke: u32,
    pub stroke_width: f64,
    pub pick: Option<String>,
    pub min_zoom: f64,
}

impl Dot {
    #[must_use]
    pub fn plain(at: Geo, radius: f64, fill: u32) -> Self {
        Self {
            at,
            radius: vec![(0.0, radius)],
            fill,
            alpha: 1.0,
            stroke: EDGE,
            stroke_width: 1.0,
            pick: None,
            min_zoom: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Plane,
    Ship,
    Arrow,
    Station,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub at: Geo,
    pub glyph: Glyph,
    pub heading: f64,
    pub colour: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub key: String,
    pub at: Geo,
    pub text: String,
    pub colour: u32,
    pub below_px: f64,
    pub min_zoom: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub heat: Vec<Heat>,
    pub polygons: Vec<Polygon>,
    pub lines: Vec<Line>,
    pub dots: Vec<Dot>,
    pub marks: Vec<Mark>,
    pub labels: Vec<Label>,
}

impl Overlay {
    pub fn extend(&mut self, other: Self) {
        self.heat.extend(other.heat);
        self.polygons.extend(other.polygons);
        self.lines.extend(other.lines);
        self.dots.extend(other.dots);
        self.marks.extend(other.marks);
        self.labels.extend(other.labels);
    }
}

fn polyline(points: &[(f64, f64)], close: bool) -> kurbo::BezPath {
    let mut path = kurbo::BezPath::new();
    let mut iter = points.iter();
    if let Some(first) = iter.next() {
        path.move_to(*first);
        for point in iter {
            path.line_to(*point);
        }
        if close {
            path.close_path();
        }
    }
    path
}

pub fn draw(scene: &mut CanvasScene, view: &View, overlay: &Overlay) {
    for heat in &overlay.heat {
        draw_heat(scene, view, heat);
    }
    for polygon in &overlay.polygons {
        let path = polyline(&view.line(&polygon.ring), true);
        scene.push(
            ShapeBuilder::new(path.clone())
                .fill(solid(polygon.fill, polygon.fill_alpha))
                .build(),
        );
        if let Some((colour, alpha, width)) = polygon.stroke {
            scene.push(
                ShapeBuilder::new(path)
                    .stroke(solid(colour, alpha), width)
                    .build(),
            );
        }
    }
    for line in &overlay.lines {
        draw_line(scene, view, line);
    }
    for dot in &overlay.dots {
        if view.zoom < dot.min_zoom {
            continue;
        }
        let (x, y) = view.screen(dot.at);
        let radius = heat::interpolate(&dot.radius, view.zoom);
        let path = kurbo::Circle::new((x, y), radius).to_path(0.1);
        let mut shape = ShapeBuilder::new(path).fill(solid(dot.fill, dot.alpha));
        if dot.stroke_width > 0.0 {
            shape = shape.stroke(solid(dot.stroke, 1.0), dot.stroke_width);
        }
        scene.push(shape.build());
    }
    for mark in &overlay.marks {
        draw_mark(scene, view, mark);
    }
}

fn draw_line(scene: &mut CanvasScene, view: &View, line: &Line) {
    if line.points.len() < 2 {
        return;
    }
    let path = polyline(&view.line(&line.points), false);
    let brush = solid(line.colour, line.alpha);
    let shape = match line.dash {
        Some([on, off]) => {
            let stroke = kurbo::Stroke::new(line.width)
                .with_dashes(0.0, [on * line.width, off * line.width])
                .with_caps(kurbo::Cap::Butt);
            ShapeBuilder::new(path).stroke_styled(brush, stroke)
        }
        None => ShapeBuilder::new(path).stroke(brush, line.width),
    };
    scene.push(shape.build());
}

fn draw_heat(scene: &mut CanvasScene, view: &View, layer: &Heat) {
    let points: Vec<(f64, f64, f64)> = layer
        .points
        .iter()
        .map(|(at, weight)| {
            let (x, y) = view.screen(*at);
            (x, y, *weight)
        })
        .collect();
    let radius = heat::interpolate(&layer.radius, view.zoom);
    let intensity = heat::interpolate(&layer.intensity, view.zoom);
    let opacity = heat::interpolate(&layer.opacity, view.zoom) as f32;
    let grid = heat::density(&points, view.width, view.height, radius, intensity);
    let mut levels: Vec<kurbo::BezPath> = vec![kurbo::BezPath::new(); heat::LEVELS + 1];
    for row in 0..grid.rows {
        for column in 0..grid.columns {
            let Some(level) = heat::level(grid.values[row * grid.columns + column]) else {
                continue;
            };
            let (x, y) = (column as f64 * heat::CELL_PX, row as f64 * heat::CELL_PX);
            let rect = kurbo::Rect::new(x, y, x + heat::CELL_PX, y + heat::CELL_PX);
            levels[level].extend(rect.to_path(0.1));
        }
    }
    for (level, path) in levels.into_iter().enumerate() {
        if path.elements().is_empty() {
            continue;
        }
        let (colour, alpha) = layer.ramp.at(level as f64 / heat::LEVELS as f64);
        scene.push(
            ShapeBuilder::new(path)
                .fill(solid(colour, alpha * opacity))
                .build(),
        );
    }
}

const PLANE: (f64, &[(f64, f64)]) = (
    26.0,
    &[
        (13.0, 1.6),
        (14.4, 4.2),
        (14.4, 9.4),
        (24.4, 14.2),
        (24.4, 16.2),
        (14.4, 13.6),
        (14.4, 19.2),
        (18.6, 21.8),
        (18.6, 23.4),
        (13.6, 22.4),
        (13.0, 23.6),
    ],
);

const SHIP: (f64, &[(f64, f64)]) = (
    22.0,
    &[
        (11.0, 1.8),
        (15.6, 7.4),
        (15.6, 17.2),
        (14.2, 19.8),
        (11.0, 19.8),
    ],
);

const ARROW: (f64, &[(f64, f64)]) = (18.0, &[(9.0, 1.5), (12.5, 8.0), (9.0, 6.5)]);

fn silhouette(px: f64, starboard: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut outline: Vec<(f64, f64)> = starboard.to_vec();
    outline.extend(starboard.iter().rev().map(|(x, y)| (px - x, *y)));
    outline
}

#[must_use]
pub fn glyph_outline(glyph: Glyph, x: f64, y: f64, heading: f64) -> Vec<(f64, f64)> {
    let (px, starboard) = match glyph {
        Glyph::Plane => PLANE,
        Glyph::Ship => SHIP,
        Glyph::Arrow | Glyph::Station => ARROW,
    };
    let (sin, cos) = heading.to_radians().sin_cos();
    let mid = px / 2.0;
    silhouette(px, starboard)
        .into_iter()
        .map(|(px, py)| {
            let (dx, dy) = (px - mid, py - mid);
            (x + dx * cos - dy * sin, y + dx * sin + dy * cos)
        })
        .collect()
}

fn draw_mark(scene: &mut CanvasScene, view: &View, mark: &Mark) {
    let (x, y) = view.screen(mark.at);
    if mark.glyph == Glyph::Station {
        let ring = kurbo::Circle::new((x, y), 6.0).to_path(0.1);
        scene.push(
            ShapeBuilder::new(ring.clone())
                .stroke(solid(EDGE, 1.0), 3.5)
                .build(),
        );
        scene.push(
            ShapeBuilder::new(ring)
                .stroke(solid(mark.colour, 1.0), 1.6)
                .build(),
        );
        let pip = kurbo::Circle::new((x, y), 1.7).to_path(0.1);
        scene.push(ShapeBuilder::new(pip).fill(solid(mark.colour, 1.0)).build());
        return;
    }
    let path = polyline(&glyph_outline(mark.glyph, x, y, mark.heading), true);
    scene.push(
        ShapeBuilder::new(path)
            .fill(solid(mark.colour, 1.0))
            .stroke(solid(EDGE, 1.0), 1.0)
            .build(),
    );
}

#[must_use]
pub fn pick(overlay: &Overlay, view: &View, sx: f64, sy: f64) -> Option<String> {
    overlay
        .dots
        .iter()
        .filter_map(|dot| {
            let id = dot.pick.as_ref()?;
            let (x, y) = view.screen(dot.at);
            let distance = (x - sx).hypot(y - sy);
            let reach = heat::interpolate(&dot.radius, view.zoom) + HIT_SLOP_PX;
            (distance <= reach).then_some((distance, id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id.clone())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub key: String,
    pub text: String,
    pub colour: u32,
    pub x: f64,
    pub y: f64,
}

#[must_use]
pub fn placed_labels(overlay: &Overlay, view: &View, cap: usize) -> Vec<Placed> {
    overlay
        .labels
        .iter()
        .filter(|label| view.zoom >= label.min_zoom)
        .filter_map(|label| {
            let (x, y) = view.screen(label.at);
            let inside =
                (-40.0..view.width + 40.0).contains(&x) && (-20.0..view.height + 20.0).contains(&y);
            inside.then(|| Placed {
                key: label.key.clone(),
                text: label.text.clone(),
                colour: label.colour,
                x,
                y: y + label.below_px,
            })
        })
        .take(cap)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .centred(Geo::new(50.0, 8.0), 8.0)
    }

    #[test]
    fn a_click_picks_the_nearest_target_within_the_slop() {
        let near = Geo::new(50.0, 8.0);
        let overlay = Overlay {
            dots: vec![
                Dot {
                    pick: Some("a".to_owned()),
                    ..Dot::plain(near, 4.0, WHITE)
                },
                Dot {
                    pick: Some("b".to_owned()),
                    ..Dot::plain(Geo::new(50.2, 8.2), 4.0, WHITE)
                },
                Dot::plain(near, 4.0, WHITE),
            ],
            ..Overlay::default()
        };
        assert_eq!(pick(&overlay, &view(), 205.0, 150.0).as_deref(), Some("a"));
        assert_eq!(pick(&overlay, &view(), 250.0, 150.0), None);
    }

    #[test]
    fn a_glyph_turns_with_its_heading() {
        let north = glyph_outline(Glyph::Arrow, 0.0, 0.0, 0.0);
        let east = glyph_outline(Glyph::Arrow, 0.0, 0.0, 90.0);
        assert!(north[0].1 < 0.0 && north[0].0.abs() < 1e-9);
        assert!(east[0].0 > 0.0 && east[0].1.abs() < 1e-9);
        assert_eq!(glyph_outline(Glyph::Plane, 0.0, 0.0, 0.0).len(), 22);
    }

    #[test]
    fn labels_off_screen_are_left_out() {
        let label = |key: &str, at| Label {
            key: key.to_owned(),
            at,
            text: key.to_owned(),
            colour: WHITE,
            below_px: 10.0,
            min_zoom: 0.0,
        };
        let overlay = Overlay {
            labels: vec![
                label("in", Geo::new(50.0, 8.0)),
                label("out", Geo::new(10.0, 80.0)),
            ],
            ..Overlay::default()
        };
        let placed = placed_labels(&overlay, &view(), 10);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].key, "in");
        assert!((placed[0].y - 160.0).abs() < 1e-6);
    }
}
