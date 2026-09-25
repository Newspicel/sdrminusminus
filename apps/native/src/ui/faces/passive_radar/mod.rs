use std::{cell::RefCell, rc::Rc};

use sdrmm_wire::{
    coherent::{
        Illuminator, MAX_CPI_MS, MAX_DOPPLER_SPAN_HZ, MAX_RANGE_BINS, MIN_CPI_MS,
        PassiveRadarParams,
    },
    frame::{FrameKind, RangeDopplerFrame},
    patch::NodeBody,
    ws::ClientCommand,
};
use zgui::{prelude::*, vocab::SharedString};

use super::df::finder_signal;
use crate::{
    bus::Source,
    store::Store,
    ui::{
        faces::scope::colormap::Colormap,
        kit_raster::{Bounds, edit_body, number, raster_view, readout},
        widgets::{check, row_field},
    },
};

pub mod radar;

use radar::{
    DEFAULT_ILLUMINATOR, RadarScene, Surface, detection_label, doppler_axis_hz, range_axis_km,
};

const SHOWN_DETECTIONS: usize = 3;

fn change(store: Store, node: &str, edit: impl FnOnce(&mut PassiveRadarParams)) {
    edit_body(store, node, |body| {
        if let NodeBody::PassiveRadar(radar) = body {
            edit(&mut radar.settings);
        }
    });
}

