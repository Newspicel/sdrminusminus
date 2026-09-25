use zgui::{
    canvas::{CanvasScene, ShapeBuilder},
    elements::{kurbo, kurbo::Shape as _},
    prelude::*,
};

use super::{
    analysis::PointReadout,
    traces::{ChartId, Domain, chart_view, series_values},
};
use crate::ui::tools::kit::{Paint, format_hz, local_point};

const HEIGHT: f64 = 300.0;
const LEFT: f64 = 64.0;
const RIGHT: f64 = 18.0;
const TOP: f64 = 16.0;
const BOTTOM: f64 = 38.0;
const PLOT_HEIGHT: f64 = HEIGHT - TOP - BOTTOM;
const GRID_FRACTIONS: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
const ZOOM_MIN_FRACTION: f64 = 0.01;
const LEGEND_STEP: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub from: f64,
    pub to: f64,
    pub zooming: bool,
}

#[must_use]
pub fn index_at(fraction: f64, count: usize) -> usize {
    let last = count.saturating_sub(1).max(1);
    ((fraction.clamp(0.0, 1.0) * last as f64).round() as usize).min(count.saturating_sub(1))
}

#[must_use]
pub fn zoom_range(drag: Drag, count: usize) -> Option<(usize, usize)> {
    if !drag.zooming || (drag.to - drag.from).abs() < ZOOM_MIN_FRACTION {
        return None;
    }
    Some((
        index_at(drag.from.min(drag.to), count),
        index_at(drag.from.max(drag.to), count),
    ))
}

#[must_use]
pub fn stepped(marker: usize, delta: i64, count: usize) -> usize {
    let last = count.saturating_sub(1) as i64;
    (marker as i64 + delta).clamp(0, last.max(0)) as usize
}

#[must_use]
pub fn runs(points: impl Iterator<Item = Option<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut open = false;
    for point in points {
        match point {
            Some(at) if open => {
                if let Some(run) = runs.last_mut() {
                    run.push(at);
                }
            }
            Some(at) => {
                runs.push(vec![at]);
                open = true;
            }
            None => open = false,
        }
    }
    runs
}

struct Frame {
    width: f64,
    domain: Domain,
    count: usize,
}

impl Frame {
    fn plot_width(&self) -> f64 {
        (self.width - LEFT - RIGHT).max(1.0)
    }

    fn x(&self, index: usize) -> f64 {
        let last = self.count.saturating_sub(1).max(1) as f64;
        LEFT + index as f64 / last * self.plot_width()
    }

    fn y(&self, value: f64) -> f64 {
        let span = self.span();
        TOP + (self.domain.high - value.clamp(self.domain.low, self.domain.high)) / span
            * PLOT_HEIGHT
    }

    fn span(&self) -> f64 {
        let span = self.domain.high - self.domain.low;
        if span == 0.0 { 1.0 } else { span }
    }
}

fn line(from: (f64, f64), to: (f64, f64)) -> kurbo::BezPath {
    let mut path = kurbo::BezPath::new();
    path.move_to(from);
    path.line_to(to);
    path
}

fn polyline(points: &[(f64, f64)]) -> kurbo::BezPath {
    let mut path = kurbo::BezPath::new();
    for (index, point) in points.iter().enumerate() {
        if index == 0 {
            path.move_to(*point);
        } else {
            path.line_to(*point);
        }
    }
    path
}

fn dot(scene: &mut CanvasScene, at: (f64, f64), radius: f64, ink: Paint) {
    scene.push(
        ShapeBuilder::new(kurbo::Circle::new(at, radius).to_path(0.1))
            .fill(ink.brush(1.0))
            .stroke(Paint::PlotBg.brush(1.0), 1.0)
            .build(),
    );
}

