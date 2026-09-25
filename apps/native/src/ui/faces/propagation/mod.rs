pub mod ionosonde;
pub mod layers;
pub mod model;

use std::{cell::RefCell, collections::HashMap, sync::Arc, time::Duration};

use sdrmm_wire::{
    DecoderLogResponse,
    decode::DecodedRecord,
    patch::NodeBody,
    propagation::{IonosondeReport, IonosondeStation, PropagationNode},
    ws::ServerEvent,
};
use zgui::prelude::*;
use zgui::reactive::RenderEffect;

use self::{
    ionosonde::{Agreement, Comparison, Forecast},
    layers::Layer,
    model::{Cell, Observation, Options, Summary},
};
use crate::{
    store::Store,
    ui::{
        kit_maps::{
            SHEET, armed_clear, every,
            feed::{Feed, Topic},
            iso_of, mhz, now_ms, ticker, trail,
            wiring::{channel_type_of, event_sources},
        },
        map::{self, Frame, Geo, MapProps, Overlay},
        widgets::{check, pick, segments},
    },
};

const REDRAW: Duration = Duration::from_secs(2);
const IONOSONDE_EVERY: Duration = Duration::from_secs(15 * 60);
const HISTORY_LIMIT: usize = 2_000;
const TABLE_ROWS: usize = 12;
const RECEIVER_ZOOM: f64 = 3.0;

const HALF_LIVES: [(u32, &str); 7] = [
    (5, "5 min"),
    (15, "15 min"),
    (30, "30 min"),
    (60, "1 h"),
    (120, "2 h"),
    (360, "6 h"),
    (720, "12 h"),
];

const HEIGHTS: [(u32, &str); 5] = [
    (110, "110 km (E)"),
    (250, "250 km (F2 low)"),
    (300, "300 km (F2)"),
    (350, "350 km (F2 high)"),
    (400, "400 km"),
];