fn listen(store: Store, node: String, scene: Rc<RefCell<RadarScene>>) {
    store.hold(ClientCommand::SubscribeSurface { node: node.clone() });
    store.on_frame(move |frame| {
        if frame.kind != FrameKind::RangeDoppler {
            return;
        }
        let ours = matches!(
            store.source_of(frame.stream_id),
            Some(Source::Surface { node: ref from, .. }) if *from == node
        );
        if !ours {
            return;
        }
        let Some(decoded) = RangeDopplerFrame::decode(&frame.bytes) else {
            tracing::warn!(node, "a range-doppler frame did not decode");
            return;
        };
        if let Ok(mut scene) = scene.try_borrow_mut() {
            scene.show(Surface {
                ranges: decoded.ranges,
                dopplers: decoded.dopplers,
                doppler_step_hz: decoded.doppler_step_hz,
                cells: decoded.cells.to_vec(),
            });
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-radar", SHEET);
    let finder = finder_signal(store, node.clone());
    let detections = Memo::new(move |_| {
        finder
            .get()
            .map(|finder| finder.detections)
            .unwrap_or_default()
    });
    let scene = Rc::new(RefCell::new(RadarScene::new(Colormap::Classic)));
    listen(store, node.clone(), scene.clone());
    let marking = {
        let scene = scene.clone();
        zgui::reactive::RenderEffect::new(move |_| {
            let hits = detections.get();
            if let Ok(mut scene) = scene.try_borrow_mut() {
                scene.mark(hits);
            }
        })
    };
    on_cleanup_local(move || drop(marking));
    let rows = move || {
        let hits = detections.get();
        let mut rows = vec![("Detections".to_owned(), hits.len().to_string())];
        rows.extend(hits.iter().take(SHOWN_DETECTIONS).map(detection_label));
        rows
    };
    view! {
        column(class = "face") {
            {raster_view("rd__plot", scene)}
            {readout(rows)}
            box(class = "rule") {}
            {settings(store, node)}
        }
    }
}

fn field(
    settings: Memo<Option<PassiveRadarParams>>,
    pick: impl Fn(&PassiveRadarParams) -> f64 + Send + Sync + 'static,
) -> Signal<f64> {
    Signal::derive(move || settings.get().as_ref().map(&pick).unwrap_or_default())
}

fn settings(store: Store, node: String) -> impl IntoView {
    let settings = {
        let node = node.clone();
        Memo::new(move |_| {
            store
                .graph
                .get()
                .node(&node)
                .and_then(|found| match &found.body {
                    NodeBody::PassiveRadar(radar) => Some(radar.settings),
                    _ => None,
                })
        })
    };
    let read = move |pick: fn(&PassiveRadarParams) -> f64| field(settings, pick);
    let write = move |edit: Box<dyn FnOnce(&mut PassiveRadarParams)>| change(store, &node, edit);
    let known = Memo::new(move |_| {
        settings
            .get()
            .is_some_and(|params| params.illuminator.is_some())
    });
    let reach = move || {
        settings
            .get()
            .map(|params| format!("{:.1} km of range", range_axis_km(&params, 1.0)))
            .unwrap_or_default()
    };
    let spread = move || {
        settings
            .get()
            .map(|params| format!("{:.1} Hz either side of zero", doppler_axis_hz(&params)))
            .unwrap_or_default()
    };
    let cpi = {
        let write = write.clone();
        move |ms: f64| write(Box::new(move |params| params.cpi_ms = ms as u32))
    };
    let bins = {
        let write = write.clone();
        move |count: f64| write(Box::new(move |params| params.max_range_bins = count as u32))
    };
    let span = {
        let write = write.clone();
        move |hz: f64| write(Box::new(move |params| params.doppler_span_hz = hz))
    };
    let toggle = {
        let write = write.clone();
        move |on: bool| {
            write(Box::new(move |params| {
                params.illuminator = on.then_some(DEFAULT_ILLUMINATOR);
            }));
        }
    };
    let transmitter = move || {
        let write = write.clone();
        known.get().then(|| {
            let place = move |edit: fn(&mut Illuminator, f64)| {
                let write = write.clone();
                move |value: f64| {
                    write(Box::new(move |params| {
                        let mut source = params.illuminator.unwrap_or(DEFAULT_ILLUMINATOR);
                        edit(&mut source, value);
                        params.illuminator = Some(source);
                    }));
                }
            };
            let lit = move |pick: fn(&Illuminator) -> f64| {
                field(settings, move |params| {
                    params.illuminator.as_ref().map(pick).unwrap_or_default()
                })
            };
            view! {
                column(class = "params") {
                    {row_field("Latitude", number(
                        "Transmitter latitude",
                        lit(|source| source.lat),
                        Bounds::new(-90.0, 90.0),
                        place(|source, value| source.lat = value),
                    ))}
                    {row_field("Longitude", number(
                        "Transmitter longitude",
                        lit(|source| source.lon),
                        Bounds::new(-180.0, 180.0),
                        place(|source, value| source.lon = value),
                    ))}
                    {row_field("Frequency Hz", number(
                        "Transmitter frequency",
                        lit(|source| source.freq_hz),
                        Bounds::new(1.0, 1e11),
                        place(|source, value| source.freq_hz = value),
                    ))}
                }
            }
        })
    };
    view! {
        column(class = "params") {
            box(a11y:description = move || SharedString::from(reach())) {
                {row_field("Integration ms", number(
                    "Coherent processing interval",
                    read(|params| f64::from(params.cpi_ms)),
                    Bounds::whole(f64::from(MIN_CPI_MS), f64::from(MAX_CPI_MS)),
                    cpi,
                ))}
            }
            {row_field("Range bins", number(
                "Range bins",
                read(|params| f64::from(params.max_range_bins)),
                Bounds::whole(1.0, f64::from(MAX_RANGE_BINS)),
                bins,
            ))}
            {row_field("Transmitter", check(known.into(), toggle))}
            {transmitter}
            box(a11y:description = move || SharedString::from(spread())) {
                {row_field("Doppler Hz", number(
                    "Doppler span",
                    read(|params| params.doppler_span_hz),
                    Bounds::new(1.0, MAX_DOPPLER_SPAN_HZ),
                    span,
                ))}
            }
        }
    }
}

const SHEET: &str = css!(
    r#"
.rd__plot { width: 100%; height: 180px; border-radius: 4px; }
"#
);