fn paint_sweep(
    scene: &mut CanvasScene,
    frame: &Frame,
    rows: &[PointReadout],
    chart: ChartId,
    marker: usize,
    drag: Option<Drag>,
) {
    let view = chart_view(chart);
    let right = frame.width - RIGHT;
    for fraction in GRID_FRACTIONS {
        let y = TOP + fraction * PLOT_HEIGHT;
        let x = LEFT + fraction * frame.plot_width();
        scene.push(
            ShapeBuilder::new(line((LEFT, y), (right, y)))
                .stroke(Paint::PlotGrid.brush(1.0), 1.0)
                .build(),
        );
        scene.push(
            ShapeBuilder::new(line((x, TOP), (x, TOP + PLOT_HEIGHT)))
                .stroke(Paint::PlotGrid.brush(1.0), 1.0)
                .build(),
        );
    }
    for series in view.series {
        let points = rows.iter().enumerate().map(|(index, row)| {
            let value = (series.value)(row);
            value.is_finite().then(|| (frame.x(index), frame.y(value)))
        });
        for run in runs(points) {
            scene.push(
                ShapeBuilder::new(polyline(&run))
                    .stroke(series.ink.brush(1.0), 1.75)
                    .build(),
            );
        }
    }
    if let Some(drag) = drag.filter(|drag| drag.zooming) {
        let left = LEFT + drag.from.min(drag.to) * frame.plot_width();
        let width = (drag.to - drag.from).abs() * frame.plot_width();
        let area = kurbo::Rect::new(left, TOP, left + width, TOP + PLOT_HEIGHT).to_path(0.1);
        scene.push(
            ShapeBuilder::new(area)
                .fill(Paint::PlotInkDim.brush(0.15))
                .stroke(Paint::PlotInkDim.brush(0.5), 1.0)
                .build(),
        );
    }
    let Some(row) = rows.get(marker) else {
        return;
    };
    let x = frame.x(marker);
    scene.push(
        ShapeBuilder::new(line((x, TOP), (x, TOP + PLOT_HEIGHT)))
            .stroke(Paint::Hold.brush(1.0), 1.0)
            .build(),
    );
    for series in view.series {
        let value = (series.value)(row);
        if value.is_finite() {
            dot(scene, (x, frame.y(value)), 3.5, series.ink);
        }
    }
}

pub struct SweepInputs {
    pub rows: Memo<Vec<PointReadout>>,
    pub chart: RwSignal<ChartId>,
    pub marker: Memo<usize>,
    pub on_marker: UnsyncCallback<usize>,
    pub on_zoom: UnsyncCallback<(usize, usize)>,
}

pub fn sweep_chart(inputs: SweepInputs) -> impl IntoView {
    let SweepInputs {
        rows,
        chart,
        marker,
        on_marker,
        on_zoom,
    } = inputs;
    let drag = RwSignal::new(None::<Drag>);
    let area = NodeRef::new();
    let domain = Memo::new(move |_| {
        let view = chart_view(chart.get());
        rows.with(|rows| (view.domain)(&series_values(view, rows)))
    });
    let canvas = zgui::elements::canvas()
        .class("vna-chart__canvas")
        .draw(move |cx| {
            let width = f64::from(cx.size.width.0);
            if width <= LEFT + RIGHT {
                return;
            }
            rows.with(|rows| {
                let frame = Frame {
                    width,
                    domain: domain.get(),
                    count: rows.len(),
                };
                paint_sweep(
                    cx.scene,
                    &frame,
                    rows,
                    chart.get(),
                    marker.get(),
                    drag.get(),
                );
            });
        })
        .into_view();
    let fraction_at = move |at| {
        local_point(area, at)
            .map(|(x, _, width, _)| ((x - LEFT) / (width - LEFT - RIGHT).max(1.0)).clamp(0.0, 1.0))
    };
    let count = move || rows.with_untracked(Vec::len);
    view! {
        box(
            node_ref = area,
            class = "vna-chart",
            tabindex = Focus::Sequential,
            a11y:role = Role::Image,
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                ev.stop_propagation();
                if ev.button != Some(PointerButton::Primary) {
                    return;
                }
                let Some(fraction) = fraction_at(ev.position) else {
                    return;
                };
                ev.capture_pointer();
                let zooming = ev.modifiers.shift();
                drag.set(Some(Drag { from: fraction, to: fraction, zooming }));
                if !zooming {
                    on_marker.run(index_at(fraction, count()));
                }
            },
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| {
                let Some(held) = drag.get_untracked() else {
                    return;
                };
                let Some(fraction) = fraction_at(ev.position) else {
                    return;
                };
                drag.set(Some(Drag { to: fraction, ..held }));
                if !held.zooming {
                    on_marker.run(index_at(fraction, count()));
                }
            },
            on:pointer_up = move |ev: &mut EventCx<'_, events::PointerUp>| {
                ev.release_pointer();
                if let Some(range) = drag.get_untracked().and_then(|held| zoom_range(held, count())) {
                    on_zoom.run(range);
                }
                drag.set(None);
            },
            on:pointer_cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
                ev.release_pointer();
                drag.set(None);
            },
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                let delta = match ev.key {
                    Key::Named(NamedKey::ArrowLeft) => -1,
                    Key::Named(NamedKey::ArrowRight) => 1,
                    _ => return,
                };
                ev.prevent_default();
                let size = if ev.modifiers.shift() { 10 } else { 1 };
                on_marker.run(stepped(marker.get_untracked(), delta * size, count()));
            }
        ) {
            {canvas}
            {move || ticks(chart.get(), domain.get())}
            box(class = "vna-plot") {
                {move || legend(chart.get())}
                {move || marker_label(rows, marker.get())}
            }
            {move || axis(rows, chart.get())}
        }
    }
}

