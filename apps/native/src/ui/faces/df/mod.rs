use std::time::{Duration, SystemTime};

use sdrmm_wire::{
    coherent::{
        DfAlgorithm, DfParams, DfReading, MAX_ARRAY_ELEMENTS, MAX_ARRAY_EXTENT_M,
        MAX_DF_BANDWIDTH_HZ, MAX_DF_OFFSET_HZ, MAX_DF_REPORT_MS, MAX_STATION_ID_LEN,
        MIN_ARRAY_ELEMENTS, MIN_DF_BANDWIDTH_HZ, MIN_DF_REPORT_MS,
    },
    patch::NodeBody,
};
use zgui::{
    canvas::{Brush, CanvasScene, ShapeBuilder, zgui_color::Color},
    elements::{kurbo, kurbo::Shape as _},
    prelude::*,
};

use crate::{
    coherent::Finder,
    store::Store,
    ui::{
        kit_raster::{Bounds, button, edit_body, number, readout},
        params::entry,
        widgets::{pick, row_field},
    },
};

pub mod logic;

use logic::{
    Beam, COMPASS_MARKS, CalVerdict, Shape, beam_azimuth, beam_mode, bearing_label, extent_of,
    geometry_of, lane_quality_percent, polar_point, shape_of, spectrum_points, tier_label,
    with_count, with_extent,
};

const SIZE: f64 = 220.0;
const CENTRE: f64 = SIZE / 2.0;
const INNER: f64 = 26.0;
const OUTER: f64 = SIZE / 2.0 - 20.0;
const TRAIL_AGE: Duration = Duration::from_secs(300);

pub(crate) fn finder_signal(store: Store, node: String) -> Signal<Option<Finder>> {
    Signal::derive(move || store.coherent.get().by_node.get(&node).cloned())
}

pub(crate) fn cal_rows(finder: Option<&Finder>) -> Vec<(String, String)> {
    let cal = finder.map(|finder| &finder.cal);
    vec![
        ("Calibration".into(), CalVerdict::of(cal).text().into()),
        ("Coherence".into(), tier_label(cal).into()),
    ]
}

pub(crate) fn calibrate(store: Store, node: String) {
    zgui::task::spawn_local(async move {
        let path = format!("/api/coherent/{node}/calibrate");
        if let Err(error) = store
            .api()
            .send::<(), serde::de::IgnoredAny>(reqwest::Method::POST, &path, None)
            .await
        {
            store.say(format!("cannot calibrate: {error}"));
        }
    });
}

fn settings_memo(store: Store, node: String) -> Memo<Option<DfParams>> {
    Memo::new(move |_| {
        store
            .graph
            .get()
            .node(&node)
            .and_then(|found| match &found.body {
                NodeBody::Df(df) => Some(df.settings.clone()),
                _ => None,
            })
    })
}

