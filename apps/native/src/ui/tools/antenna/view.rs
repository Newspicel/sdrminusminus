use sdrmm_wire::tools::{AntennaReport, AntennaSegmentRole};
use zgui::{
    canvas::ShapeBuilder,
    elements::{kurbo, kurbo::Shape as _},
    prelude::*,
};

use super::model::{
    Angles, ISOMETRIC, Mode, Point2, ScaleBar, Unit, Viewport, bounds_of, fit_to, format_length,
    ground_grid, place, plan_view, project, role_label, role_style, roles_in, scale_bar,
    structure_bounds, turn,
};
use crate::ui::{
    tools::kit::{Paint, button, local_point},
    widgets::segments,
};

pub const WIDTH: f64 = 640.0;
pub const HEIGHT: f64 = 320.0;
const ANNOTATION_BAND: f64 = 56.0;
const VIEWPORT: Viewport = Viewport {
    width: WIDTH,
    height: HEIGHT - ANNOTATION_BAND,
    padding: 34.0,
};
const DIMENSION_RULE: f64 = HEIGHT - 40.0;
const RULER_LINE: f64 = HEIGHT - 12.0;
const HOVER_REACH: f64 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub from: Point2,
    pub to: Point2,
    pub ink: Paint,
    pub width: f64,
    pub dash: Option<[f64; 2]>,
    pub alpha: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub label: String,
    pub from: Point2,
    pub to: Point2,
    pub length_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub start: f64,
    pub end: f64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub strokes: Vec<Stroke>,
    pub pieces: Vec<Piece>,
    pub feed: Point2,
    pub ruler: ScaleBar,
    pub across: Option<Rule>,
    pub down: Option<Rule>,
    pub title: &'static str,
}

fn plain(from: Point2, to: Point2, ink: Paint, width: f64) -> Stroke {
    Stroke {
        from,
        to,
        ink,
        width,
        dash: None,
        alpha: 1.0,
    }
}

fn at(x: f64, y: f64) -> Point2 {
    Point2 { x, y }
}

#[must_use]
pub fn drawing(
    report: &AntennaReport,
    mode: Mode,
    orbit: Angles,
    highlight: Option<&str>,
    unit: Unit,
) -> Drawing {
    let geometry = &report.geometry;
    let bounds = bounds_of(geometry);
    let plan = plan_view(&bounds);
    let angles = if mode == Mode::Plan {
        plan.angles
    } else {
        orbit
    };
    let grid = if mode == Mode::Orbit {
        ground_grid(&bounds)
    } else {
        Vec::new()
    };
    let mut projected: Vec<Point2> = Vec::new();
    for (from, to) in &grid {
        projected.extend([project(from, angles), project(to, angles)]);
    }
    for segment in &geometry.segments {
        projected.extend([project(&segment.from, angles), project(&segment.to, angles)]);
    }
    let fit = fit_to(&projected, VIEWPORT);
    let to_screen = |point| place(project(point, angles), fit);

    let mut strokes: Vec<Stroke> = grid
        .iter()
        .map(|(from, to)| plain(to_screen(from), to_screen(to), Paint::Line, 1.0))
        .collect();
    let mut pieces = Vec::with_capacity(geometry.segments.len());
    for segment in &geometry.segments {
        let style = role_style(segment.role);
        let lit = highlight == Some(segment.label.as_str());
        let dimmed = highlight.is_some() && !lit;
        let from = to_screen(&segment.from);
        let to = to_screen(&segment.to);
        strokes.push(Stroke {
            from,
            to,
            ink: style.ink,
            width: if lit { style.width + 2.0 } else { style.width },
            dash: style.dash,
            alpha: if dimmed { 0.3 } else { 1.0 },
        });
        pieces.push(Piece {
            label: segment.label.clone(),
            from,
            to,
            length_m: segment.length_m(),
        });
    }

    let (across, down) = if mode == Mode::Plan {
        dimensions(report, &plan, angles, fit, unit, &mut strokes)
    } else {
        (None, None)
    };
    let ruler = scale_bar(fit.scale, VIEWPORT.width / 4.0, unit);
    strokes.push(plain(
        at(16.0, RULER_LINE),
        at(16.0 + ruler.pixels, RULER_LINE),
        Paint::InkFaint,
        1.5,
    ));

    Drawing {
        strokes,
        pieces,
        feed: to_screen(&geometry.feed),
        ruler,
        across,
        down,
        title: if mode == Mode::Plan {
            plan.label
        } else {
            "Orbit: drag to turn"
        },
    }
}

