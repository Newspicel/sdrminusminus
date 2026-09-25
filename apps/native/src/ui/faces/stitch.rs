use sdrmm_wire::{
    coherent::{MAX_ARRAY_ELEMENTS, MIN_ARRAY_ELEMENTS, StitchMode, StitchParams},
    patch::NodeBody,
    state::ExtraLane,
};
use zgui::{prelude::*, vocab::SharedString};

use super::df::SHEET;
use crate::{
    binding, format,
    store::Store,
    ui::{
        kit_raster::{Bounds, edit_body, number, readout},
        widgets::{pick, row_field},
    },
};

#[must_use]
pub fn mode_note(mode: StitchMode) -> &'static str {
    match mode {
        StitchMode::Auto => "Tunes the lanes side by side into one gapless span",
        StitchMode::Manual => "Keeps each lane where it is tuned and joins what they cover",
    }
}

#[must_use]
pub fn lane_rows(lane: Option<ExtraLane>) -> Vec<(String, String)> {
    lane.map_or_else(Vec::new, |lane| {
        vec![
            ("Center".into(), format::frequency(lane.center_hz)),
            ("Rate".into(), format::rate(lane.sample_rate)),
        ]
    })
}

pub(crate) fn carrier_set(store: Store, node: &str) -> Option<u32> {
    let graph = store.graph.get();
    let devices = binding::device_sets(&graph, &store.state.get().device_sets);
    devices.get(node).copied().or_else(|| {
        let (source, _) = binding::iq_source_of(&graph, node)?;
        devices.get(&source).copied()
    })
}

fn change(store: Store, node: &str, edit: impl FnOnce(&mut StitchParams)) {
    edit_body(store, node, |body| {
        if let NodeBody::Stitch(stitch) = body {
            edit(&mut stitch.settings);
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-coherent", SHEET);
    let lane = {
        let node = node.clone();
        Signal::derive(move || {
            carrier_set(store, &node)
                .and_then(|set| store.set_of(set))
                .and_then(|set| set.extra_lane)
        })
    };
    let settings = {
        let node = node.clone();
        Memo::new(move |_| {
            store
                .graph
                .get()
                .node(&node)
                .and_then(|found| match &found.body {
                    NodeBody::Stitch(stitch) => Some(stitch.settings),
                    _ => None,
                })
        })
    };
    let mode = Signal::derive(move || settings.get().map(|params| params.mode));
    let lanes = Signal::derive(move || {
        settings
            .get()
            .map(|params| f64::from(params.lanes))
            .unwrap_or_default()
    });
    let write = move |edit: Box<dyn FnOnce(&mut StitchParams)>| change(store, &node, edit);
    let pick_mode = {
        let write = write.clone();
        move |chosen: StitchMode| write(Box::new(move |params| params.mode = chosen))
    };
    let set_lanes = move |count: f64| write(Box::new(move |params| params.lanes = count as u32));
    let note = move || mode.get().map(mode_note).unwrap_or_default();

    view! {
        column(class = "face") {
            {readout(move || lane_rows(lane.get()))}
            column(class = "params") {
                box(a11y:description = move || SharedString::from(note())) {
                    {row_field("Mode", pick(
                        vec![(StitchMode::Auto, "Auto".to_owned()), (StitchMode::Manual, "Manual".to_owned())],
                        mode,
                        pick_mode,
                    ))}
                }
                {row_field("Lanes", number(
                    "Lanes wired in",
                    lanes,
                    Bounds::whole(f64::from(MIN_ARRAY_ELEMENTS), f64::from(MAX_ARRAY_ELEMENTS)),
                    set_lanes,
                ))}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_reads_its_centre_and_rate() {
        let rows = lane_rows(Some(ExtraLane {
            stream: 1,
            center_hz: 100e6,
            sample_rate: 2.4e6,
        }));
        assert_eq!(rows[0].1, "100.000000 MHz");
        assert_eq!(rows[1].1, "2.400 MS/s");
        assert!(lane_rows(None).is_empty());
    }

    #[test]
    fn the_default_stitch_is_one_the_server_accepts() {
        assert!(StitchParams::default().valid());
        assert!(mode_note(StitchMode::Auto).contains("gapless"));
    }
}
