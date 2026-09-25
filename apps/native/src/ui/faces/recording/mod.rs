pub mod library;
pub mod playback;

use std::path::PathBuf;

use sdrmm_wire::{
    RecordingInfo, RecordingsResponse,
    patch::NodeBody,
    state::DeviceSetStatus,
    ws::{ServerEvent, StateScope},
};
use zgui::prelude::*;

use self::library::{
    check_upload, claimed_recordings, describe_recording, find_recording, recording_choices,
    recording_device_id, recording_provenance, recording_title, upload_field,
};
use super::{
    device::{
        actions::{Busy, create_set, edit_body, release},
        devices::device_id,
        radio::Radio,
    },
    set_signal,
};
use crate::{
    store::Store,
    ui::kit_sources::{Tone, button, draft_field, footer, install, readout, units},
};

type Library = RwSignal<Option<Result<Vec<RecordingInfo>, String>>>;

fn library(store: Store) -> Library {
    let shelf: Library = RwSignal::new(None);
    let fetch = move || {
        zgui::task::spawn_local(async move {
            let fetched = store
                .api()
                .get::<RecordingsResponse>("/api/recordings")
                .await
                .map(|response| response.recordings)
                .map_err(|error| error.to_string());
            shelf.try_set(Some(fetched));
        });
    };
    fetch();
    store.on_event(move |event| {
        if matches!(
            event,
            ServerEvent::StateChanged {
                scope: StateScope::Recordings | StateScope::All
            }
        ) {
            fetch();
        }
    });
    shelf
}

fn stem_of(store: Store, node: &str) -> Option<String> {
    store.graph.with(|graph| {
        graph.node(node).and_then(|found| match &found.body {
            NodeBody::Recording(recording) => {
                recording.recording.clone().filter(|stem| !stem.is_empty())
            }
            _ => None,
        })
    })
}

fn name_recording(store: Store, node: String, stem: Option<String>) {
    edit_body(store, node, move |body| {
        if let NodeBody::Recording(recording) = body {
            recording.recording = stem;
        }
    });
}

#[derive(Clone, PartialEq)]
enum Phase {
    Unnamed,
    Named(String),
    Open(String),
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install();
    let set = set_signal(store, node.clone());
    let shelf = library(store);
    let busy = Busy::new();
    let phase = {
        let node = node.clone();
        Memo::new(
            move |_| match (stem_of(store, &node), set.with(Option::is_some)) {
                (None, _) => Phase::Unnamed,
                (Some(stem), false) => Phase::Named(stem),
                (Some(stem), true) => Phase::Open(stem),
            },
        )
    };
    move || match phase.get() {
        Phase::Unnamed => AnyView::new(pick_view(store, node.clone(), shelf, busy)),
        Phase::Named(stem) => AnyView::new(named(store, node.clone(), stem, shelf, busy)),
        Phase::Open(stem) => AnyView::new(playing(
            Radio { store, set },
            node.clone(),
            stem,
            shelf,
            busy,
        )),
    }
}

fn pick(store: Store, node: String, busy: Busy, recording: RecordingInfo) {
    let open = store.state.with_untracked(|state| {
        state
            .device_sets
            .iter()
            .any(|set| device_id(&set.device) == recording.device_id)
    });
    busy.run(store, async move {
        if !open {
            create_set(store, recording.device_id.clone()).await?;
        }
        name_recording(store, node, Some(recording.file));
        Ok(())
    });
}

fn pick_view(store: Store, node: String, shelf: Library, busy: Busy) -> impl IntoView {
    let search = RwSignal::new_local(String::new());
    let choices = {
        let node = node.clone();
        Signal::derive_local(move || {
            let all = shelf.get().and_then(Result::ok).unwrap_or_default();
            let claimed = store.graph.with(|graph| claimed_recordings(graph, &node));
            recording_choices(&all, &claimed, &search.get())
        })
    };
    let count = Signal::derive(move || {
        shelf.with(|shelf| {
            shelf
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .map_or(0, Vec::len)
        })
    });
    let list = {
        let node = node.clone();
        move || {
            choices
                .get()
                .into_iter()
                .map(|recording| {
                    let meta = describe_recording(&recording);
                    let title = recording_title(&recording);
                    let tip = recording_provenance(&recording);
                    let node = node.clone();
                    AnyView::new(view! {
                        control(
                            class = "kit-choice",
                            tabindex = Focus::Sequential,
                            a11y:role = Role::Button,
                            a11y:description = tip,
                            state:disabled = busy.busy,
                            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                            on:click:stop = move |_| pick(store, node.clone(), busy, recording.clone())
                        ) {
                            text {{title}}
                            text(class = "kit-choice__meta") {{meta}}
                        }
                    })
                })
                .collect::<Vec<_>>()
        }
    };
    let said = move || match shelf.get() {
        None => Some("Reading the library…".to_owned()),
        Some(Err(error)) => Some(error),
        Some(Ok(all)) if all.is_empty() => {
            Some("No recordings yet. Drop a .sigmf here.".to_owned())
        }
        Some(Ok(_)) if choices.with(Vec::is_empty) => Some("Nothing free matches that.".to_owned()),
        Some(Ok(_)) => None,
    };
    let dropped = move |ev: &mut EventCx<'_, events::Drop>| {
        upload(store, node.clone(), busy, ev.paths.clone())
    };
    view! {
        column(class = "face kit-drop", on:drop = dropped) {
            box(hidden = move || count.get() == 0) {
                {draft_field("Search recordings", search, "Search recordings", Signal::derive_local(|| false), || {})}
            }
            column(class = "kit-list") {{list}}
            {move || said().map(|said| AnyView::new(view! { text(class = "kit-note") {{said}} }))}
        }
    }
}

