use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    channel::ChannelParams,
    patch::{NodeBody, PatchEdge, PatchGraph, PatchNode, PortRef, Position},
    state::StateSnapshot,
};
use zgui::{prelude::*, reactive::RenderEffect};

use super::{
    ScopeCx,
    live::FrameMeta,
    pick::{ScopePick, format_mhz, stream_channels, take_creation_tune, tune_on_create},
    plot::readout_at,
    radio::{Lane, Radio, auto_tuning, trunk_roles, tune_delta, tuning_locked},
    traces::{DbWindow, TraceMode, clamp_window},
};
use crate::{binding, ui::palette};

const NEW_NODE_GAP: f32 = 40.0;

pub fn radio(cx: ScopeCx) -> Radio {
    let lane = cx.lane.get_value();
    radio_of(&lane, &cx.store.graph.get(), &cx.store.state.get())
}

pub fn untracked_radio(cx: ScopeCx) -> Radio {
    let lane = cx.lane.get_value();
    radio_of(
        &lane,
        &cx.store.graph.get_untracked(),
        &cx.store.state.get_untracked(),
    )
}

#[must_use]
pub fn radio_of(lane: &Lane, graph: &PatchGraph, state: &StateSnapshot) -> Radio {
    let Some(set) = state
        .device_sets
        .iter()
        .find(|set| set.id == lane.set)
        .cloned()
    else {
        return Radio::default();
    };
    let channels = stream_channels(&set.channels, lane.stream);
    let on_stream: HashSet<u32> = channels.iter().map(|channel| channel.id).collect();
    let devices = binding::device_sets(graph, &state.device_sets);
    let mut faces: HashMap<u32, String> = binding::channels(graph, &state.device_sets, &devices)
        .into_iter()
        .filter(|(node, info)| {
            on_stream.contains(&info.id)
                && binding::iq_source_of(graph, node)
                    .is_some_and(|(source, stream)| source == lane.device && stream == lane.stream)
        })
        .map(|(node, info)| (info.id, node))
        .collect();
    let owners = trunk_roles(&state.trunk_systems, lane.set);
    for (channel, owner) in &owners {
        if on_stream.contains(channel) {
            faces.insert(*channel, owner.node.clone());
        }
    }
    let locked = faces
        .iter()
        .filter(|(_, node)| tuning_locked(graph, node, 0))
        .map(|(channel, _)| *channel)
        .collect();
    Radio {
        centre_held: tuning_locked(graph, &lane.device, lane.stream),
        on_auto: auto_tuning(&set, lane.stream),
        set: Some(set),
        channels,
        faces,
        owners,
        locked,
    }
}

#[must_use]
pub fn chosen_channel(radio: &Radio, selected: Option<&str>, picked: Option<u32>) -> Option<u32> {
    radio.face_channel(selected).or_else(|| {
        picked.filter(|picked| radio.channels.iter().any(|channel| channel.id == *picked))
    })
}

pub fn selected_channel(cx: ScopeCx, radio: &Radio) -> Option<u32> {
    chosen_channel(radio, cx.store.selected.get().as_deref(), cx.picked.get())
}

pub fn tunable_now(cx: ScopeCx, radio: &Radio) -> Option<u32> {
    chosen_channel(
        radio,
        cx.store.selected.get_untracked().as_deref(),
        cx.picked.get_untracked(),
    )
    .filter(|channel| !radio.held(*channel))
}

pub fn select_channel(cx: ScopeCx, channel: u32) {
    cx.picked.set(Some(channel));
    if let Some(node) = untracked_radio(cx).faces.get(&channel).cloned() {
        cx.store.selected.set(Some(node));
    }
}

pub fn tune_centre(cx: ScopeCx, radio: &Radio, hz: f64) {
    let Some(set) = radio.set.as_ref() else {
        return;
    };
    if radio.centre_held {
        return;
    }
    let stream = cx.lane.with_value(|lane| lane.stream);
    cx.store
        .set_device(set.id, tune_delta(&set.capabilities, stream, hz));
}

pub fn tune_channel(cx: ScopeCx, radio: &Radio, channel: u32, hz: f64) {
    if radio.held(channel) {
        return;
    }
    let Some(info) = radio.channel(channel) else {
        return;
    };
    let mut settings = info.settings.clone();
    settings.frequency_hz = hz.round();
    match radio.faces.get(&channel) {
        Some(node) => cx.store.set_channel(node.clone(), settings),
        None => {
            let set = cx.lane.with_value(|lane| lane.set);
            let store = cx.store;
            zgui::task::spawn_local(async move {
                let path = format!("/api/devicesets/{set}/channels/{channel}");
                if let Err(error) = store
                    .api()
                    .patch::<_, serde_json::Value>(&path, &settings)
                    .await
                {
                    store.say(format!("cannot tune the channel: {error}"));
                }
                store.refresh_state();
            });
        }
    }
}

pub fn tune_to(cx: ScopeCx, pick: ScopePick) {
    let radio = untracked_radio(cx);
    match tunable_now(cx, &radio) {
        Some(channel) => tune_channel(cx, &radio, channel, pick.hz),
        None => tune_centre(cx, &radio, pick.hz),
    }
}

