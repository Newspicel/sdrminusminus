pub mod survey;

use std::sync::Arc;

use sdrmm_wire::{
    frame::FrameKind,
    patch::{NodeBody, SignalMapNode},
    ws::ClientCommand,
};
use zgui::prelude::*;
use zgui::reactive::RenderEffect;

use self::survey::{SIGNAL_MAX_DBFS, SIGNAL_MIN_DBFS, Sample, Session};
use crate::{
    binding,
    bus::Source,
    format,
    store::{SPECTRUM_BINS, SPECTRUM_FPS, Store},
    ui::{
        kit_maps::{
            SHEET, armed_clear, entry,
            feed::{Feed, Topic},
            now_ms, trail,
        },
        map::{self, ACCENT, Frame, MapProps, Overlay},
    },
};

const LEVEL_REFRESH_MS: i64 = 200;
const MAX_BANDWIDTH_HZ: u64 = 100_000_000;
const MAX_OFFSET_HZ: f64 = 1e12;
const SURVEY_FRAME_ZOOM: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Live {
    level: Option<f64>,
    target_hz: f64,
    center_hz: f64,
    span_hz: f64,
}

fn settings_of(store: Store, node: &str) -> SignalMapNode {
    store
        .graph
        .get()
        .node(node)
        .and_then(|found| match &found.body {
            NodeBody::SignalMap(settings) => Some(*settings),
            _ => None,
        })
        .unwrap_or_default()
}

fn write(store: Store, node: String, settings: SignalMapNode) {
    store.edit_graph(move |graph| {
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
            && let NodeBody::SignalMap(held) = &mut found.body
        {
            *held = settings;
        }
    });
}

fn source_of(store: Store, node: &str) -> Option<(u32, u32)> {
    let graph = store.graph.get();
    let (device, stream) = binding::iq_source_of(&graph, node)?;
    Some((store.device_set_of(&device)?, stream))
}