fn ticks(chart: ChartId, domain: Domain) -> Vec<AnyView> {
    let view = chart_view(chart);
    let span = domain.high - domain.low;
    let span = if span == 0.0 { 1.0 } else { span };
    GRID_FRACTIONS
        .iter()
        .map(|fraction| {
            let value = domain.high - fraction * span;
            let text = (view.format)(if value == 0.0 { 0.0 } else { value });
            let top = TOP + fraction * PLOT_HEIGHT - 7.0;
            AnyView::new(view! { text(class = "vna-label vna-tick", style:top = Some(format!("{top:.1}px"))) {{text}} })
        })
        .collect()
}

fn legend(chart: ChartId) -> Vec<AnyView> {
    let view = chart_view(chart);
    if view.series.len() < 2 {
        return Vec::new();
    }
    let count = view.series.len();
    view.series
        .iter()
        .enumerate()
        .map(|(index, series)| {
            let right = 8.0 + (count - 1 - index) as f64 * LEGEND_STEP;
            AnyView::new(view! {
                text(
                    class = "vna-label",
                    style:top = "2px",
                    style:right = Some(format!("{right:.0}px")),
                    style:color = series.ink.css()
                ) {{series.label}}
            })
        })
        .collect()
}

fn marker_label(rows: Memo<Vec<PointReadout>>, marker: usize) -> Option<AnyView> {
    let (frequency, count) =
        rows.with(|rows| (rows.get(marker).map(|row| row.frequency_hz), rows.len()));
    let frequency = frequency?;
    let last = count.saturating_sub(1).max(1) as f64;
    let percent = marker as f64 / last * 100.0;
    let flipped = marker as f64 > count as f64 / 2.0;
    let text = format_hz(frequency);
    let place = if flipped {
        format!(
            "top: 0px; right: {:.2}%; margin-right: 6px",
            100.0 - percent
        )
    } else {
        format!("top: 0px; left: {percent:.2}%; margin-left: 6px")
    };
    Some(AnyView::new(
        view! { text(class = "vna-label hold", style = Some(place)) {{text}} },
    ))
}

#[must_use]
pub fn axis_title(chart: ChartId) -> String {
    let view = chart_view(chart);
    if view.unit.is_empty() {
        view.label.to_owned()
    } else {
        format!("{} ({})", view.label, view.unit)
    }
}

fn axis(rows: Memo<Vec<PointReadout>>, chart: ChartId) -> impl IntoView {
    let (first, last) = rows.with(|rows| {
        (
            rows.first().map_or(0.0, |row| row.frequency_hz),
            rows.last().map_or(0.0, |row| row.frequency_hz),
        )
    });
    let top = format!("{:.0}px", HEIGHT - 24.0);
    let bottom = top.clone();
    let middle = top.clone();
    view! {
        text(class = "vna-label", style:top = Some(top), style:left = Some(format!("{LEFT:.0}px"))) {{format_hz(first)}}
        text(class = "vna-label", style:top = Some(bottom), style:right = Some(format!("{RIGHT:.0}px"))) {{format_hz(last)}}
        box(class = "vna-label vna-title", style:top = Some(middle), style:left = Some(format!("{LEFT:.0}px")), style:right = Some(format!("{RIGHT:.0}px"))) {{axis_title(chart)}}
    }
}