fn upload(store: Store, node: String, busy: Busy, paths: Vec<PathBuf>) {
    let names: Vec<String> = paths
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    if let Some(problem) = check_upload(&names) {
        store.say(problem.said());
        return;
    }
    busy.run(store, async move {
        let parts = zgui::task::blocking(move || {
            paths
                .into_iter()
                .zip(names)
                .map(|(path, name)| std::fs::read(&path).map(|bytes| (name, bytes)))
                .collect::<std::io::Result<Vec<_>>>()
        })
        .await?;
        let mut form = reqwest::multipart::Form::new();
        for (name, bytes) in parts {
            let field = upload_field(&name);
            form = form.part(
                field,
                reqwest::multipart::Part::bytes(bytes).file_name(name),
            );
        }
        let recording: RecordingInfo = store.api().multipart("/api/recordings", form).await?;
        pick(store, node, busy, recording);
        Ok(())
    });
}

fn named(store: Store, node: String, stem: String, shelf: Library, busy: Busy) -> impl IntoView {
    let known = {
        let stem = stem.clone();
        Signal::derive(move || {
            shelf
                .get()
                .and_then(Result::ok)
                .and_then(|all| find_recording(&all, Some(&stem)).cloned())
        })
    };
    let gone = Signal::derive(move || {
        shelf.with(|shelf| matches!(shelf, Some(Ok(_)))) && known.with(Option::is_none)
    });
    let blocked = Signal::derive(move || gone.get() || busy.busy.get());
    let forget_node = node.clone();
    let play = move || {
        let stem = stem.clone();
        busy.run(store, async move {
            create_set(store, recording_device_id(&stem)).await?;
            store.apply().await;
            Ok(())
        });
    };
    view! {
        column(class = "face") {
            row(class = "kit-head", hidden = move || !gone.get()) {
                spacer() {}
                text(class = "kit-legend warn") {"missing"}
            }
            text(class = "kit-mono") {{move || known.get().map(|known| known.file).unwrap_or_else(|| stem_of(store, &node).unwrap_or_default())}}
            text(class = "kit-meta") {{move || known.get().map(|known| describe_recording(&known)).unwrap_or_default()}}
            {footer(view! {
                {button(|| "Forget recording".to_owned(), Tone::Quiet, busy.busy.into(), move || forget(store, forget_node.clone(), busy))}
                {button(|| "Play".to_owned(), Tone::Primary, blocked, play)}
            })}
        }
    }
}

fn forget(store: Store, node: String, busy: Busy) {
    release(store, node, busy, |body| {
        if let NodeBody::Recording(recording) = body {
            recording.recording = None;
        }
    });
}

fn playing(radio: Radio, node: String, stem: String, shelf: Library, busy: Busy) -> impl IntoView {
    let store = radio.store;
    let known = {
        let stem = stem.clone();
        Signal::derive(move || {
            shelf
                .get()
                .and_then(Result::ok)
                .and_then(|all| find_recording(&all, Some(&stem)).cloned())
        })
    };
    let title = Signal::derive(move || {
        known
            .get()
            .map_or_else(|| stem.clone(), |known| recording_title(&known))
    });
    let failed = Signal::derive(move || {
        radio
            .read(|set| set.status == DeviceSetStatus::Error)
            .unwrap_or(false)
    });
    let has_transport =
        Memo::new(move |_| radio.read(|set| set.playback.is_some()).unwrap_or(false));
    let error = Signal::derive(move || radio.read(|set| set.error.clone()).flatten());
    let facts = move || {
        known.get().map(|known| {
            AnyView::new(readout(vec![
                (
                    "Centre".to_owned(),
                    AnyView::new(units::mhz(known.center_hz)),
                ),
                (
                    "Rate".to_owned(),
                    AnyView::new(units::sample_rate(known.sample_rate)),
                ),
                (
                    "Length".to_owned(),
                    AnyView::new(units::duration(known.duration_s)),
                ),
                (
                    "Size".to_owned(),
                    AnyView::new(units::bytes(known.bytes as f64)),
                ),
            ]))
        })
    };
    let closing = busy.busy;
    view! {
        column(class = "face") {
            row(class = "kit-head") {
                text(class = "kit-head__title") {{move || title.get()}}
                spacer() {}
                text(class = "kit-legend warn", hidden = move || !failed.get()) {"error"}
            }
            {move || has_transport.get().then(|| AnyView::new(playback::transport(radio)))}
            {facts}
            {move || error.get().map(|error| AnyView::new(view! { text(class = "kit-alert") {{error}} }))}
            {footer(button(move || if closing.get() { "Closing…".to_owned() } else { "Forget recording".to_owned() }, Tone::Quiet, closing.into(), move || forget(store, node.clone(), busy)))}
        }
    }
}
