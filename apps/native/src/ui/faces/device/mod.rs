pub mod actions;
pub mod caps;
pub mod devices;
pub mod gain;
pub mod lanes;
pub mod open;
pub mod patch;
pub mod radio;
pub mod tuner;

use sdrmm_wire::{
    device::DeviceInfo,
    patch::{DeviceRef, NodeBody},
    state::DeviceSet,
};
use zgui::prelude::*;

use self::{
    actions::{Busy, create_set, edit_body, opened_device, release},
    lanes::{
        Tone as HeardTone, clipping_said, coherent_lanes, fault_said, hearing, lanes_merged,
        ref_label, refusal_said,
    },
    open::{Choices, device_choices},
    radio::{Radio, radio_settings},
    tuner::{Tuned, tuner},
};
use super::set_signal;
use crate::{
    store::Store,
    ui::kit_sources::{Tone, button, collapsible, footer, install, readout},
};

#[derive(Clone, PartialEq)]
enum Phase {
    Unnamed,
    Named(DeviceRef),
    Open,
}

fn reference_of(store: Store, node: &str) -> Option<DeviceRef> {
    store.graph.with(|graph| {
        graph.node(node).and_then(|found| match &found.body {
            NodeBody::Device(device) => device.device.clone(),
            _ => None,
        })
    })
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install();
    let set = set_signal(store, node.clone());
    let phase = {
        let node = node.clone();
        Memo::new(
            move |_| match (reference_of(store, &node), set.with(Option::is_some)) {
                (None, _) => Phase::Unnamed,
                (Some(_), true) => Phase::Open,
                (Some(reference), false) => Phase::Named(reference),
            },
        )
    };
    let busy = Busy::new();
    move || match phase.get() {
        Phase::Unnamed => AnyView::new(choose(store, node.clone(), busy)),
        Phase::Named(reference) => AnyView::new(named(store, node.clone(), reference, busy)),
        Phase::Open => AnyView::new(opened(store, node.clone(), set, busy)),
    }
}

fn choose(store: Store, node: String, busy: Busy) -> impl IntoView {
    let bind = {
        let node = node.clone();
        move |info: DeviceInfo| {
            let chosen = DeviceRef::from_info(&info);
            let already = store.state.with_untracked(|state| {
                state
                    .device_sets
                    .iter()
                    .any(|set| chosen.matches(&set.device))
            });
            let node = node.clone();
            busy.run(store, async move {
                if !already {
                    create_set(store, devices::device_id(&info)).await?;
                }
                name_radio(store, node, Some(chosen));
                Ok(())
            });
        }
    };
    let network = {
        let node = node.clone();
        move |id: String| {
            let node = node.clone();
            busy.run(store, async move {
                let set = create_set(store, id).await?;
                match opened_device(store, set).await? {
                    Some(info) => name_radio(store, node, Some(DeviceRef::from_info(&info))),
                    None => store.apply().await,
                }
                Ok(())
            });
        }
    };
    let choices = Choices {
        on_choose: Box::new(bind),
        on_network: Box::new(network),
    };
    view! {
        column(class = "face") {
            {device_choices(store, node, busy, choices)}
        }
    }
}

fn name_radio(store: Store, node: String, chosen: Option<DeviceRef>) {
    edit_body(store, node, move |body| {
        if let NodeBody::Device(device) = body {
            device.device = chosen;
        }
    });
}

fn forget(store: Store, node: String, busy: Busy) {
    release(store, node, busy, |body| {
        if let NodeBody::Device(device) = body {
            device.device = None;
        }
    });
}