pub fn tune_to_band(cx: ScopeCx, hz: f64, suggested: Option<ChannelParams>) {
    let radio = untracked_radio(cx);
    let Some(channel) = tunable_now(cx, &radio) else {
        tune_centre(cx, &radio, hz);
        return;
    };
    let stream = cx.lane.with_value(|lane| lane.stream);
    let held = radio
        .set
        .as_ref()
        .is_none_or(|set| !auto_tuning(set, stream));
    let meta = cx.meta.get_untracked();
    if held && meta.is_none_or(|meta| (hz - meta.centre_hz).abs() >= meta.span_hz / 2.0) {
        tune_centre(cx, &radio, hz);
    }
    let Some(info) = radio.channel(channel) else {
        return;
    };
    let mut settings = info.settings.clone();
    settings.frequency_hz = hz.round();
    if let Some(params) = suggested.clone() {
        settings.params = params;
    }
    let Some(face) = radio.faces.get(&channel).cloned() else {
        return;
    };
    cx.store.set_channel(face.clone(), settings);
    if let Some(params) = suggested {
        let type_id = params.type_id().to_owned();
        cx.store.edit_graph(move |graph| {
            if let Some(NodeBody::Channel(channel)) = graph
                .nodes
                .iter_mut()
                .find(|node| node.id == face)
                .map(|node| &mut node.body)
            {
                channel.channel_type = type_id;
            }
        });
    }
}

pub fn add_channel_at(cx: ScopeCx, pick: ScopePick, channel_type: String) {
    let lane = cx.lane.get_value();
    let scope = cx.node.get_value();
    let graph = cx.store.graph.get_untracked();
    let taken: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    let id = palette::free_id(&taken, &format!("channel:{channel_type}"));
    let Some(body) = palette::body_for(&format!("channel:{channel_type}")) else {
        cx.store.say(format!("Unknown decoder: {channel_type}"));
        return;
    };
    let position = graph
        .node(&scope)
        .map_or(Position { x: 0.0, y: 0.0 }, |node| Position {
            x: node.position.x + crate::ui::node::width_of(node) + NEW_NODE_GAP,
            y: node.position.y,
        });
    tune_on_create(&id, pick.hz);
    let port = if lane.stream == 0 {
        String::from("iq")
    } else {
        format!("iq{}", lane.stream + 1)
    };
    let created = id.clone();
    cx.store.edit_graph(move |graph| {
        graph.nodes.push(PatchNode {
            id: created.clone(),
            body,
            position,
            size: None,
            label: None,
        });
        graph.edges.push(PatchEdge {
            from: PortRef {
                node: lane.device,
                port,
            },
            to: PortRef {
                node: created,
                port: String::from("iq"),
            },
        });
    });
    cx.store.selected.set(Some(id));
}

pub fn apply_creation_tunes(cx: ScopeCx) {
    let effect = RenderEffect::new(move |_| {
        let radio = radio(cx);
        for (channel, face) in &radio.faces {
            if let Some(hz) = take_creation_tune(face) {
                tune_channel(cx, &radio, *channel, hz);
            }
        }
    });
    on_cleanup_local(move || drop(effect));
}

pub fn apply_range(cx: ScopeCx, next: Option<DbWindow>) {
    cx.range.set(next);
    let lane = cx.lane.with_value(|lane| (lane.set, lane.stream));
    let feed = cx.feed.get_value();
    super::live::reseed(&mut feed.borrow_mut(), lane, cx.meta.get_untracked(), next);
    cx.live.get_value().borrow_mut().clear_density();
}

pub fn hold_range(cx: ScopeCx) {
    let shown = cx
        .meta
        .get_untracked()
        .map_or(super::EMPTY_WINDOW, FrameMeta::window);
    apply_range(cx, Some(clamp_window(shown)));
}

pub fn toggle_phosphor(cx: ScopeCx) {
    let on = cx.phosphor.get_untracked();
    if !on && cx.range.get_untracked().is_none() {
        hold_range(cx);
    }
    cx.phosphor.set(!on);
}

pub fn toggle_trace(cx: ScopeCx, mode: TraceMode) {
    cx.modes.update(|modes| {
        if let Some(at) = modes.iter().position(|held| *held == mode) {
            modes.remove(at);
        } else {
            modes.push(mode);
        }
    });
}

pub fn update_readout(cx: ScopeCx) {
    let Some(at) = cx.hover.get_untracked() else {
        if cx.readout.with_untracked(Option::is_some) {
            cx.readout.set(None);
        }
        return;
    };
    let live = cx.live.get_value();
    let mut live = live.borrow_mut();
    let Some(meta) = live.frame else {
        return;
    };
    let now = live.now_ms();
    let shown = live.tween.sample(now).to_vec();
    let view = cx.view.get_untracked();
    let text = readout_at(meta.centre_hz, meta.span_hz, &shown, view, at).map(|read| {
        let db = live.readout.read(read.bin, read.db, now);
        if db.is_finite() {
            format!("{}  {db:.1} dBFS", format_mhz(read.hz))
        } else {
            format_mhz(read.hz)
        }
    });
    if cx.readout.with_untracked(|held| *held != text) {
        cx.readout.set(text);
    }
}
