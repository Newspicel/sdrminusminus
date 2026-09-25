use sdrmm_wire::{
    patch::NodeBody,
    state::{DeviceSet, DeviceSetStatus},
    timemachine::{
        MAX_TIME_MACHINE_SECONDS, MIN_TIME_MACHINE_SECONDS, TimeMachineAction, TimeMachineNode,
        TimeMachineRequest, TimeMachineStatus,
    },
};
use zgui::prelude::*;

use super::{device::actions::edit_body, set_signal};
use crate::{
    binding,
    store::Store,
    ui::{
        kit_sources::{NumberSpec, Tone, button, footer, install, number_field, readout, units},
        widgets::row_field,
    },
};

pub const HISTORY_BYTES_PER_SAMPLE: f64 = 8.0;
const UNWIRED: &str = "Wire a running device's IQ into this sink first.";

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Unavailable,
    Idle,
    Armed(TimeMachineStatus),
    Capturing(TimeMachineStatus),
    Busy(String),
}

#[must_use]
pub fn phase_of(set: Option<&DeviceSet>, node: &str) -> Phase {
    let Some(set) = set.filter(|set| set.status == DeviceSetStatus::Running) else {
        return Phase::Unavailable;
    };
    match &set.time_machine {
        None => Phase::Idle,
        Some(held) if held.node != node => Phase::Busy(held.node.clone()),
        Some(held) if held.capture.is_none() => Phase::Armed(held.clone()),
        Some(held) => Phase::Capturing(held.clone()),
    }
}

#[must_use]
pub fn history_fill(status: &TimeMachineStatus) -> f64 {
    if status.capacity_samples == 0 {
        0.0
    } else {
        (status.held_samples as f64 / status.capacity_samples as f64).min(1.0)
    }
}

pub fn request_for(
    set: Option<u32>,
    action: TimeMachineAction,
    node: &str,
    stream: u32,
    settings: TimeMachineNode,
) -> Result<(String, TimeMachineRequest), &'static str> {
    let set = set.ok_or(UNWIRED)?;
    Ok((
        format!("/api/devicesets/{set}/time-machine"),
        TimeMachineRequest {
            action,
            node: node.to_owned(),
            stream,
            settings,
        },
    ))
}

fn settings_of(store: Store, node: &str) -> TimeMachineNode {
    store.graph.with(|graph| {
        graph
            .node(node)
            .and_then(|found| match &found.body {
                NodeBody::TimeMachine(settings) => Some(*settings),
                _ => None,
            })
            .unwrap_or_default()
    })
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install();
    let set = set_signal(store, node.clone());
    let phase = {
        let node = node.clone();
        Memo::new(move |_| set.with(|set| phase_of(set.as_ref(), &node)))
    };
    let pending = RwSignal::new(false);
    let seconds = {
        let node = node.clone();
        Signal::derive(move || f64::from(settings_of(store, &node).history_seconds))
    };
    let control = {
        let node = node.clone();
        move |action: TimeMachineAction| {
            let stream = store
                .graph
                .with(|graph| binding::iq_source_of(graph, &node))
                .map_or(0, |(_, stream)| stream);
            let request = request_for(
                set.with_untracked(|set| set.as_ref().map(|set| set.id)),
                action,
                &node,
                stream,
                settings_of(store, &node),
            );
            let (path, body) = match request {
                Ok(request) => request,
                Err(said) => return store.say(said),
            };
            pending.set(true);
            zgui::task::spawn_local(async move {
                let sent: anyhow::Result<TimeMachineStatus> = store.api().post(&path, &body).await;
                pending.try_set(false);
                if let Err(error) = sent {
                    store.say(error.to_string());
                }
                store.refresh_state();
            });
        }
    };
    let edit = {
        let node = node.clone();
        move |value: f64| {
            let history_seconds = value.round() as u32;
            edit_body(store, node.clone(), move |body| {
                if let NodeBody::TimeMachine(settings) = body {
                    settings.history_seconds = history_seconds;
                }
            });
        }
    };
    let locked = Signal::derive(move || phase.get() != Phase::Idle || pending.get());
    let spec = NumberSpec::unit("s")
        .within(
            f64::from(MIN_TIME_MACHINE_SECONDS),
            f64::from(MAX_TIME_MACHINE_SECONDS),
        )
        .step(1.0);
    view! {
        column(class = "face") {
            {row_field("History", number_field("Seconds of history", seconds, spec, locked, edit))}
            {move || body(phase.get(), seconds.get())}
            {move || actions(phase.get(), pending, control.clone())}
        }
    }
}

