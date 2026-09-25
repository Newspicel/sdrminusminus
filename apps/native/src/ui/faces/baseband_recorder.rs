use zgui::prelude::*;

use crate::{
    binding::{self, Input},
    store::Store,
    ui::kit_audio::{self, format_bytes, readout},
};

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    let inputs = {
        let node = node.clone();
        Signal::derive(move || {
            let state = store.state.get();
            binding::inputs_of(
                &store.graph.get(),
                &node,
                "baseband",
                &state.device_sets,
                &state.trunk_systems,
            )
        })
    };
    let recording = kit_audio::is_recording(store, node.clone());
    view! {
        column(class = "face") {
            if move || inputs.get().is_empty() {
                text(class = "hint") {"Wire a channel's baseband in"}
            } else {
                row(class = "field__body") {
                    {kit_audio::recorder_switch(store, node.clone())}
                }
            }
            for input in move || inputs.get(), key = |input: &Input| input.node.clone() {
                {lane(store, input, recording)}
            }
        }
    }
}

fn lane(store: Store, input: Input, recording: Signal<bool>) -> impl IntoView {
    let name = {
        let node = input.node.clone();
        let kind = input.channel.settings.params.type_id().to_owned();
        move || kit_audio::node_label(store, &node, &kind)
    };
    let node = input.node;
    let status = Signal::derive(move || {
        store
            .channel_of(&node)
            .and_then(|channel| channel.baseband_recording)
    });
    let written = move || {
        status
            .get()
            .map_or_else(String::new, |status| format_bytes(status.bytes))
    };
    let samples = move || status.get().map_or(0, |status| status.samples).to_string();
    let drops = move || status.get().map_or(0, |status| status.overruns).to_string();
    let file = move || status.get().map(|status| status.file).unwrap_or_default();
    view! {
        column(class = "lane") {
            row(class = "lane__head") {
                text(class = "lane__name") {{name}}
                if move || recording.get() && status.get().is_none() {
                    text(class = "hint") {"Waiting"}
                }
            }
            if move || status.get().is_some() {
                {readout(vec![
                    ("Written", AnyView::new(written)),
                    ("Samples", AnyView::new(samples)),
                    ("Drops", AnyView::new(drops)),
                    ("File", AnyView::new(file)),
                ])}
            }
            {kit_audio::alert(move || status.get().and_then(|status| status.error))}
        }
    }
}
