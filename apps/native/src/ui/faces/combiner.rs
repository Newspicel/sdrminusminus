use sdrmm_wire::{
    coherent::{
        CombineMode, CombinerParams, MAX_ARRAY_ELEMENTS, MAX_DF_BANDWIDTH_HZ, MAX_DF_OFFSET_HZ,
        MAX_DF_REPORT_MS, MIN_ARRAY_ELEMENTS, MIN_DF_BANDWIDTH_HZ, MIN_DF_REPORT_MS,
    },
    patch::NodeBody,
};
use zgui::{prelude::*, vocab::SharedString};

use super::df::{SHEET, cal_rows, calibrate, finder_signal};
use crate::{
    store::Store,
    ui::{
        kit_raster::{Bounds, button, edit_body, number, readout},
        widgets::{pick, row_field},
    },
};

#[must_use]
pub fn mode_note(mode: CombineMode) -> &'static str {
    match mode {
        CombineMode::Diversity => "Every antenna turned into step and added: about 3 dB for two",
        CombineMode::Cancel => "The first antenna kept, what the others hear subtracted from it",
    }
}

fn change(store: Store, node: &str, edit: impl FnOnce(&mut CombinerParams)) {
    edit_body(store, node, |body| {
        if let NodeBody::Combiner(combiner) = body {
            edit(&mut combiner.settings);
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-coherent", SHEET);
    let finder = finder_signal(store, node.clone());
    let settings = {
        let node = node.clone();
        Memo::new(move |_| {
            store
                .graph
                .get()
                .node(&node)
                .and_then(|found| match &found.body {
                    NodeBody::Combiner(combiner) => Some(combiner.settings),
                    _ => None,
                })
        })
    };
    let read = move |pick: fn(&CombinerParams) -> f64| {
        Signal::derive(move || settings.get().as_ref().map(pick).unwrap_or_default())
    };
    let press = {
        let node = node.clone();
        move || calibrate(store, node.clone())
    };
    let write = move |edit: Box<dyn FnOnce(&mut CombinerParams)>| change(store, &node, edit);
    let mode = Signal::derive(move || settings.get().map(|params| params.mode));
    let note = move || mode.get().map(mode_note).unwrap_or_default();
    let pick_mode = {
        let write = write.clone();
        move |chosen: CombineMode| write(Box::new(move |params| params.mode = chosen))
    };
    let lanes = {
        let write = write.clone();
        move |count: f64| write(Box::new(move |params| params.lanes = count as u32))
    };
    let offset = {
        let write = write.clone();
        move |hz: f64| write(Box::new(move |params| params.offset_hz = hz))
    };
    let bandwidth = {
        let write = write.clone();
        move |hz: f64| write(Box::new(move |params| params.bandwidth_hz = hz))
    };
    let every = move |ms: f64| write(Box::new(move |params| params.update_ms = ms as u32));

    view! {
        column(class = "face") {
            {readout(move || cal_rows(finder.get().as_ref()))}
            row(class = "section") {
                {button("Calibrate", Signal::stored(false), press)}
            }
            box(class = "rule") {}
            column(class = "params") {
                box(a11y:description = move || SharedString::from(note())) {
                    {row_field("Mode", pick(
                        vec![
                            (CombineMode::Diversity, "Diversity".to_owned()),
                            (CombineMode::Cancel, "Cancel".to_owned()),
                        ],
                        mode,
                        pick_mode,
                    ))}
                }
                {row_field("Antennas", number(
                    "Antennas wired in",
                    read(|params| f64::from(params.lanes)),
                    Bounds::whole(f64::from(MIN_ARRAY_ELEMENTS), f64::from(MAX_ARRAY_ELEMENTS)),
                    lanes,
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
                {row_field("Solve ms", number(
                    "Weights solved every",
                    read(|params| f64::from(params.update_ms)),
                    Bounds::whole(f64::from(MIN_DF_REPORT_MS), f64::from(MAX_DF_REPORT_MS)),
                    every,
                ))}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_combiner_is_one_the_server_accepts() {
        assert!(CombinerParams::default().valid());
        assert_eq!(CombinerParams::default().mode, CombineMode::Diversity);
    }

    #[test]
    fn every_mode_says_what_it_does() {
        assert!(mode_note(CombineMode::Diversity).contains("added"));
        assert!(mode_note(CombineMode::Cancel).contains("subtracted"));
    }
}