fn limit(live: Option<Live>, bandwidth_hz: u64) -> f64 {
    live.map_or(MAX_OFFSET_HZ, |live| {
        survey::offset_limit_hz(live.span_hz, bandwidth_hz as f64).min(MAX_OFFSET_HZ)
    })
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("kit-maps", SHEET);
    let feed = Feed::get();
    feed.listen(store);
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let source = {
        let node = node.clone();
        Memo::new(move |_| source_of(store, &node))
    };
    let positions = {
        let node = node.clone();
        Memo::new(move |_| trail::positions_of(&store.graph.get(), &node))
    };
    let session = RwSignal::new(survey::session_of(&node));
    let keeping = {
        let node = node.clone();
        RenderEffect::new(move |_| survey::keep(&node, session.get()))
    };
    on_cleanup_local(move || drop(keeping));
    let live = RwSignal::new(None::<Live>);
    let rendered = StoredValue::new(0i64);
    let recorded = StoredValue::new(0i64);

    let holding = RenderEffect::new(move |_| {
        if let Some((device_set, stream)) = source.get() {
            store.hold(ClientCommand::SubscribeSpectrum {
                device_set,
                fps: SPECTRUM_FPS,
                bins: SPECTRUM_BINS,
                stream,
            });
        }
    });
    on_cleanup_local(move || drop(holding));

    let latest = {
        let feed = feed.clone();
        move || {
            let node = positions.get_untracked().into_iter().next()?;
            feed.held
                .borrow()
                .tracks
                .get(&node)
                .and_then(|track| track.history.last().cloned())
        }
    };
    store.on_frame(move |frame| {
        if frame.kind != FrameKind::Spectrum {
            return;
        }
        let Some((device_set, stream)) = source.get_untracked() else {
            return;
        };
        if store.source_of(frame.stream_id) != Some(Source::Spectrum { device_set, stream }) {
            return;
        }
        let Some(spectrum) = crate::socket::spectrum(&frame.bytes) else {
            return;
        };
        let chosen = settings.get_untracked();
        let target_hz = (spectrum.center_hz + chosen.offset_hz as f64).round();
        let level = survey::measure(&spectrum, target_hz, chosen.bandwidth_hz as f64);
        let now = now_ms();
        if now - rendered.get_value() >= LEVEL_REFRESH_MS || level.is_none() {
            rendered.set_value(now);
            live.set(Some(Live {
                level,
                target_hz,
                center_hz: spectrum.center_hz,
                span_hz: f64::from(spectrum.span_hz),
            }));
        }
        let held = session.get_untracked();
        if let Some(first) = held.samples.first()
            && first.frequency_hz != target_hz
        {
            if held.recording {
                session.update(|session| session.recording = false);
            }
            return;
        }
        let (Some(level), Some(fix)) = (level, latest()) else {
            return;
        };
        if !held.recording || fix.received_at == recorded.get_value() {
            return;
        }
        recorded.set_value(fix.received_at);
        session.update(|session| {
            survey::merge(
                &mut session.samples,
                Sample {
                    at: fix.at,
                    frequency_hz: target_hz,
                    level_dbfs: level,
                    measured_at: now,
                    observations: 0,
                    accuracy_m: fix.accuracy_m,
                },
            );
        });
    });

    let has_fix = {
        let feed = feed.clone();
        Signal::derive_local(move || {
            let nodes = positions.get();
            feed.with(Topic::Positions, |held| {
                nodes.iter().any(|node| {
                    held.tracks
                        .get(node)
                        .is_some_and(|track| !track.history.is_empty())
                })
            })
        })
    };
    let moved = Signal::derive(move || {
        let target = live.get().map(|live| live.target_hz);
        session.with(|session| {
            session
                .samples
                .first()
                .zip(target)
                .is_some_and(|(first, target)| first.frequency_hz != target)
        })
    });

    let drawn = {
        let feed = feed.clone();
        Signal::derive_local(move || {
            let nodes = positions.get();
            let (mut overlay, trail_points) = feed.with(Topic::Positions, |held| {
                let tracks: Vec<_> = nodes
                    .iter()
                    .filter_map(|node| held.tracks.get(node))
                    .collect();
                (trail::overlay(&tracks), trail::points(&tracks))
            });
            let samples = session.with(|session| session.samples.clone());
            overlay.extend(survey::overlay(&samples));
            let frame = if samples.is_empty() {
                trail_points
            } else {
                samples.iter().map(|sample| sample.at).collect()
            };
            (Arc::new(overlay), frame)
        })
    };
    let overlay = RwSignal::new(Arc::new(Overlay::default()));
    let frame = RwSignal::new(None::<Arc<Frame>>);
    let publishing = RenderEffect::new(move |_| {
        let (next, points) = drawn.get();
        overlay.set(next);
        frame.set((!points.is_empty()).then(|| {
            Arc::new(Frame {
                points,
                max_zoom: SURVEY_FRAME_ZOOM,
            })
        }));
    });
    on_cleanup_local(move || drop(publishing));

    let active = {
        let node = node.clone();
        Signal::derive(move || store.selected.get().as_deref() == Some(node.as_str()))
    };
    let props = MapProps {
        overlay: overlay.into(),
        active,
        frame: frame.into(),
        on_pick: None,
    };
    let chrome = AnyView::new(legend(session, positions));
    view! {
        column(class = "geo survey") {
            {controls(store, node.clone(), settings, session, live, has_fix, moved)}
            {strip(session, live, has_fix, moved)}
            {map::map(store, props, chrome)}
        }
    }
}