fn dimensions(
    report: &AntennaReport,
    plan: &super::model::PlanView,
    angles: Angles,
    fit: super::model::Fit,
    unit: Unit,
    strokes: &mut Vec<Stroke>,
) -> (Option<Rule>, Option<Rule>) {
    let geometry = &report.geometry;
    let drawn: Vec<Point2> = geometry
        .segments
        .iter()
        .filter(|segment| segment.role != AntennaSegmentRole::Feedline)
        .flat_map(|segment| [segment.from, segment.to])
        .map(|point| place(project(&point, angles), fit))
        .collect();
    if drawn.is_empty() {
        return (None, None);
    }
    let left = drawn
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let right = drawn
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let top = drawn
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let bottom = drawn
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let metres = structure_bounds(geometry);
    let across_m = metres.along(plan.horizontal).size;
    let down_m = metres.along(plan.vertical).size;
    let rule = DIMENSION_RULE;
    let across = (across_m > 0.0).then(|| {
        strokes.push(plain(at(left, rule), at(right, rule), Paint::InkFaint, 1.0));
        strokes.push(plain(
            at(left, rule - 4.0),
            at(left, rule + 4.0),
            Paint::InkFaint,
            1.0,
        ));
        strokes.push(plain(
            at(right, rule - 4.0),
            at(right, rule + 4.0),
            Paint::InkFaint,
            1.0,
        ));
        Rule {
            start: left,
            end: right,
            label: format_length(across_m, unit),
        }
    });
    let down = (down_m > 0.0).then(|| {
        strokes.push(plain(at(20.0, top), at(20.0, bottom), Paint::InkFaint, 1.0));
        strokes.push(plain(at(16.0, top), at(24.0, top), Paint::InkFaint, 1.0));
        strokes.push(plain(
            at(16.0, bottom),
            at(24.0, bottom),
            Paint::InkFaint,
            1.0,
        ));
        Rule {
            start: top,
            end: bottom,
            label: format_length(down_m, unit),
        }
    });
    (across, down)
}

