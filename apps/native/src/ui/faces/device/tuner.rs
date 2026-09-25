use sdrmm_wire::{device::Tuning, patch::NodeBody};
use zgui::prelude::*;

use super::{
    actions::edit_body,
    gain::lane_controls,
    lanes::{
        auto_tuning, bond_said, has_lane_controls, lanes_merged, lock_stream, tune_delta,
        tuner_dials, tuning_delta,
    },
    radio::Radio,
};
use crate::{
    store::Store,
    ui::kit_sources::{
        DialSpec,
        dial::{ANY_FREQUENCY, Reach, in_tuning_range, is_tunable, tuning_range},
        frequency_dial, icon_button, icons, tune_to, units,
    },
};

#[derive(Clone)]
pub struct Tuned {
    pub store: Store,
    pub node: String,
    pub radio: Radio,
}

impl Tuned {
    fn locked(&self, stream: u32) -> Signal<bool> {
        let (store, node) = (self.store, self.node.clone());
        Signal::derive(move || {
            store.graph.with(|graph| {
                graph.node(&node).is_some_and(|found| match &found.body {
                    NodeBody::Device(device) => device.tuning_locked(stream),
                    _ => false,
                })
            })
        })
    }

    fn lock(&self, stream: u32, held: bool) {
        edit_body(self.store, self.node.clone(), move |body| {
            if let NodeBody::Device(device) = body {
                device.locked_streams = lock_stream(&device.locked_streams, stream, held);
            }
        });
    }

    fn array_tuning(&self) -> Signal<bool> {
        let (store, node) = (self.store, self.node.clone());
        Signal::derive(move || {
            let array = store
                .graph
                .with(|graph| graph.array_holding(&node).map(str::to_owned));
            array.is_some_and(|array| store.device_set_of(&array).is_some())
        })
    }
}

pub fn reach_of(radio: Radio) -> Signal<Reach> {
    Signal::derive(move || {
        radio
            .read(|set| tuning_range(&set.capabilities))
            .unwrap_or(ANY_FREQUENCY)
    })
}

pub fn tuner(tuned: Tuned, advised: Signal<std::collections::BTreeSet<u32>>) -> impl IntoView {
    let radio = tuned.radio;
    let shape = Memo::new(move |_| {
        radio.read(|set| {
            let merged = lanes_merged(set);
            let streams: Vec<(u32, Option<String>)> = tuner_dials(set)
                .into_iter()
                .take(if merged { usize::MAX } else { 1 })
                .map(|dial| (dial.stream, dial.port))
                .collect();
            (
                merged,
                has_lane_controls(&set.capabilities),
                bond_said(set.capabilities.coherence),
                streams,
            )
        })
    });
    move || {
        let (merged, controls, bond, streams) = shape.get()?;
        let lanes: Vec<AnyView> = streams
            .into_iter()
            .enumerate()
            .map(|(index, (stream, port))| {
                let rule = merged.then(|| {
                    AnyView::new(lane_rule(
                        &tuned,
                        stream,
                        port.clone(),
                        if index == 0 { bond } else { None },
                        index == 0,
                    ))
                });
                let extra = (merged && controls).then(|| {
                    let lane_advised = advised.with_untracked(|lanes| lanes.contains(&stream));
                    AnyView::new(lane_controls(radio, stream, lane_advised))
                });
                let row = dial_row(&tuned, stream, port);
                let locked = tuned.locked(stream);
                AnyView::new(view! {
                    column(class = "kit-lane-block", class:locked = locked) {
                        {rule}
                        {row}
                        {extra}
                    }
                })
            })
            .collect();
        Some(AnyView::new(
            view! { column(class = "kit-tuner") {{lanes}} },
        ))
    }
}

fn lane_rule(
    tuned: &Tuned,
    stream: u32,
    port: Option<String>,
    bond: Option<&'static str>,
    first: bool,
) -> impl IntoView {
    let locked = tuned.locked(stream);
    let array = tuned.array_tuning();
    view! {
        row(class = "kit-rule") {
            box(class = "kit-rule__tick")
            text(class = "kit-rule__port") {{port.unwrap_or_default()}}
            {move || locked.get().then(|| AnyView::new(icons::icon(icons::LOCK)))}
            box(class = "kit-rule__line") {}
            {bond.map(|bond| AnyView::new(view! { row(class = "kit-rule__badge") { {icons::icon(icons::LINK)} text {{bond}} } }))}
            {move || (first && array.get()).then(|| AnyView::new(view! { row(class = "kit-rule__badge") { {icons::icon(icons::LINK)} text {"Array"} } }))}
        }
    }
}

fn dial_row(tuned: &Tuned, stream: u32, port: Option<String>) -> impl IntoView {
    let radio = tuned.radio;
    let store = tuned.store;
    let node = tuned.node.clone();
    let reach = reach_of(radio);
    let pinned = Signal::derive(move || !is_tunable(reach.get()));
    let locked = tuned.locked(stream);
    let held = Signal::derive(move || pinned.get() || locked.get());
    let hz = Signal::derive(move || {
        radio
            .read(|set| {
                tuner_dials(set)
                    .into_iter()
                    .find(|dial| dial.stream == stream)
                    .map_or(0.0, |dial| dial.hz)
            })
            .unwrap_or(0.0)
    });
    let active = Signal::derive(move || store.selected.get().as_deref() == Some(node.as_str()));
    let tune = move |value: f64| {
        if let Some(delta) =
            radio.read_untracked(|set| tune_delta(&set.capabilities, stream, value))
        {
            radio.patch(delta);
        }
    };
    let spec = DialSpec {
        hz,
        reach,
        disabled: held,
        wheel: active,
    };
    let hint = Signal::derive(move || {
        let reach = reach.get();
        format!(
            "Reaches {} to {}",
            units::mhz(reach.min),
            units::mhz(reach.max)
        )
    });
    let title = if port.is_some() {
        "Type a frequency for this lane"
    } else {
        "Type a frequency"
    };
    let auto = Signal::derive(move || radio.read(|set| auto_tuning(set, stream)).unwrap_or(true));
    let tuned_lock = tuned.clone();
    let tools = move || {
        (!pinned.get()).then(|| {
            let lock = tuned_lock.clone();
            AnyView::new(view! {
                row(class = "kit-tools") {
                    {tune_to(title, hz, hint, move |entered| in_tuning_range(entered, reach.get_untracked()), held, tune)}
                    {icon_button(icons::RADAR, "Follow the decoders", auto, Signal::stored(false), move || {
                        let next = if auto.get_untracked() { Tuning::Manual } else { Tuning::Auto };
                        if let Some(delta) = radio.read_untracked(|set| tuning_delta(&set.capabilities, stream, next)) {
                            radio.patch(delta);
                        }
                    })}
                    {icon_button(if locked.get() { icons::LOCK } else { icons::LOCK_OPEN }, "Lock tuning", locked, Signal::stored(false), move || lock.lock(stream, !locked.get_untracked()))}
                }
            })
        })
    };
    view! {
        row(class = "kit-dial-row") {
            {frequency_dial(spec, tune)}
            spacer() {}
            {tools}
        }
    }
}