fn change(store: Store, node: &str, edit: impl FnOnce(&mut DfParams)) {
    edit_body(store, node, |body| {
        if let NodeBody::Df(df) = body {
            edit(&mut df.settings);
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-coherent", SHEET);
    let finder = finder_signal(store, node.clone());
    let bearing = Signal::derive(move || {
        let finder = finder.get()?;
        CalVerdict::of(Some(&finder.cal))
            .trusts_bearing()
            .then_some(finder.reading)
    });
    let rows = move || {
        let shown = bearing.get();
        let mut rows = vec![
            (
                "Bearing".to_owned(),
                shown
                    .as_ref()
                    .map_or_else(|| "-".into(), |reading| bearing_label(reading.bearing_deg)),
            ),
            (
                "Confidence".to_owned(),
                shown.as_ref().map_or_else(
                    || "-".into(),
                    |reading| format!("{:.0}%", reading.confidence * 100.0),
                ),
            ),
        ];
        rows.extend(cal_rows(finder.get().as_ref()));
        rows
    };
    let quiet = Signal::derive(move || finder.get().is_none_or(|finder| !finder.heard));
    let press = {
        let node = node.clone();
        move || calibrate(store, node.clone())
    };
    view! {
        column(class = "face") {
            box(class = "df__rose") {
                {rose(finder, bearing)}
                {marks()}
            }
            {readout(rows)}
            {lanes(finder)}
            row(class = "section") {
                {button("Calibrate", quiet, press)}
            }
            box(class = "rule") {}
            {settings(store, node, bearing)}
        }
    }
}

fn marks() -> Vec<AnyView> {
    COMPASS_MARKS
        .iter()
        .map(|(bearing, label)| {
            let (x, y) = polar_point(*bearing, OUTER + 10.0, CENTRE);
            AnyView::new(view! {
                text(
                    class = "df__mark",
                    style:left = Some(format!("{:.1}px", x - 10.0)),
                    style:top = Some(format!("{:.1}px", y - 6.0))
                ) {
                    {*label}
                }
            })
        })
        .collect()
}

fn lanes(finder: Signal<Option<Finder>>) -> impl IntoView {
    move || {
        let lanes = finder
            .get()
            .map(|finder| finder.cal.lanes)
            .unwrap_or_default();
        (!lanes.is_empty()).then(|| {
            let bars = lanes
                .iter()
                .map(|lane| {
                    let width = lane_quality_percent(lane.quality);
                    view! {
                        box(class = "df__lane") {
                            box(class = "df__lane_fill", style:width = Some(format!("{width}%")))
                        }
                    }
                })
                .collect::<Vec<_>>();
            view! { row(class = "df__lanes", a11y:label = "Lane calibration") {{bars}} }
        })
    }
}

fn ink(alpha: f32) -> Brush {
    Brush::Solid(Color::srgb(0.33, 0.35, 0.39, alpha))
}

fn accent(alpha: f32) -> Brush {
    Brush::Solid(Color::srgb(0.48, 0.66, 1.0, alpha))
}

fn rose(finder: Signal<Option<Finder>>, bearing: Signal<Option<DfReading>>) -> impl IntoView {
    zgui::elements::canvas()
        .class("df__dial")
        .draw(move |cx| {
            let scale = f64::from(cx.size.width.0) / SIZE;
            if scale <= 0.0 {
                return;
            }
            let at = |(x, y): (f64, f64)| kurbo::Point::new(x * scale, y * scale);
            dial(cx.scene, scale, at);
            let Some(reading) = bearing.get() else {
                return;
            };
            if let Some(finder) = finder.get() {
                trail(cx.scene, &finder, scale, at);
            }
            spectrum(cx.scene, &reading.pseudospectrum, at);
            let mut needle = kurbo::BezPath::new();
            needle.move_to(at((CENTRE, CENTRE)));
            needle.line_to(at(polar_point(
                f64::from(reading.bearing_deg),
                OUTER,
                CENTRE,
            )));
            cx.scene.push(
                ShapeBuilder::new(needle)
                    .stroke(accent(1.0), 2.0 * scale)
                    .build(),
            );
        })
        .into_view()
}

fn dial(scene: &mut CanvasScene, scale: f64, at: impl Fn((f64, f64)) -> kurbo::Point) {
    let centre = at((CENTRE, CENTRE));
    for (radius, alpha) in [(OUTER, 1.0), ((OUTER + INNER) / 2.0, 0.5)] {
        let ring = kurbo::Circle::new(centre, radius * scale).to_path(0.1);
        scene.push(ShapeBuilder::new(ring).stroke(ink(alpha), 1.0).build());
    }
    for (bearing, _) in COMPASS_MARKS {
        let mut tick = kurbo::BezPath::new();
        tick.move_to(at(polar_point(bearing, OUTER - 6.0, CENTRE)));
        tick.line_to(at(polar_point(bearing, OUTER, CENTRE)));
        scene.push(ShapeBuilder::new(tick).stroke(ink(1.0), 1.0).build());
    }
    let dot = kurbo::Circle::new(centre, 2.0 * scale).to_path(0.1);
    scene.push(ShapeBuilder::new(dot).fill(accent(1.0)).build());
}

fn spectrum(scene: &mut CanvasScene, levels: &[u8], at: impl Fn((f64, f64)) -> kurbo::Point) {
    let points = spectrum_points(levels, CENTRE, INNER, OUTER);
    let Some(first) = points.first() else {
        return;
    };
    let mut outline = kurbo::BezPath::new();
    outline.move_to(at(*first));
    for point in &points[1..] {
        outline.line_to(at(*point));
    }
    outline.close_path();
    scene.push(ShapeBuilder::new(outline.clone()).fill(accent(0.2)).build());
    scene.push(ShapeBuilder::new(outline).stroke(accent(1.0), 1.0).build());
}

fn trail(
    scene: &mut CanvasScene,
    finder: &Finder,
    scale: f64,
    at: impl Fn((f64, f64)) -> kurbo::Point,
) {
    let now = SystemTime::now();
    for sample in &finder.history {
        let age = now.duration_since(sample.at).unwrap_or_default();
        let fade = 1.0 - age.as_secs_f32() / TRAIL_AGE.as_secs_f32();
        if fade <= 0.0 {
            continue;
        }
        let mut ray = kurbo::BezPath::new();
        ray.move_to(at((CENTRE, CENTRE)));
        ray.line_to(at(polar_point(
            f64::from(sample.bearing_deg),
            OUTER,
            CENTRE,
        )));
        let alpha = 0.25 * fade * sample.confidence.clamp(0.0, 1.0);
        scene.push(ShapeBuilder::new(ray).stroke(accent(alpha), scale).build());
    }
}

fn settings(store: Store, node: String, bearing: Signal<Option<DfReading>>) -> impl IntoView {
    let settings = settings_memo(store, node.clone());
    let read = move |pick: fn(&DfParams) -> f64| {
        Signal::derive(move || settings.get().as_ref().map(pick).unwrap_or_default())
    };
    let write = move |edit: Box<dyn FnOnce(&mut DfParams)>| change(store, &node, edit);
    let algorithm = Signal::derive(move || settings.get().map(|params| params.algorithm));
    let shape = Memo::new(move |_| settings.get().and_then(|params| shape_of(&params.geometry)));
    let beam = Memo::new(move |_| {
        settings
            .get()
            .map(|params| beam_mode(params.beam_bearing_deg))
    });
    let station = Signal::derive(move || {
        settings
            .get()
            .and_then(|params| params.station_id)
            .unwrap_or_default()
    });
    let heading = move || {
        bearing
            .get_untracked()
            .map(|reading| f64::from(reading.bearing_deg))
    };

    let pick_algorithm = {
        let write = write.clone();
        move |chosen: DfAlgorithm| write(Box::new(move |params| params.algorithm = chosen))
    };
    let pick_shape = {
        let write = write.clone();
        move |chosen: Shape| {
            write(Box::new(move |params| {
                params.geometry = geometry_of(chosen, &params.geometry);
            }));
        }
    };
    let extent = {
        let write = write.clone();
        move || {
            let write = write.clone();
            shape.get().map(|shape| {
                let label = if shape == Shape::Circle {
                    "Radius m"
                } else {
                    "Spacing m"
                };
                let value = read(|params| extent_of(&params.geometry).unwrap_or_default());
                AnyView::new(row_field(
                    label,
                    number(
                        label,
                        value,
                        Bounds::new(0.01, MAX_ARRAY_EXTENT_M),
                        move |metres| {
                            write(Box::new(move |params| {
                                params.geometry = with_extent(&params.geometry, metres);
                            }));
                        },
                    ),
                ))
            })
        }
    };
    let elements = {
        let write = write.clone();
        move |count: f64| {
            write(Box::new(move |params| {
                params.geometry = with_count(&params.geometry, count as u32);
            }));
        }
    };
    let offset = {
        let write = write.clone();
        move |hz: f64| write(Box::new(move |params| params.offset_hz = hz))
    };
    let bandwidth = {
        let write = write.clone();
        move |hz: f64| write(Box::new(move |params| params.bandwidth_hz = hz))
    };
    let report = {
        let write = write.clone();
        move |ms: f64| write(Box::new(move |params| params.report_ms = ms as u32))
    };
    let name = {
        let write = write.clone();
        move |text: String| {
            let trimmed = text.trim().to_owned();
            if trimmed.len() > MAX_STATION_ID_LEN {
                return Err(format!("at most {MAX_STATION_ID_LEN} characters"));
            }
            let id = (!trimmed.is_empty()).then_some(trimmed);
            write(Box::new(move |params| params.station_id = id));
            Ok(())
        }
    };
    let pick_beam = {
        let write = write.clone();
        move |mode: Beam| {
            let azimuth = beam_azimuth(mode, heading());
            write(Box::new(move |params| params.beam_bearing_deg = azimuth));
        }
    };
    let azimuth = move || {
        let write = write.clone();
        (beam.get() == Some(Beam::Fixed)).then(|| {
            AnyView::new(row_field(
                "Azimuth",
                number(
                    "Beam azimuth",
                    read(|params| params.beam_bearing_deg.unwrap_or_default()),
                    Bounds::whole(0.0, 359.0),
                    move |deg| write(Box::new(move |params| params.beam_bearing_deg = Some(deg))),
                ),
            ))
        })
    };

    view! {
        column(class = "params") {
            {row_field("Algorithm", pick(
                vec![
                    (DfAlgorithm::Correlative, "Beamformer".to_owned()),
                    (DfAlgorithm::Music, "MUSIC".to_owned()),
                ],
                algorithm,
                pick_algorithm,
            ))}
            {row_field("Geometry", pick(
                vec![(Shape::Circle, "Circle".to_owned()), (Shape::Line, "Line".to_owned())],
                shape.into(),
                pick_shape,
            ))}
            {extent}
            {row_field("Elements", number(
                "Element count",
                read(|params| f64::from(params.geometry.count())),
                Bounds::whole(f64::from(MIN_ARRAY_ELEMENTS), f64::from(MAX_ARRAY_ELEMENTS)),
                elements,
            ))}
            {row_field("Offset Hz", number(
                "Signal offset",
                read(|params| params.offset_hz),
                Bounds::new(-MAX_DF_OFFSET_HZ, MAX_DF_OFFSET_HZ),
                offset,
            ))}
            {row_field("Bandwidth Hz", number(
                "Signal bandwidth",
                read(|params| params.bandwidth_hz),
                Bounds::new(MIN_DF_BANDWIDTH_HZ, MAX_DF_BANDWIDTH_HZ),
                bandwidth,
            ))}
            {row_field("Report ms", number(
                "Report interval",
                read(|params| f64::from(params.report_ms)),
                Bounds::whole(f64::from(MIN_DF_REPORT_MS), f64::from(MAX_DF_REPORT_MS)),
                report,
            ))}
            {row_field("Station", entry(station, "Station id".to_owned(), true, name))}
            {row_field("Beam", pick(
                vec![(Beam::Follow, "Follow bearing".to_owned()), (Beam::Fixed, "Fixed azimuth".to_owned())],
                beam.into(),
                pick_beam,
            ))}
            {azimuth}
        }
    }
}

pub(crate) const SHEET: &str = css!(
    r#"
.df__rose { position: relative; width: 220px; height: 220px; align-self: center; }
.df__dial { position: absolute; left: 0; top: 0; width: 220px; height: 220px; }
.df__mark {
    position: absolute;
    width: 20px;
    height: 12px;
    text-align: center;
    font-family: var(--mono);
    font-size: 8px;
    line-height: 12px;
    color: var(--ink-dim);
    pointer-events: none;
}
.df__lanes { gap: 4px; }
.df__lane { flex: 1 1 0; height: 6px; border-radius: 999px; background-color: var(--line); overflow: hidden; }
.df__lane_fill { height: 6px; border-radius: 999px; background-color: var(--accent); }
.co__station { gap: 8px; align-items: baseline; justify-content: space-between; font-size: 12px; }
.co__station_seen { font-size: 11px; color: var(--ink-dim); }
"#
);
