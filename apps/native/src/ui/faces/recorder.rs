use std::time::Duration;

use zgui::prelude::*;

use super::set_signal;
use crate::{
    store::Store,
    ui::kit_audio::{self, format_bytes, format_duration, readout, recording_elapsed_s},
};

const TICK: Duration = Duration::from_secs(1);

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    let set = set_signal(store, node.clone());
    let recording = kit_audio::is_recording(store, node.clone());
    let status = Signal::derive(move || set.get().and_then(|set| set.recording));
    let rate = Signal::derive(move || {
        set.get()
            .and_then(|set| set.settings.sample_rate)
            .unwrap_or(0.0)
    });
    let now = kit_audio::ticker(TICK);
    let hint = move || {
        if set.get().is_none() {
            "Wire a device's IQ in"
        } else if recording.get() {
            "Waiting"
        } else {
            ""
        }
    };
    let elapsed = move || {
        status.get().map_or_else(String::new, |status| {
            format_duration(recording_elapsed_s(&status, now.get(), rate.get()))
        })
    };
    let written = move || {
        status
            .get()
            .map_or_else(String::new, |status| format_bytes(status.bytes))
    };
    let drops = move || status.get().map_or(0, |status| status.overruns).to_string();
    let file = move || status.get().map(|status| status.file).unwrap_or_default();
    view! {
        column(class = "face") {
            if move || status.get().is_none() {
                text(class = "hint") {{hint}}
            } else {
                {readout(vec![
                    ("Elapsed", AnyView::new(elapsed)),
                    ("Written", AnyView::new(written)),
                    ("Drops", AnyView::new(drops)),
                    ("File", AnyView::new(file)),
                ])}
            }
            {kit_audio::alert(move || status.get().and_then(|status| status.error))}
            if move || set.get().is_some() {
                row(class = "face__foot") {
                    {kit_audio::recorder_switch(store, node.clone())}
                }
            }
        }
    }
}