#[must_use]
pub fn distance_to(point: Point2, from: Point2, to: Point2) -> f64 {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let squared = dx * dx + dy * dy;
    let along = if squared > 0.0 {
        (((point.x - from.x) * dx + (point.y - from.y) * dy) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (nearest_x, nearest_y) = (from.x + along * dx, from.y + along * dy);
    (point.x - nearest_x).hypot(point.y - nearest_y)
}

#[must_use]
pub fn piece_at(pieces: &[Piece], point: Point2, reach: f64) -> Option<&Piece> {
    pieces
        .iter()
        .map(|piece| (piece, distance_to(point, piece.from, piece.to)))
        .filter(|(_, distance)| *distance <= reach)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(piece, _)| piece)
}

fn segment_path(from: Point2, to: Point2) -> kurbo::BezPath {
    let mut path = kurbo::BezPath::new();
    path.move_to((from.x, from.y));
    path.line_to((to.x, to.y));
    path
}

fn paint(scene: &mut zgui::canvas::CanvasScene, drawing: &Drawing) {
    for stroke in &drawing.strokes {
        let mut style = kurbo::Stroke::new(stroke.width);
        if let Some([on, off]) = stroke.dash {
            style = style.with_dashes(0.0, [on, off]);
        }
        scene.push(
            ShapeBuilder::new(segment_path(stroke.from, stroke.to))
                .stroke_styled(stroke.ink.brush(stroke.alpha), style)
                .build(),
        );
    }
    let feed = kurbo::Circle::new((drawing.feed.x, drawing.feed.y), 4.5).to_path(0.1);
    scene.push(
        ShapeBuilder::new(feed)
            .fill(Paint::Bg.brush(1.0))
            .stroke(Paint::Accent.brush(1.0), 2.0)
            .build(),
    );
}

pub struct Inputs {
    pub report: Memo<Option<AntennaReport>>,
    pub unit: RwSignal<Unit>,
    pub highlight: RwSignal<Option<String>>,
    pub mode: RwSignal<Mode>,
    pub orbit: RwSignal<Angles>,
}

pub fn antenna_view(inputs: Inputs) -> impl IntoView {
    let Inputs {
        report,
        unit,
        highlight,
        mode,
        orbit,
    } = inputs;
    let shown = Memo::new(move |_| {
        report.with(|report| {
            report.as_ref().map(|report| {
                drawing(
                    report,
                    mode.get(),
                    orbit.get(),
                    highlight.get().as_deref(),
                    unit.get(),
                )
            })
        })
    });
    let canvas = zgui::elements::canvas()
        .class("ant-draw__canvas")
        .draw(move |cx| {
            shown.with(|drawing| {
                if let Some(drawing) = drawing {
                    paint(cx.scene, drawing);
                }
            });
        })
        .into_view();
    let hovered = move || {
        let label = highlight.get()?;
        shown.with(|drawing| {
            let piece = drawing
                .as_ref()?
                .pieces
                .iter()
                .find(|piece| piece.label == label)?;
            Some(format!(
                "{}: {}",
                piece.label,
                format_length(piece.length_m, unit.get())
            ))
        })
    };
    view! {
        column(class = "ant-view") {
            row(class = "ant-view__head") {
                text(class = "legend") {{move || shown.with(|drawing| drawing.as_ref().map_or("", |drawing| drawing.title))}}
                text(class = "ant-view__hover") {{move || hovered().unwrap_or_default()}}
                spacer() {}
                {move || (mode.get() == Mode::Orbit).then(|| AnyView::new(button("btn", || "Reset angle".to_owned(), || false, move || orbit.set(ISOMETRIC))))}
                {segments(vec![(Mode::Plan, "2D"), (Mode::Orbit, "3D")], mode.into(), move |picked| mode.set(picked))}
            }
            {surface(canvas, shown, highlight, mode, orbit)}
            {legend(report)}
        }
    }
}

fn surface(
    canvas: impl IntoView + 'static,
    shown: Memo<Option<Drawing>>,
    highlight: RwSignal<Option<String>>,
    mode: RwSignal<Mode>,
    orbit: RwSignal<Angles>,
) -> impl IntoView {
    let area = NodeRef::new();
    let drag = RwSignal::new(None::<(f32, f32)>);
    let press = move |ev: &mut EventCx<'_, events::PointerDown>| {
        ev.stop_propagation();
        if ev.button != Some(PointerButton::Primary) || mode.get_untracked() != Mode::Orbit {
            return;
        }
        ev.capture_pointer();
        drag.set(Some((ev.position.x.0, ev.position.y.0)));
    };
    let hover = move |ev: &mut EventCx<'_, events::PointerMove>| {
        if let Some((x, y)) = drag.get_untracked() {
            let (nx, ny) = (ev.position.x.0, ev.position.y.0);
            drag.set(Some((nx, ny)));
            orbit.update(|angles| *angles = turn(*angles, f64::from(nx - x), f64::from(ny - y)));
            return;
        }
        let Some((x, y, width, _)) = local_point(area, ev.position) else {
            return;
        };
        let factor = if width > 0.0 { WIDTH / width } else { 1.0 };
        let point = at(x * factor, y * factor);
        let found = shown.with_untracked(|drawing| {
            drawing
                .as_ref()
                .and_then(|drawing| piece_at(&drawing.pieces, point, HOVER_REACH))
                .map(|piece| piece.label.clone())
        });
        if found != highlight.get_untracked() {
            highlight.set(found);
        }
    };
    let release = move |ev: &mut EventCx<'_, events::PointerUp>| {
        ev.release_pointer();
        drag.set(None);
    };
    view! {
        box(
            node_ref = area,
            class = "ant-draw",
            class:orbit = move || mode.get() == Mode::Orbit,
            on:pointer_down = press,
            on:pointer_move = hover,
            on:pointer_up = release,
            on:pointer_cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
                ev.release_pointer();
                drag.set(None);
            },
            on:pointer_leave = move |_| if drag.get_untracked().is_none() { highlight.set(None) }
        ) {
            {canvas}
            {move || labels(shown)}
        }
    }
}