const SMITH_SIZE: f64 = 340.0;
const SMITH_CENTER: f64 = SMITH_SIZE / 2.0;
const SMITH_RADIUS: f64 = SMITH_SIZE / 2.0 - 12.0;
const RESISTANCES: [f64; 5] = [0.2, 0.5, 1.0, 2.0, 5.0];
const REACTANCES: [f64; 5] = [0.2, 0.5, 1.0, 2.0, 5.0];

#[must_use]
pub fn nearest(rows: &[PointReadout], u: f64, v: f64, fallback: usize) -> usize {
    rows.iter()
        .enumerate()
        .map(|(index, row)| (index, (row.s11.re - u).hypot(row.s11.im - v)))
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map_or(fallback, |(index, _)| index)
}

fn smith_point(re: f64, im: f64) -> (f64, f64) {
    (
        SMITH_CENTER + re * SMITH_RADIUS,
        SMITH_CENTER - im * SMITH_RADIUS,
    )
}

fn circle(centre: (f64, f64), radius: f64) -> kurbo::BezPath {
    kurbo::Circle::new(centre, radius).to_path(0.1)
}

fn paint_smith(scene: &mut CanvasScene, rows: &[PointReadout], marker: usize) {
    let unit = circle((SMITH_CENTER, SMITH_CENTER), SMITH_RADIUS);
    let grid = |path: kurbo::BezPath| {
        ShapeBuilder::new(path)
            .stroke(Paint::PlotGrid.brush(1.0), 1.0)
            .clipped(unit.clone())
            .build()
    };
    scene.push(
        ShapeBuilder::new(unit.clone())
            .stroke(Paint::PlotInkDim.brush(1.0), 1.0)
            .build(),
    );
    scene.push(grid(line(
        (SMITH_CENTER - SMITH_RADIUS, SMITH_CENTER),
        (SMITH_CENTER + SMITH_RADIUS, SMITH_CENTER),
    )));
    for r in RESISTANCES {
        scene.push(grid(circle(
            (SMITH_CENTER + r / (1.0 + r) * SMITH_RADIUS, SMITH_CENTER),
            SMITH_RADIUS / (1.0 + r),
        )));
    }
    for x in REACTANCES {
        for value in [x, -x] {
            scene.push(grid(circle(
                (
                    SMITH_CENTER + SMITH_RADIUS,
                    SMITH_CENTER - SMITH_RADIUS / value,
                ),
                (SMITH_RADIUS / value).abs(),
            )));
        }
    }
    let trace: Vec<(f64, f64)> = rows
        .iter()
        .map(|row| smith_point(row.s11.re, row.s11.im))
        .collect();
    if trace.len() > 1 {
        scene.push(
            ShapeBuilder::new(polyline(&trace))
                .stroke(Paint::Trace.brush(1.0), 1.75)
                .build(),
        );
    }
    if let Some(row) = rows.get(marker) {
        dot(scene, smith_point(row.s11.re, row.s11.im), 4.0, Paint::Hold);
    }
}