fn controls(
    store: Store,
    node: String,
    settings: Memo<SignalMapNode>,
    session: RwSignal<Session>,
    live: RwSignal<Option<Live>>,
    has_fix: Signal<bool, LocalStorage>,
    moved: Signal<bool>,
) -> impl IntoView {
    let surveyed = Signal::derive(move || session.with(|session| !session.samples.is_empty()));
    let offset_node = node.clone();
    let offset = entry(
        "Offset kHz",
        Signal::derive(move || format!("{}", settings.get().offset_hz as f64 / 1e3)),
        surveyed,
        move |text| {
            let current = settings.get_untracked();
            let Ok(khz) = text.trim().replace(',', ".").parse::<f64>() else {
                store.say("Offset must be a number of kHz");
                return;
            };
            let bound = limit(live.get_untracked(), current.bandwidth_hz);
            let offset_hz = (khz * 1e3).round().clamp(-bound, bound) as i64;
            if offset_hz != current.offset_hz {
                write(
                    store,
                    offset_node.clone(),
                    SignalMapNode {
                        offset_hz,
                        ..current
                    },
                );
            }
        },
    );
    let width_node = node.clone();
    let width = entry(
        "Width kHz",
        Signal::derive(move || format!("{}", settings.get().bandwidth_hz as f64 / 1e3)),
        surveyed,
        move |text| {
            let current = settings.get_untracked();
            let hz = text
                .trim()
                .replace(',', ".")
                .parse::<f64>()
                .map(|khz| (khz * 1e3).round());
            let Ok(hz) = hz.map_err(drop).and_then(|hz| {
                (hz >= 1.0 && hz <= MAX_BANDWIDTH_HZ as f64)
                    .then_some(hz as u64)
                    .ok_or(())
            }) else {
                store.say("Width must be 0.001 to 100000 kHz");
                return;
            };
            if hz != current.bandwidth_hz {
                let bound = limit(live.get_untracked(), hz);
                let offset_hz = (current.offset_hz as f64).clamp(-bound, bound) as i64;
                write(
                    store,
                    width_node.clone(),
                    SignalMapNode {
                        offset_hz,
                        bandwidth_hz: hz,
                    },
                );
            }
        },
    );
    let recording = Signal::derive(move || session.with(|session| session.recording));
    let can_start = Signal::derive_local(move || {
        recording.get()
            || (has_fix.get()
                && live.get().is_some_and(|live| live.level.is_some())
                && !moved.get())
    });
    view! {
        row(class = "geo__bar") {
            {offset}
            {width}
            control(
                class = "btn",
                class:danger = recording,
                class:primary = move || !recording.get(),
                state:disabled = move || !can_start.get(),
                on:pointer_down:stop = |_| {},
                on:click:stop = move |_| session.update(|session| session.recording = !session.recording)
            ) {
                {move || if recording.get() { "Pause" } else { "Start survey" }}
            }
            {armed_clear(surveyed, move || session.update(|session| session.samples.clear()))}
            control(
                class = "btn",
                state:disabled = move || !surveyed.get(),
                on:pointer_down:stop = |_| {},
                on:click:stop = move |_| export(store, settings.get_untracked(), &session.get_untracked().samples)
            ) {"Export CSV"}
        }
    }
}

fn export(store: Store, settings: SignalMapNode, samples: &[Sample]) {
    let frequency = samples
        .first()
        .map_or(0, |sample| sample.frequency_hz.round() as i64);
    let name = format!(
        "signal-survey-{frequency}-hz-{}-hz-wide.csv",
        settings.bandwidth_hz
    );
    let Some(folder) = dirs::download_dir().or_else(dirs::home_dir) else {
        store.say("No folder to save the survey in");
        return;
    };
    let path = folder.join(name);
    match std::fs::write(
        &path,
        survey::csv(samples, settings.offset_hz, settings.bandwidth_hz),
    ) {
        Ok(()) => store.say(format!("Saved {}", path.display())),
        Err(error) => store.say(format!("Cannot save the survey: {error}")),
    }
}

fn strip(
    session: RwSignal<Session>,
    live: RwSignal<Option<Live>>,
    has_fix: Signal<bool, LocalStorage>,
    moved: Signal<bool>,
) -> impl IntoView {
    let recording = move || session.with(|session| session.recording);
    let status = move || {
        survey::status(
            has_fix.get(),
            live.get().and_then(|live| live.level),
            recording(),
            moved.get(),
        )
    };
    view! {
        row(class = "geo__strip") {
            text(class:on = recording) {{status}}
            spacer()
            text {{move || format!("{} cells", session.with(|session| session.samples.len()))}}
            text {{move || live.get().map_or_else(|| String::from("- Hz"), |live| format::frequency(live.target_hz))}}
            text(class = "geo__num") {
                {move || live.get().and_then(|live| live.level).map_or_else(|| String::from("- dBFS"), |level| format!("{level:.1} dBFS"))}
            }
        }
    }
}

fn legend(session: RwSignal<Session>, positions: Memo<Vec<String>>) -> impl IntoView {
    let feed = Feed::get();
    let trail_count = move || {
        let nodes = positions.get();
        feed.with(Topic::Positions, |held| {
            nodes
                .iter()
                .filter_map(|node| held.tracks.get(node))
                .map(|track| track.history.len())
                .sum::<usize>()
                .to_string()
        })
    };
    view! {
        column(class = "geo__legend") {
            if move || !positions.get().is_empty() {
                {trail::legend_row(ACCENT, "GPS trail", trail_count.clone())}
            }
            row(class = "geo__legend-row") {
                text(class = "geo__legend-name") {"Signal cells"}
                text(class = "geo__legend-count") {{move || session.with(|session| session.samples.len()).to_string()}}
            }
            box(class = "geo__ramp", style:background-image = Some(survey::RAMP_CSS.to_string()))
            row(class = "geo__ends") {
                text {{format!("{SIGNAL_MIN_DBFS} dBFS")}}
                text {{format!("{SIGNAL_MAX_DBFS} dBFS")}}
            }
        }
    }
}