fn labels(shown: Memo<Option<Drawing>>) -> Vec<AnyView> {
    shown.with(|drawing| {
        let Some(drawing) = drawing else {
            return Vec::new();
        };
        let mut views = vec![label_at(
            16.0,
            RULER_LINE - 20.0,
            drawing.ruler.label.clone(),
            Anchor::Start,
        )];
        if let Some(rule) = &drawing.across {
            views.push(label_at(
                f64::midpoint(rule.start, rule.end),
                DIMENSION_RULE - 20.0,
                rule.label.clone(),
                Anchor::Centre,
            ));
        }
        if let Some(rule) = &drawing.down {
            views.push(label_at(
                12.0,
                f64::midpoint(rule.start, rule.end),
                rule.label.clone(),
                Anchor::Up,
            ));
        }
        views
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Anchor {
    Start,
    Centre,
    Up,
}

fn label_at(x: f64, y: f64, text: String, anchor: Anchor) -> AnyView {
    AnyView::new(view! {
        text(
            class = "ant-label",
            class:centre = anchor == Anchor::Centre,
            class:up = anchor == Anchor::Up,
            style:left = Some(format!("{x:.1}px")),
            style:top = Some(format!("{y:.1}px"))
        ) {{text}}
    })
}

fn legend(report: Memo<Option<AntennaReport>>) -> impl IntoView {
    move || {
        let roles = report.with(|report| {
            report
                .as_ref()
                .map(|report| roles_in(&report.geometry))
                .unwrap_or_default()
        });
        let items: Vec<AnyView> = roles
            .into_iter()
            .map(|role| {
                let style = role_style(role);
                let swatch = zgui::elements::canvas()
                    .class("ant-legend__swatch")
                    .draw(move |cx| {
                        let mut stroke = kurbo::Stroke::new(3.0);
                        if let Some([on, off]) = style.dash {
                            stroke = stroke.with_dashes(0.0, [on, off]);
                        }
                        cx.scene.push(
                            ShapeBuilder::new(segment_path(at(0.0, 2.0), at(16.0, 2.0)))
                                .stroke_styled(style.ink.brush(1.0), stroke)
                                .build(),
                        );
                    })
                    .into_view();
                AnyView::new(view! {
                    row(class = "ant-legend__item") {
                        {swatch}
                        text {{role_label(role)}}
                    }
                })
            })
            .collect();
        view! { row(class = "ant-legend") {{items}} }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::tools::{AntennaDesign, AntennaGeometry, AntennaPoint, AntennaSegment};

    use super::*;

    fn report() -> AntennaReport {
        AntennaReport {
            design: AntennaDesign::Dipole,
            frequency_hz: 145_500_000.0,
            wavelength_m: 2.06,
            velocity_factor: 0.95,
            parts: Vec::new(),
            geometry: AntennaGeometry {
                feed: AntennaPoint::ORIGIN,
                segments: vec![
                    AntennaSegment {
                        label: "Left leg".to_owned(),
                        role: AntennaSegmentRole::Driven,
                        from: AntennaPoint::new(-0.5, 0.0, 0.0),
                        to: AntennaPoint::ORIGIN,
                    },
                    AntennaSegment {
                        label: "Right leg".to_owned(),
                        role: AntennaSegmentRole::Driven,
                        from: AntennaPoint::ORIGIN,
                        to: AntennaPoint::new(0.5, 0.0, 0.0),
                    },
                ],
            },
            feedpoint_ohms: Some(73.0),
            balanced: true,
            notes: Vec::new(),
        }
    }

    #[test]
    fn a_plan_drawing_fills_the_width_and_measures_it() {
        let drawn = drawing(&report(), Mode::Plan, ISOMETRIC, None, Unit::Metres);
        assert_eq!(drawn.title, "Front view");
        assert_eq!(drawn.pieces.len(), 2);
        assert!((drawn.pieces[0].from.x - 34.0).abs() < 1e-9);
        assert!((drawn.pieces[1].to.x - (WIDTH - 34.0)).abs() < 1e-9);
        assert_eq!(
            drawn.across.map(|rule| rule.label),
            Some("1.000 m".to_owned())
        );
        assert!(drawn.down.is_none());
    }

    #[test]
    fn a_highlight_thickens_its_part_and_dims_the_rest() {
        let drawn = drawing(
            &report(),
            Mode::Plan,
            ISOMETRIC,
            Some("Left leg"),
            Unit::Metres,
        );
        assert_eq!(drawn.strokes[0].width, 5.0);
        assert_eq!(drawn.strokes[1].alpha, 0.3);
    }

    #[test]
    fn the_orbit_adds_a_ground_grid_and_drops_the_rules() {
        let drawn = drawing(&report(), Mode::Orbit, ISOMETRIC, None, Unit::Metres);
        assert!(drawn.across.is_none());
        assert!(
            drawn
                .strokes
                .iter()
                .filter(|stroke| stroke.ink == Paint::Line)
                .count()
                > 0
        );
    }

    #[test]
    fn hovering_near_a_part_finds_it() {
        let drawn = drawing(&report(), Mode::Plan, ISOMETRIC, None, Unit::Metres);
        let near = at(100.0, drawn.feed.y + 3.0);
        assert_eq!(
            piece_at(&drawn.pieces, near, HOVER_REACH).map(|piece| piece.label.as_str()),
            Some("Left leg")
        );
        assert!(piece_at(&drawn.pieces, at(100.0, drawn.feed.y + 30.0), HOVER_REACH).is_none());
    }

    #[test]
    fn distance_to_a_segment_clamps_to_its_ends() {
        assert_eq!(distance_to(at(5.0, 3.0), at(0.0, 0.0), at(10.0, 0.0)), 3.0);
        assert_eq!(distance_to(at(13.0, 4.0), at(0.0, 0.0), at(10.0, 0.0)), 5.0);
        assert_eq!(distance_to(at(3.0, 4.0), at(0.0, 0.0), at(0.0, 0.0)), 5.0);
    }
}