#[derive(Clone, Debug, Default, PartialEq)]
struct Session {
    observations: Vec<Observation>,
    cleared_at: i64,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<String, Session>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Input {
    node: String,
    device_set: u32,
    channel: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Computed {
    overlay: Arc<Overlay>,
    summary: Summary,
    cells: Vec<Cell>,
    comparisons: Vec<Comparison>,
    agreement: Agreement,
    overhead: Option<Forecast>,
    live: usize,
}

fn settings_of(store: Store, node: &str) -> PropagationNode {
    store
        .graph
        .get()
        .node(node)
        .and_then(|found| match &found.body {
            NodeBody::Propagation(settings) => Some(*settings),
            _ => None,
        })
        .unwrap_or_default()
}

fn update(store: Store, node: String, change: impl FnOnce(&mut PropagationNode) + 'static) {
    store.edit_graph(move |graph| {
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
            && let NodeBody::Propagation(held) = &mut found.body
        {
            change(held);
        }
    });
}

fn inputs_of(store: Store, node: &str) -> Vec<Input> {
    let graph = store.graph.get();
    event_sources(&graph, node)
        .into_iter()
        .filter(|source| {
            channel_type_of(&graph, source)
                .and_then(|kind| store.descriptor_of(&kind)?.decoder_kind)
                .is_some_and(|kind| model::is_propagation_kind(&kind))
        })
        .filter_map(|source| {
            let channel = store.channel_of(&source)?;
            Some(Input {
                device_set: store.device_set_of(&source)?,
                channel: channel.id,
                node: source,
            })
        })
        .collect()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn history_path(kind: &str, inputs: &[Input], since: i64) -> String {
    let nodes: Vec<&str> = inputs.iter().map(|input| input.node.as_str()).collect();
    let sources: Vec<String> = inputs
        .iter()
        .map(|input| format!("{}:{}", input.device_set, input.channel))
        .collect();
    format!(
        "/api/decoderlog?kind={kind}&nodes={}&sources={}&since={}&limit={HISTORY_LIMIT}",
        encode(&nodes.join(",")),
        encode(&sources.join(",")),
        encode(&iso_of(since)),
    )
}

fn observations_of(
    records: &[DecodedRecord],
    wanted: &[Input],
    receiver: Geo,
    height_km: f64,
) -> Vec<Observation> {
    records
        .iter()
        .filter(|record| {
            wanted.iter().any(|input| {
                input.device_set == record.device_set && input.channel == record.channel
            })
        })
        .filter_map(|record| model::observation_of(record, receiver, height_km))
        .collect()
}

fn compute(
    held: &[Observation],
    cleared_at: i64,
    settings: PropagationNode,
    receiver: Option<Geo>,
    sondes: &[IonosondeStation],
    layer: Layer,
    now: i64,
) -> Computed {
    let options = Options {
        half_life_minutes: f64::from(settings.half_life_minutes),
        now_ms: now,
    };
    let observations = model::live(held, options, cleared_at);
    let cells = model::cells(&observations, options);
    let paths = match (settings.show_paths, receiver) {
        (true, Some(receiver)) => model::paths(&observations, receiver, options),
        _ => Vec::new(),
    };
    let sondes: &[IonosondeStation] = if settings.compare_forecast {
        sondes
    } else {
        &[]
    };
    let comparisons = ionosonde::compare(&cells, sondes);
    Computed {
        overlay: Arc::new(layers::overlay(&cells, &paths, sondes, layer)),
        summary: model::summary(&observations),
        agreement: ionosonde::agreement(&comparisons),
        overhead: receiver
            .and_then(|at| ionosonde::forecast_at(sondes, at, ionosonde::FORECAST_RADIUS_KM)),
        comparisons,
        cells,
        live: observations.len(),
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("kit-maps", SHEET);
    let feed = Feed::get();
    feed.listen(store);
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let inputs = {
        let node = node.clone();
        Memo::new(move |_| inputs_of(store, &node))
    };
    let positions = {
        let node = node.clone();
        Memo::new(move |_| trail::positions_of(&store.graph.get(), &node))
    };
    let receiver = {
        let feed = feed.clone();
        Signal::derive_local(move || {
            let nodes = positions.get();
            feed.with(Topic::Positions, |held| {
                nodes
                    .iter()
                    .find_map(|node| model::receiver_of(held.tracks.get(node)?.fix.as_ref()))
            })
        })
    };
    let session = RwSignal::new(
        SESSIONS.with(|sessions| sessions.borrow().get(&node).cloned().unwrap_or_default()),
    );
    let keeping = {
        let node = node.clone();
        RenderEffect::new(move |_| {
            let held = session.get();
            SESSIONS.with(|sessions| sessions.borrow_mut().insert(node.clone(), held));
        })
    };
    on_cleanup_local(move || drop(keeping));
    let layer = RwSignal::new(Layer::Activity);
    let now = ticker(REDRAW);
    let seed = RwSignal::new(Vec::<DecodedRecord>::new());
    let report = RwSignal::new(None::<Result<IonosondeReport, String>>);

    listen(store, inputs, receiver, settings, session);
    load_history(store, inputs, seed);
    load_ionosondes(store, settings, report);

    let computed = RwSignal::new(Computed::default());
    let computing = RenderEffect::new(move |_| {
        let at = now.get();
        let chosen = settings.get();
        let here = receiver.get();
        let height = f64::from(chosen.reflection_height_km);
        let seeded = match here {
            Some(receiver) => {
                seed.with(|records| observations_of(records, &inputs.get(), receiver, height))
            }
            None => Vec::new(),
        };
        let (held, cleared_at) = session.with(|session| {
            (
                model::merge(&seeded, &session.observations, model::OBSERVATION_CAPACITY)
                    .unwrap_or(seeded),
                session.cleared_at,
            )
        });
        let sondes = report.with(|report| {
            report
                .as_ref()
                .and_then(|report| report.as_ref().ok())
                .map(|report| report.stations.clone())
                .unwrap_or_default()
        });
        computed.set(compute(
            &held,
            cleared_at,
            chosen,
            here,
            &sondes,
            layer.get(),
            at,
        ));
    });
    on_cleanup_local(move || drop(computing));

    let active = {
        let node = node.clone();
        Signal::derive(move || store.selected.get().as_deref() == Some(node.as_str()))
    };
    let overlay = {
        let feed = feed.clone();
        Signal::derive_local(move || {
            let nodes = positions.get();
            let mut drawn = feed.with(Topic::Positions, |held| {
                let tracks: Vec<_> = nodes
                    .iter()
                    .filter_map(|node| held.tracks.get(node))
                    .collect();
                trail::overlay(&tracks)
            });
            drawn.extend((*computed.with(|computed| computed.overlay.clone())).clone());
            Arc::new(drawn)
        })
    };
    let shown = RwSignal::new(Arc::new(Overlay::default()));
    let frame = RwSignal::new(None::<Arc<Frame>>);
    let publishing = RenderEffect::new(move |_| {
        shown.set(overlay.get());
        let at = receiver.get();
        frame.set(at.map(|at| {
            Arc::new(Frame {
                points: vec![at],
                max_zoom: RECEIVER_ZOOM,
            })
        }));
    });
    on_cleanup_local(move || drop(publishing));
    let props = MapProps {
        overlay: shown.into(),
        active,
        frame: frame.into(),
        on_pick: None,
    };
    view! {
        column(class = "geo propagation") {
            {controls(store, node.clone(), settings, layer, session, computed)}
            {strip(settings, computed)}
            {map::map(store, props, AnyView::new(()))}
            {table(settings, computed)}
            {notice(settings, report, computed)}
        }
    }
}

fn listen(
    store: Store,
    inputs: Memo<Vec<Input>>,
    receiver: Signal<Option<Geo>, LocalStorage>,
    settings: Memo<PropagationNode>,
    session: RwSignal<Session>,
) {
    store.on_event(move |event| {
        let records: &[DecodedRecord] = match event {
            ServerEvent::Decoded(record) => std::slice::from_ref(record.as_ref()),
            ServerEvent::DecodedBacklog { records } => records,
            _ => return,
        };
        let Some(here) = receiver.get_untracked() else {
            return;
        };
        let height = f64::from(settings.get_untracked().reflection_height_km);
        let fresh = observations_of(records, &inputs.get_untracked(), here, height);
        if fresh.is_empty() {
            return;
        }
        let merged = session.with_untracked(|session| {
            model::merge(&session.observations, &fresh, model::OBSERVATION_CAPACITY)
        });
        if let Some(merged) = merged {
            session.update(|session| session.observations = merged);
        }
    });
}

fn load_history(store: Store, inputs: Memo<Vec<Input>>, seed: RwSignal<Vec<DecodedRecord>>) {
    let since = now_ms() - model::HISTORY_WINDOW_MS;
    let loading = RenderEffect::new(move |_| {
        let wanted = inputs.get();
        if wanted.is_empty() {
            seed.set(Vec::new());
            return;
        }
        zgui::task::spawn_local(async move {
            let mut records = Vec::new();
            for kind in model::PROPAGATION_KINDS {
                match store
                    .api()
                    .get::<DecoderLogResponse>(&history_path(kind, &wanted, since))
                    .await
                {
                    Ok(response) => {
                        records.extend(response.entries.into_iter().map(|entry| DecodedRecord {
                            origin: entry.origin,
                            device_set: entry.device_set,
                            channel: entry.channel,
                            at: entry.at,
                            freq_hz: entry.freq_hz,
                            event: entry.event,
                            sinks: Vec::new(),
                        }))
                    }
                    Err(error) => store.say(format!("cannot read the {kind} history: {error}")),
                }
            }
            seed.set(records);
        });
    });
    on_cleanup_local(move || drop(loading));
}

fn load_ionosondes(
    store: Store,
    settings: Memo<PropagationNode>,
    report: RwSignal<Option<Result<IonosondeReport, String>>>,
) {
    let fetch = move || {
        if !settings.get_untracked().compare_forecast {
            return;
        }
        zgui::task::spawn_local(async move {
            let answer = store.api().get::<IonosondeReport>("/api/ionosonde").await;
            report.set(Some(answer.map_err(|error| error.to_string())));
        });
    };
    let asking = RenderEffect::new(move |_| {
        if settings.get().compare_forecast {
            fetch();
        }
    });
    on_cleanup_local(move || drop(asking));
    every(IONOSONDE_EVERY, fetch);
}

fn controls(
    store: Store,
    node: String,
    settings: Memo<PropagationNode>,
    layer: RwSignal<Layer>,
    session: RwSignal<Session>,
    computed: RwSignal<Computed>,
) -> impl IntoView {
    let half_node = node.clone();
    let height_node = node.clone();
    let paths_node = node.clone();
    let sondes_node = node;
    let half_life = pick(
        HALF_LIVES
            .iter()
            .map(|(value, name)| (*value, (*name).to_owned()))
            .collect(),
        Signal::derive(move || Some(settings.get().half_life_minutes)),
        move |minutes| {
            update(store, half_node.clone(), move |held| {
                held.half_life_minutes = minutes
            })
        },
    );
    let height = pick(
        HEIGHTS
            .iter()
            .map(|(value, name)| (*value, (*name).to_owned()))
            .collect(),
        Signal::derive(move || Some(settings.get().reflection_height_km)),
        move |km| {
            update(store, height_node.clone(), move |held| {
                held.reflection_height_km = km
            })
        },
    );
    let layers = segments(
        vec![(Layer::Activity, "Activity"), (Layer::Muf, "MUF")],
        layer.into(),
        move |chosen| layer.set(chosen),
    );
    let paths = check(
        Signal::derive(move || settings.get().show_paths),
        move |on| {
            update(store, paths_node.clone(), move |held| held.show_paths = on);
        },
    );
    let sondes = check(
        Signal::derive(move || settings.get().compare_forecast),
        move |on| {
            update(store, sondes_node.clone(), move |held| {
                held.compare_forecast = on
            });
        },
    );
    let clearable = Signal::derive(move || computed.with(|computed| computed.live > 0));
    view! {
        row(class = "geo__bar", on:pointer_down:stop = |_| {}) {
            column(class = "geo__entry") { text(class = "geo__label") {"Half-life"} {half_life} }
            column(class = "geo__entry") { text(class = "geo__label") {"Reflection"} {height} }
            {layers}
            row(class = "geo__check") { {paths} text(class = "geo__label") {"Paths"} }
            row(class = "geo__check") { {sondes} text(class = "geo__label") {"Ionosondes"} }
            {armed_clear(clearable, move || session.update(|session| {
                session.observations.clear();
                session.cleared_at = now_ms();
            }))}
        }
    }
}

fn strip(settings: Memo<PropagationNode>, computed: RwSignal<Computed>) -> impl IntoView {
    let read = move |pick: fn(&Computed) -> String| move || computed.with(pick);
    view! {
        row(class = "geo__strip") {
            row(class = "geo__pair") { text(class = "geo__num") {{read(|c| c.summary.decodes.to_string())}} text {"decodes"} }
            row(class = "geo__pair") { text(class = "geo__num") {{read(|c| c.summary.grids.to_string())}} text {"grids"} }
            row(class = "geo__pair") { text(class = "geo__num") {{read(|c| c.cells.len().to_string())}} text {"cells"} }
            row(class = "geo__pair") { text {"highest"} text(class = "geo__num") {{read(|c| mhz(c.summary.best_freq_hz))}} }
            row(class = "geo__pair") { text {"MUF(3000)"} text(class = "geo__num") {{read(|c| c.summary.best_muf3000_mhz.map_or_else(|| String::from("-"), |muf| format!("\u{2265} {muf:.1} MHz")))}} }
            row(class = "geo__pair") { text {"farthest"} text(class = "geo__num") {{read(|c| format!("{:.0} km", c.summary.farthest_km))}} }
            if move || settings.get().compare_forecast {
                row(class = "geo__pair") { text {"overhead"} text(class = "geo__num") {{read(|c| c.overhead.as_ref().map_or_else(|| String::from("-"), |forecast| format!("{:.1} MHz", forecast.muf3000_mhz)))}} }
            }
        }
    }
}

fn cell(text: String) -> AnyView {
    AnyView::new(view! { text(class = "geo__cell") {{text}} })
}

fn table(settings: Memo<PropagationNode>, computed: RwSignal<Computed>) -> impl IntoView {
    move || {
        let compare = settings.get().compare_forecast;
        let rows: Vec<Vec<AnyView>> = computed.with(|computed| {
            if compare {
                let mut sorted = computed.comparisons.clone();
                sorted.sort_by(|a, b| b.cell.weight.total_cmp(&a.cell.weight));
                sorted
                    .into_iter()
                    .take(TABLE_ROWS)
                    .map(|row| {
                        let colour = format!("#{:06x}", layers::muf_colour(row.measured_muf3000_mhz));
                        vec![
                            cell(row.cell.key.clone()),
                            cell(row.cell.decodes.to_string()),
                            cell(mhz(row.cell.best_freq_hz)),
                            AnyView::new(view! {
                                text(class = "geo__cell", style:color = Some(colour)) {{format!("\u{2265} {:.1}", row.measured_muf3000_mhz)}}
                            }),
                            cell(format!("{:.1}", row.forecast.muf3000_mhz)),
                            cell(format!("{:+.1}", row.delta_mhz)),
                        ]
                    })
                    .collect()
            } else {
                computed
                    .cells
                    .iter()
                    .take(TABLE_ROWS)
                    .map(|found| {
                        vec![
                            cell(found.key.clone()),
                            cell(found.decodes.to_string()),
                            cell(mhz(found.best_freq_hz)),
                            cell(found.measured_muf3000_mhz.map_or_else(|| String::from("-"), |muf| format!("\u{2265} {muf:.1}"))),
                        ]
                    })
                    .collect()
            }
        });
        if rows.is_empty() {
            return AnyView::new(view! { text(class = "geo__note") {"No reflection points yet"} });
        }
        let mut head = vec!["Midpoint", "Decodes", "Highest", "Measured MUF"];
        if compare {
            head.extend(["Forecast", "\u{0394}"]);
        }
        let head: Vec<AnyView> = head.into_iter().map(|name| cell(name.to_owned())).collect();
        let body: Vec<AnyView> = rows
            .into_iter()
            .map(|cells| AnyView::new(view! { row(class = "geo__row") {{cells}} }))
            .collect();
        AnyView::new(view! {
            column(class = "geo__table") {
                row(class = "geo__row head") {{head}}
                {body}
            }
        })
    }
}

fn notice(
    settings: Memo<PropagationNode>,
    report: RwSignal<Option<Result<IonosondeReport, String>>>,
    computed: RwSignal<Computed>,
) -> impl IntoView {
    let text = move || -> (bool, String) {
        if !settings.get().compare_forecast {
            return (
                false,
                String::from(
                    "Measured MUF is a floor: the highest decode per path, projected onto 3000 km",
                ),
            );
        }
        match report.get() {
            None => (false, String::from("Waiting for the ionosonde network")),
            Some(Err(error)) => (true, format!("Ionosonde feed: {error}")),
            Some(Ok(report)) => match &report.error {
                Some(error) => (true, format!("Ionosonde feed: {error}")),
                None if report.stations.is_empty() => {
                    (false, String::from("Waiting for the ionosonde network"))
                }
                None => {
                    let agreement = computed.with(|computed| computed.agreement.clone());
                    (
                        false,
                        format!(
                            "{} sounding sites · {} of {} cells above forecast · median \u{0394} {:+.1} MHz · {}",
                            report.stations.len(),
                            agreement.above,
                            agreement.cells,
                            agreement.median_delta_mhz,
                            report.source
                        ),
                    )
                }
            },
        }
    };
    view! {
        text(class = "geo__note", class:bad = move || text().0) {{move || text().1}}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_query_names_every_wired_decoder() {
        let inputs = [
            Input {
                node: "ft8 a".to_owned(),
                device_set: 1,
                channel: 2,
            },
            Input {
                node: "wspr".to_owned(),
                device_set: 1,
                channel: 3,
            },
        ];
        let path = history_path("ft8", &inputs, 0);
        assert!(path.starts_with(
            "/api/decoderlog?kind=ft8&nodes=ft8%20a%2Cwspr&sources=1%3A2%2C1%3A3&since="
        ));
        assert!(path.ends_with("&limit=2000"));
    }

    #[test]
    fn an_empty_session_computes_an_empty_picture() {
        let computed = compute(
            &[],
            0,
            PropagationNode::default(),
            None,
            &[],
            Layer::Activity,
            0,
        );
        assert_eq!(computed.live, 0);
        assert!(computed.cells.is_empty());
        assert!(computed.overhead.is_none());
    }
}