fn body(phase: Phase, seconds: f64) -> AnyView {
    match phase {
        Phase::Armed(status) | Phase::Capturing(status) => AnyView::new(held(status)),
        Phase::Unavailable => {
            AnyView::new(view! { text(class = "kit-note") {"Wire a device's IQ in"} })
        }
        Phase::Busy(_) => AnyView::new(
            view! { text(class = "kit-note") {"Another time machine holds this radio"} },
        ),
        Phase::Idle => AnyView::new(
            view! { text(class = "kit-note") {{format!("Arm it to keep the last {seconds} s in memory")}} },
        ),
    }
}

fn held(status: TimeMachineStatus) -> impl IntoView {
    let mut rows = vec![
        (
            "Held".to_owned(),
            AnyView::new(format!(
                "{} · {:.0}% of {} s",
                units::duration(status.held_seconds()),
                history_fill(&status) * 100.0,
                status.history_seconds
            )),
        ),
        (
            "Memory".to_owned(),
            AnyView::new(units::bytes(
                status.capacity_samples as f64 * HISTORY_BYTES_PER_SAMPLE,
            )),
        ),
    ];
    if cfg!(debug_assertions) && status.overruns > 0 {
        rows.push((
            "Drops".to_owned(),
            AnyView::new(status.overruns.to_string()),
        ));
    }
    if let Some(capture) = &status.capture {
        rows.push((
            "Written".to_owned(),
            AnyView::new(units::bytes(capture.bytes as f64)),
        ));
        rows.push(("File".to_owned(), AnyView::new(capture.file.clone())));
    }
    let error = status
        .error
        .map(|error| AnyView::new(view! { text(class = "kit-alert") {{error}} }));
    view! { column { {readout(rows)} {error} } }
}

