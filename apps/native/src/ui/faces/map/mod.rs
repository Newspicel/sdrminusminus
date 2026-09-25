pub mod df;
pub mod targets;

use std::{sync::Arc, time::Duration};

use sdrmm_wire::channel::ChannelParams;
use zgui::prelude::*;
use zgui::reactive::RenderEffect;

use crate::{
    store::Store,
    ui::{
        kit_maps::{
            self, SHEET,
            feed::{Feed, Held, Topic},
            mhz, now_ms, ticker, trail,
            wiring::{channel_type_of, event_sources},
        },
        map::{self, ACCENT, Frame, Geo, MapProps, Overlay},
    },
};

const DRAW_TICK: Duration = Duration::from_millis(500);
const AGE_OUT_EVERY: Duration = Duration::from_secs(15);
const TARGET_FRAME_ZOOM: f64 = 9.0;
const TRAIL_FRAME_ZOOM: f64 = 14.0;

#[derive(Clone, Debug, Default, PartialEq)]
struct Wiring {
    kinds: Vec<&'static str>,
    positions: Vec<String>,
    df: df::Sources,
    references: Vec<Geo>,
}

fn wiring(store: Store, node: &str) -> Wiring {
    let graph = store.graph.get();
    let sources = event_sources(&graph, node);
    let kinds: Vec<String> = sources
        .iter()
        .filter_map(|source| channel_type_of(&graph, source))
        .filter_map(|kind| store.descriptor_of(&kind)?.decoder_kind)
        .collect();
    let params: Vec<ChannelParams> = sources
        .iter()
        .filter_map(|source| store.channel_of(source))
        .map(|channel| channel.settings.params)
        .collect();
    Wiring {
        kinds: targets::map_kinds_of(&kinds),
        positions: trail::positions_of(&graph, node),
        df: df::sources_of(&graph, node),
        references: targets::references(&params),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Drawn {
    overlay: Arc<Overlay>,
    counts: Vec<(&'static str, usize)>,
    trail: usize,
    frame: Option<Arc<Frame>>,
}

fn draw(held: &Held, wired: &Wiring, selected: Option<&str>, now: i64) -> Drawn {
    let tracks: Vec<_> = wired
        .positions
        .iter()
        .filter_map(|node| held.tracks.get(node))
        .collect();
    let from = tracks
        .iter()
        .find_map(|track| track.fix.as_ref())
        .map(|fix| Geo::new(fix.latitude, fix.longitude));
    let mut overlay = Overlay {
        marks: targets::reference_marks(&wired.references),
        ..Overlay::default()
    };
    overlay.extend(trail::overlay(&tracks));
    let picture = df::picture(&wired.df, |node| held.finders.get(node).cloned(), now, from);
    overlay.extend(df::overlay(&picture));
    let mut counts = Vec::new();
    let mut target_points = Vec::new();
    for kind in &wired.kinds {
        let stations: Vec<_> = held
            .stations
            .get(kind)
            .map(|stations| stations.values().collect())
            .unwrap_or_default();
        let (drawn, shown) = targets::overlay(&stations, selected, now);
        target_points.extend(drawn.dots.iter().map(|dot| dot.at));
        overlay.extend(drawn);
        counts.push((*kind, shown));
    }
    let trail_points = trail::points(&tracks);
    let frame = if target_points.is_empty() {
        (!trail_points.is_empty()).then(|| Frame {
            points: trail_points.clone(),
            max_zoom: TRAIL_FRAME_ZOOM,
        })
    } else {
        Some(Frame {
            points: target_points,
            max_zoom: TARGET_FRAME_ZOOM,
        })
    };
    Drawn {
        overlay: Arc::new(overlay),
        counts,
        trail: trail_points.len(),
        frame: frame.map(Arc::new),
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("kit-maps", SHEET);
    install_stylesheet("map-face", FACE_SHEET);
    let feed = Feed::get();
    feed.listen(store);
    let now = ticker(DRAW_TICK);
    {
        let feed = feed.clone();
        kit_maps::every(AGE_OUT_EVERY, move || {
            feed.change(Topic::Stations, |held| {
                held.age_out(targets::TARGET_MAX_AGE_MS, now_ms())
            });
        });
    }
    let wired = {
        let node = node.clone();
        Memo::new(move |_| wiring(store, &node))
    };
    let selected = RwSignal::new(None::<String>);
    let drawn = RwSignal::new(Drawn::default());
    let drawing = {
        let feed = feed.clone();
        RenderEffect::new(move |_| {
            let at = now.get();
            let wiring = wired.get();
            let chosen = selected.get();
            let next = draw(&feed.held.borrow(), &wiring, chosen.as_deref(), at);
            drawn.set(next);
        })
    };
    on_cleanup_local(move || drop(drawing));

    let active = {
        let node = node.clone();
        Signal::derive(move || store.selected.get().as_deref() == Some(node.as_str()))
    };
    let props = MapProps {
        overlay: Signal::derive(move || drawn.with(|drawn| drawn.overlay.clone())),
        active,
        frame: Signal::derive(move || drawn.with(|drawn| drawn.frame.clone())),
        on_pick: Some(UnsyncCallback::new(move |hit: Option<String>| {
            selected.set(hit)
        })),
    };
    let chrome = AnyView::new(view! {
        {legend(drawn, wired)}
        {detail(feed, selected)}
    });
    view! {
        column(class = "geo map-face") {
            {map::map(store, props, chrome)}
        }
    }
}

fn legend(drawn: RwSignal<Drawn>, wired: Memo<Wiring>) -> impl IntoView {
    move || {
        let counts = drawn.with(|drawn| drawn.counts.clone());
        let trail = wired.with(|wired| !wired.positions.is_empty());
        if counts.is_empty() && !trail {
            return None;
        }
        let rows: Vec<AnyView> = counts
            .into_iter()
            .map(|(kind, _)| {
                let (title, colour) = targets::style(kind);
                let count = move || {
                    drawn.with(|drawn| {
                        drawn
                            .counts
                            .iter()
                            .find(|(held, _)| *held == kind)
                            .map_or(0, |(_, count)| *count)
                            .to_string()
                    })
                };
                AnyView::new(trail::legend_row(colour, title, count))
            })
            .collect();
        let trail_row = trail.then(|| {
            AnyView::new(trail::legend_row(ACCENT, "GPS trail", move || {
                drawn.with(|drawn| drawn.trail.to_string())
            }))
        });
        Some(view! {
            column(class = "geo__legend") {
                {rows}
                {trail_row}
            }
        })
    }
}

fn detail(feed: std::rc::Rc<Feed>, selected: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        let key = selected.get()?;
        let (kind, id) = key.split_once('/')?;
        let shown = feed.with(Topic::Stations, |held| {
            held.stations
                .get(kind)
                .and_then(|stations| stations.get(id))
                .map(targets::detail)
        });
        let Some(shown) = shown else {
            selected.set(None);
            return None;
        };
        let (_, colour) = targets::style(shown.kind);
        let rows: Vec<AnyView> = shown
            .rows
            .into_iter()
            .map(|(name, value)| {
                AnyView::new(view! {
                    row(class = "map-face__row") {
                        text(class = "map-face__name") {{name}}
                        text(class = "map-face__value") {{value}}
                    }
                })
            })
            .collect();
        let foot = format!(
            "{} · last seen {}",
            mhz(shown.freq_hz),
            kit_maps::utc_clock(shown.last_seen)
        );
        Some(view! {
            column(class = "map-face__card", on:pointer_down:stop = |_| {}) {
                row(class = "map-face__head") {
                    text(class = "map-face__title", style:color = Some(format!("#{colour:06x}"))) {{shown.label}}
                    spacer()
                    control(
                        class = "map-face__close",
                        a11y:label = "Clear target selection",
                        on:click:stop = move |_| selected.set(None)
                    ) {"x"}
                }
                column(class = "map-face__rows") {{rows}}
                text(class = "map-face__foot") {{foot}}
            }
        })
    }
}

pub const FACE_SHEET: &str = css!(
    r#"
.map-face { min-height: 300px; }
.map-face__card {
    position: absolute;
    right: 8px;
    bottom: 20px;
    width: 240px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--panel);
    cursor: default;
}
.map-face__head { align-items: center; gap: 6px; padding: 3px 8px; border-bottom: 1px solid var(--line); }
.map-face__title { font-family: var(--mono); font-size: 13px; }
.map-face__close { color: var(--ink-dim); font-family: var(--mono); font-size: 11px; padding: 0 4px; }
.map-face__close:hover { color: var(--ink); }
.map-face__rows { padding: 4px 8px; gap: 1px; }
.map-face__row { justify-content: space-between; gap: 10px; }
.map-face__name { font-size: 11px; color: var(--ink-faint); }
.map-face__value { font-family: var(--mono); font-size: 11px; color: var(--ink); }
.map-face__foot { padding: 3px 8px; border-top: 1px solid var(--line); font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
"#
);