fn named(store: Store, node: String, reference: DeviceRef, busy: Busy) -> impl IntoView {
    let probe = reference.clone();
    let on_bus = Signal::derive(move || {
        store
            .devices
            .with(|devices| devices.iter().any(|info| probe.matches(info)))
    });
    let label = ref_label(&reference);
    view! {
        column(class = "face") {
            row(class = "kit-head") {
                spacer()
                text(class = "kit-legend") {{move || if on_bus.get() { "not open" } else { "disconnected" }}}
            }
            text(class = "kit-mono") {{label}}
            {footer(view! {
                {button(|| "Forget radio".to_owned(), Tone::Quiet, busy.busy.into(), move || forget(store, node.clone(), busy))}
                {button(|| "Open radio".to_owned(), Tone::Primary, Signal::derive(move || !on_bus.get()), move || {
                    zgui::task::spawn_local(async move { store.apply().await });
                })}
            })}
        }
    }
}

fn opened(store: Store, node: String, set: Signal<Option<DeviceSet>>, busy: Busy) -> impl IntoView {
    let radio = Radio { store, set };
    let advised = {
        let node = node.clone();
        Signal::derive(move || store.graph.with(|graph| coherent_lanes(graph, &node)))
    };
    let lanes_shown = radio.read_untracked(lanes_merged).unwrap_or(false);
    let tuned = Tuned {
        store,
        node: node.clone(),
        radio,
    };
    let closing = busy.busy;
    view! {
        column(class = "face") {
            {heard_head(radio)}
            {tuner(tuned, advised)}
            {radio_settings(radio, lanes_shown, advised)}
            {health(radio)}
            {fault(radio)}
            {footer(button(move || if closing.get() { "Closing…".to_owned() } else { "Forget radio".to_owned() }, Tone::Quiet, closing.into(), move || forget(store, node.clone(), busy)))}
        }
    }
}

fn heard_head(radio: Radio) -> impl IntoView {
    let label = Signal::derive(move || {
        radio
            .read(|set| set.device.label.clone())
            .unwrap_or_default()
    });
    let heard = Signal::derive(move || radio.read(hearing));
    let tone = move || match heard.get().map(|heard| heard.tone) {
        Some(HeardTone::Ok) => "ok",
        Some(HeardTone::Warn) => "warn",
        _ => "danger",
    };
    view! {
        row(class = "kit-head") {
            text(class = "kit-head__title") {{move || label.get()}}
            spacer()
            text(class = "kit-heard", attr:data-tone = move || Some(tone().to_owned())) {
                {move || heard.get().map(|heard| format!("{}/{}", heard.heard, heard.total)).unwrap_or_default()}
            }
        }
    }
}

fn health(radio: Radio) -> impl IntoView {
    move || {
        if !cfg!(debug_assertions) {
            return None;
        }
        let (clipping, overruns) = radio.read(|set| (clipping_said(set), set.overruns))?;
        if clipping.is_none() && overruns == 0 {
            return None;
        }
        let mut rows = Vec::new();
        if let Some(clipping) = clipping {
            rows.push(("Clipping".to_owned(), AnyView::new(clipping)));
        }
        if overruns > 0 {
            rows.push(("Drops".to_owned(), AnyView::new(overruns.to_string())));
        }
        Some(AnyView::new(readout(rows)))
    }
}

fn fault(radio: Radio) -> impl IntoView {
    let said = Memo::new(move |_| {
        radio.read(|set| {
            let error = set.error.clone();
            let fault = error.as_ref().map(|error| (fault_said(set), error.clone()));
            let refused = refusal_said(set).map(|said| {
                (
                    said,
                    set.refused
                        .as_ref()
                        .map(|r| r.error.clone())
                        .unwrap_or_default(),
                )
            });
            (fault, refused)
        })
    });
    move || {
        let (fault, refused) = said.get()?;
        let alert = |(summary, detail): (String, String)| {
            AnyView::new(view! {
                box(class = "kit-alert-block") {
                    {collapsible(move || summary.clone(), "kit-fold__alert", move || AnyView::new(view! { text(class = "kit-meta") {{detail.clone()}} }))}
                }
            })
        };
        let fault = fault.map(|(said, error)| match said {
            Some(said) => alert((said, error)),
            None => AnyView::new(view! { text(class = "kit-alert kit-alert-block") {{format!("Device fault · {error}")}} }),
        });
        Some(AnyView::new(
            view! { column { {fault} {refused.map(alert)} } },
        ))
    }
}
