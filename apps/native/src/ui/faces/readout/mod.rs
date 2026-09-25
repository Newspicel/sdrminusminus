mod pictures;
mod targets;
mod text;
mod views;

use std::sync::Arc;

use sdrmm_wire::patch::NodeBody;
use zgui::prelude::*;

use crate::{
    decoded::{Frames, Stations},
    decoders::{
        views::{DecoderScope, Records, in_scope},
        wiring::hears_monitor,
    },
    store::Store,
    ui::{faces::decoder_log, kit_decoders},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Wired {
    node: String,
    label: String,
    kind: String,
    scope: DecoderScope,
}

impl Wired {
    fn key(&self) -> String {
        format!(
            "{}/{}/{:?}/{:?}",
            self.node, self.kind, self.scope.device_set, self.scope.channel
        )
    }
}

const VIEWED: [&str; 14] = [
    "rds",
    "adsb",
    "ais",
    "rtty",
    "morse",
    "psk",
    "cw_skimmer",
    "tone",
    "ident",
    "broadcast",
    "broadcast_data",
    "sstv",
    "vor",
    "dect",
];

#[must_use]
pub fn has_view(kind: &str) -> bool {
    VIEWED.contains(&kind)
}

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_decoders::install();
    let monitor = {
        let node = node.clone();
        Memo::new(move |_| hears_monitor(&store.graph.get(), &node))
    };
    move || {
        if monitor.get() {
            AnyView::new(decoder_log::panel(store, node.clone()))
        } else {
            AnyView::new(readout(store, node.clone()))
        }
    }
}

fn wired(store: Store, node: &str) -> (usize, Vec<Wired>) {
    let graph = store.graph.get();
    let sources: Vec<String> = graph
        .sources_of(node, "events")
        .map(str::to_owned)
        .collect();
    let readable = sources
        .iter()
        .filter_map(|source| {
            let found = graph.node(source)?;
            let NodeBody::Channel(channel) = &found.body else {
                return None;
            };
            let kind = store.descriptor_of(&channel.channel_type)?.decoder_kind?;
            has_view(&kind).then_some(())?;
            let info = store.channel_of(source)?;
            Some(Wired {
                node: source.clone(),
                label: found
                    .label
                    .clone()
                    .unwrap_or_else(|| channel.channel_type.to_uppercase()),
                kind,
                scope: DecoderScope {
                    device_set: store.device_set_of(source),
                    channel: Some(info.id),
                },
            })
        })
        .collect();
    (sources.len(), readable)
}

fn readout(store: Store, node: String) -> impl IntoView {
    let inputs = Memo::new(move |_| wired(store, &node));
    let hint = move || {
        let (count, readable) = inputs.get();
        let said = if count == 0 {
            Some("Wire a decoder's events in")
        } else if readable.is_empty() {
            Some("No wired decoder builds up a picture")
        } else {
            None
        };
        said.map(|said| view! { text(class = "hint") {{said}} })
    };
    view! {
        column(class = "face", {..kit_decoders::no_pan()}) {
            {hint}
            for input in move || inputs.get().1, key = |input: &Wired| input.key() {
                column(class = "dk-pane") {
                    if move || inputs.with(|(_, readable)| readable.len() > 1) {
                        text(class = "legend") {{input.label.clone()}}
                    }
                    {views::decoder_view(store, &input.kind, input.scope)}
                }
            }
        }
    }
}

fn frames_changed(a: Option<&Option<Frames>>, b: Option<&Option<Frames>>) -> bool {
    match (a, b) {
        (Some(Some(a)), Some(Some(b))) => !Arc::ptr_eq(a, b),
        (Some(None), Some(None)) => false,
        _ => true,
    }
}

fn stations_changed(a: Option<&Option<Stations>>, b: Option<&Option<Stations>>) -> bool {
    match (a, b) {
        (Some(Some(a)), Some(Some(b))) => !Arc::ptr_eq(a, b),
        (Some(None), Some(None)) => false,
        _ => true,
    }
}

pub(super) fn frames_of(store: Store, kind: &'static str) -> Memo<Option<Frames>> {
    Memo::new_with_compare(
        move |_| store.decoded.with(|decoded| decoded.frames(kind).cloned()),
        frames_changed,
    )
}

pub(super) fn stations_of(store: Store, kind: &'static str) -> Memo<Option<Stations>> {
    Memo::new_with_compare(
        move |_| {
            store
                .decoded
                .with(|decoded| decoded.stations(kind).cloned())
        },
        stations_changed,
    )
}

pub(super) fn records(frames: Memo<Option<Frames>>, scope: DecoderScope) -> Records {
    frames.with(|frames| {
        frames
            .as_ref()
            .map(|frames| in_scope(frames.iter(), scope))
            .unwrap_or_default()
    })
}