fn actions(
    phase: Phase,
    pending: RwSignal<bool>,
    control: impl Fn(TimeMachineAction) + Clone + 'static,
) -> impl IntoView {
    let busy: Signal<bool> = pending.into();
    let unavailable = phase == Phase::Unavailable;
    let press = move |action: TimeMachineAction| {
        let control = control.clone();
        move || control(action)
    };
    let buttons = match phase {
        Phase::Idle | Phase::Unavailable => {
            let blocked = Signal::derive(move || unavailable || pending.get());
            vec![AnyView::new(button(
                || "Arm".to_owned(),
                Tone::Plain,
                blocked,
                press(TimeMachineAction::Arm),
            ))]
        }
        Phase::Armed(_) => vec![
            AnyView::new(button(
                || "Capture".to_owned(),
                Tone::Plain,
                busy,
                press(TimeMachineAction::Capture),
            )),
            AnyView::new(button(
                || "Disarm".to_owned(),
                Tone::Plain,
                busy,
                press(TimeMachineAction::Disarm),
            )),
        ],
        Phase::Capturing(_) | Phase::Busy(_) => vec![
            AnyView::new(button(
                || "Stop".to_owned(),
                Tone::Danger,
                busy,
                press(TimeMachineAction::Stop),
            )),
            AnyView::new(button(
                || "Disarm".to_owned(),
                Tone::Plain,
                busy,
                press(TimeMachineAction::Disarm),
            )),
        ],
    };
    footer(buttons)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::timemachine::DEFAULT_TIME_MACHINE_SECONDS;
    use serde_json::json;

    use super::*;

    fn held() -> TimeMachineStatus {
        serde_json::from_value(json!({
            "node": "history", "stream": 0, "history_seconds": 10, "sample_rate": 2_048_000,
            "center_hz": 100_000_000, "held_samples": 10_240_000, "capacity_samples": 20_480_000, "overruns": 0
        }))
        .expect("status")
    }

    fn capturing() -> TimeMachineStatus {
        serde_json::from_value(json!({
            "node": "history", "stream": 0, "history_seconds": 10, "sample_rate": 2_048_000,
            "center_hz": 100_000_000, "held_samples": 10_240_000, "capacity_samples": 20_480_000, "overruns": 0,
            "capture": { "file": "tm_1", "stream": 0, "started_at": "2026-08-16T00:00:00Z", "samples": 10_240_000, "bytes": 81_920_000, "overruns": 0 }
        }))
        .expect("status")
    }

    fn set(status: &str, machine: Option<TimeMachineStatus>) -> DeviceSet {
        serde_json::from_value(json!({
            "id": 4, "status": status, "time_machine": machine,
            "device": { "driver": "virtual", "key": "siggen", "label": "Signal Generator" },
            "capabilities": { "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": [] },
            "settings": {}, "channels": []
        }))
        .expect("device set")
    }

    #[test]
    fn an_idle_radio_an_armed_buffer_and_a_running_capture_are_told_apart() {
        assert_eq!(
            phase_of(Some(&set("running", None)), "history"),
            Phase::Idle
        );
        assert_eq!(
            phase_of(Some(&set("running", Some(held()))), "history"),
            Phase::Armed(held())
        );
        assert_eq!(
            phase_of(Some(&set("running", Some(capturing()))), "history"),
            Phase::Capturing(capturing())
        );
    }

    #[test]
    fn the_node_already_holding_the_history_is_named() {
        assert_eq!(
            phase_of(Some(&set("running", Some(held()))), "other"),
            Phase::Busy("history".to_owned())
        );
    }

    #[test]
    fn nothing_is_offered_while_the_radio_is_not_running() {
        assert_eq!(
            phase_of(Some(&set("error", Some(held()))), "history"),
            Phase::Unavailable
        );
        assert_eq!(phase_of(None, "history"), Phase::Unavailable);
    }

    #[test]
    fn the_held_window_reads_as_seconds_and_as_a_fraction() {
        let status = held();
        assert_eq!(status.held_seconds(), 5.0);
        assert_eq!(history_fill(&status), 0.5);
        assert_eq!(
            history_fill(&TimeMachineStatus {
                held_samples: 40_960_000,
                ..held()
            }),
            1.0
        );
        assert_eq!(
            history_fill(&TimeMachineStatus {
                capacity_samples: 0,
                ..held()
            }),
            0.0
        );
        assert_eq!(
            TimeMachineStatus {
                sample_rate: 0,
                ..held()
            }
            .held_seconds(),
            0.0
        );
    }

    #[test]
    fn each_action_goes_to_the_radio_holding_the_buffer() {
        let settings = TimeMachineNode {
            history_seconds: DEFAULT_TIME_MACHINE_SECONDS,
        };
        for action in [
            TimeMachineAction::Arm,
            TimeMachineAction::Capture,
            TimeMachineAction::Stop,
            TimeMachineAction::Disarm,
        ] {
            let (path, body) =
                request_for(Some(4), action, "history", 1, settings).expect("a request");
            assert_eq!(path, "/api/devicesets/4/time-machine");
            assert_eq!(
                body,
                TimeMachineRequest {
                    action,
                    node: "history".to_owned(),
                    stream: 1,
                    settings
                }
            );
        }
    }

    #[test]
    fn an_action_without_a_radio_wired_in_is_refused() {
        let refused = request_for(
            None,
            TimeMachineAction::Arm,
            "history",
            0,
            TimeMachineNode::default(),
        );
        assert!(refused.is_err_and(|said| said.starts_with("Wire a running device's IQ")));
    }
}