pub fn smith_chart(
    rows: Memo<Vec<PointReadout>>,
    marker: Memo<usize>,
    on_marker: UnsyncCallback<usize>,
) -> impl IntoView {
    let area = NodeRef::new();
    let held = RwSignal::new(false);
    let canvas = zgui::elements::canvas()
        .class("vna-smith__canvas")
        .draw(move |cx| rows.with(|rows| paint_smith(cx.scene, rows, marker.get())))
        .into_view();
    let pick = move |at| {
        let Some((x, y, width, height)) = local_point(area, at) else {
            return;
        };
        let u = (x / width.max(1.0) * SMITH_SIZE - SMITH_CENTER) / SMITH_RADIUS;
        let v = -(y / height.max(1.0) * SMITH_SIZE - SMITH_CENTER) / SMITH_RADIUS;
        let index = rows.with_untracked(|rows| nearest(rows, u, v, marker.get_untracked()));
        on_marker.run(index);
    };
    let frequency = move || {
        rows.with(|rows| {
            rows.get(marker.get())
                .map(|row| format_hz(row.frequency_hz))
        })
        .unwrap_or_default()
    };
    view! {
        box(
            node_ref = area,
            class = "vna-smith",
            a11y:role = Role::Image,
            a11y:label = "S11 on a Smith chart",
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                ev.stop_propagation();
                if ev.button != Some(PointerButton::Primary) {
                    return;
                }
                ev.capture_pointer();
                held.set(true);
                pick(ev.position);
            },
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| if held.get_untracked() { pick(ev.position) },
            on:pointer_up = move |ev: &mut EventCx<'_, events::PointerUp>| {
                ev.release_pointer();
                held.set(false);
            },
            on:pointer_cancel = move |ev: &mut EventCx<'_, events::PointerCancel>| {
                ev.release_pointer();
                held.set(false);
            }
        ) {
            {canvas}
            text(class = "vna-label hold", style:left = "8px", style:top = Some(format!("{:.0}px", SMITH_SIZE - 22.0))) {{frequency}}
            text(class = "vna-label", style:left = Some(format!("{:.0}px", SMITH_CENTER - SMITH_RADIUS + 2.0)), style:top = Some(format!("{:.0}px", SMITH_CENTER - 18.0))) {"0"}
            text(class = "vna-label", style:right = Some(format!("{:.0}px", SMITH_SIZE - SMITH_CENTER - SMITH_RADIUS + 2.0)), style:top = Some(format!("{:.0}px", SMITH_CENTER - 18.0))) {"\u{221e}"}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tools::nanovna::{analysis::readouts, rf::complex, testdata::point};

    #[test]
    fn a_fraction_lands_on_the_nearest_point() {
        assert_eq!(index_at(0.0, 101), 0);
        assert_eq!(index_at(0.5, 101), 50);
        assert_eq!(index_at(1.0, 101), 100);
        assert_eq!(index_at(2.0, 101), 100);
        assert_eq!(index_at(0.7, 1), 0);
        assert_eq!(index_at(0.7, 0), 0);
    }

    #[test]
    fn a_zoom_needs_a_shift_drag_wider_than_one_percent() {
        let drag = Drag {
            from: 0.6,
            to: 0.2,
            zooming: true,
        };
        assert_eq!(zoom_range(drag, 101), Some((20, 60)));
        assert_eq!(zoom_range(Drag { to: 0.605, ..drag }, 101), None);
        assert_eq!(
            zoom_range(
                Drag {
                    zooming: false,
                    ..drag
                },
                101
            ),
            None
        );
    }

    #[test]
    fn arrow_keys_step_the_marker_inside_the_sweep() {
        assert_eq!(stepped(5, -10, 101), 0);
        assert_eq!(stepped(95, 10, 101), 100);
        assert_eq!(stepped(5, 1, 101), 6);
        assert_eq!(stepped(0, 1, 0), 0);
    }

    #[test]
    fn a_missing_value_breaks_the_trace() {
        let pieces = runs([Some((0.0, 0.0)), Some((1.0, 1.0)), None, Some((3.0, 3.0))].into_iter());
        assert_eq!(pieces, vec![vec![(0.0, 0.0), (1.0, 1.0)], vec![(3.0, 3.0)]]);
        assert!(runs([None, None].into_iter()).is_empty());
    }

    #[test]
    fn the_smith_chart_picks_the_point_nearest_the_press() {
        let rows = readouts(&[
            point(1, complex(0.5, 0.0), complex(0.0, 0.0)),
            point(2, complex(0.0, 0.5), complex(0.0, 0.0)),
            point(3, complex(-0.5, 0.0), complex(0.0, 0.0)),
        ]);
        assert_eq!(nearest(&rows, 0.1, 0.45, 0), 1);
        assert_eq!(nearest(&[], 0.1, 0.45, 7), 7);
    }

    #[test]
    fn the_axis_names_the_chart_and_its_unit() {
        assert_eq!(axis_title(ChartId::Magnitude), "Magnitude (dB)");
        assert_eq!(axis_title(ChartId::Smith), "Smith");
    }
}
