use std::sync::Arc;

use sdrmm_wire::patch::NodeBody;
use zgui::prelude::*;

use crate::{
    binding,
    shell::{
        hotkeys::{self, Action, Chord},
        library_target::tuning_locked,
        rack_grid, tuner,
    },
    store::{Pane, Phase, Store},
    ui::shell::{Shell, leave_auto},
};

fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some(String::from("ArrowLeft")),
        Key::Named(NamedKey::ArrowRight) => Some(String::from("ArrowRight")),
        Key::Character(text) => Some(text.to_string()),
        _ => None,
    }
}

pub fn escape(store: Store, shell: Shell) -> bool {
    if shell.asking_auto_off.get_untracked() {
        shell.cancel_auto_off();
    } else if shell.dialog.get_untracked().is_some() {
        shell.close();
    } else if shell.menu.get_untracked().is_some() {
        shell.menu.set(None);
    } else if store.palette.get_untracked() {
        store.palette.set(false);
    } else if store.expanded.get_untracked().is_some() {
        store.expanded.set(None);
    } else if store.selected.get_untracked().is_some() {
        store.selected.set(None);
    } else {
        return false;
    }
    true
}

pub fn press(store: Store, shell: Shell, ev: &mut EventCx<'_, events::KeyDown>) {
    if matches!(ev.key, Key::Named(NamedKey::Escape)) {
        if escape(store, shell) {
            ev.prevent_default();
        }
        return;
    }
    let busy = shell.dialog.get_untracked().is_some() || shell.asking_auto_off.get_untracked();
    if busy || store.phase.get_untracked() != Phase::Ready {
        return;
    }
    let Some(name) = key_name(&ev.key) else {
        return;
    };
    let chord = Chord {
        ctrl: ev.modifiers.control(),
        meta: ev.modifiers.meta(),
        alt: ev.modifiers.alt(),
        shift: ev.modifiers.shift(),
    };
    if let Some(action) = hotkeys::action(&name, chord) {
        run(store, shell, action);
        ev.prevent_default();
    }
}

fn run(store: Store, shell: Shell, action: Action) {
    match action {
        Action::Tune(steps) => tune(store, shell, steps),
        Action::StepBy(direction) => shell
            .step_hz
            .update(|step| *step = hotkeys::stepped(*step, direction)),
        Action::CycleMode(direction) => cycle_mode(store, direction),
        Action::Squelch(delta) => {
            edit_squelch(store, |squelch| hotkeys::nudged_squelch(squelch, delta));
        }
        Action::ToggleSquelch => edit_squelch(store, hotkeys::toggled_squelch),
        Action::SelectChannel(direction) => select_channel(store, direction),
        Action::SelectNode(index) => {
            let id = store
                .graph
                .get_untracked()
                .nodes
                .get(index)
                .map(|node| node.id.clone());
            store.selected.set(id);
        }
        Action::TogglePin => {
            if let Some(id) = store.selected.get_untracked() {
                store.edit_rack(|rack| rack_grid::toggle_pin(rack, &id));
            }
        }
        Action::ToggleView => store.pane.update(|pane| {
            *pane = if *pane == Pane::Patch {
                Pane::Rack
            } else {
                Pane::Patch
            };
        }),
        Action::ToggleFull => {
            let selected = store.selected.get_untracked();
            store
                .expanded
                .update(|expanded| *expanded = if expanded.is_some() { None } else { selected });
        }
        Action::Undo => store.step_history(true),
        Action::Redo => store.step_history(false),
        Action::ShowShortcuts => shell.open(crate::ui::shell::Dialog::Shortcuts),
    }
}

fn tune(store: Store, shell: Shell, steps: i32) {
    let Some(selected) = store.selected.get_untracked() else {
        return;
    };
    let graph = store.graph.get_untracked();
    let Some(device) = binding::device_node_of(&graph, &selected) else {
        return;
    };
    if tuning_locked(&graph, &device) {
        return;
    }
    let Some(set) = store.device_set_of(&device).and_then(|id| store.set_of(id)) else {
        return;
    };
    let current = tuner::center_hz(&set, 0).unwrap_or_default();
    let wanted = current + f64::from(steps) * shell.step_hz.get_untracked();
    let delta = tuner::tune_delta(&set.capabilities, 0, wanted);
    let id = set.id;
    leave_auto(tuner::auto_tuning(&set, 0), move || {
        store.set_device(id, delta)
    });
}

fn cycle_mode(store: Store, direction: i32) {
    let Some(selected) = store.selected.get_untracked() else {
        return;
    };
    let graph = store.graph.get_untracked();
    let Some(NodeBody::Channel(channel)) = graph.node(&selected).map(|node| &node.body) else {
        return;
    };
    let wanted = hotkeys::next_analog_mode(&channel.channel_type, direction);
    if store.descriptor_of(wanted).is_none() {
        return;
    }
    store.edit_graph(move |graph| {
        if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == selected)
            && let NodeBody::Channel(channel) = &mut node.body
        {
            wanted.clone_into(&mut channel.channel_type);
        }
    });
}

fn edit_squelch(
    store: Store,
    change: impl FnOnce(&sdrmm_wire::channel::Squelch) -> sdrmm_wire::channel::Squelch,
) {
    let Some(selected) = store.selected.get_untracked() else {
        return;
    };
    let Some(channel) = store.channel_of(&selected) else {
        return;
    };
    let mut settings = channel.settings;
    settings.squelch = change(&settings.squelch);
    store.set_channel(selected, settings);
}

fn select_channel(store: Store, direction: i32) {
    let graph: Arc<_> = store.graph.get_untracked();
    let ids: Vec<String> = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.body, NodeBody::Channel(_)))
        .map(|node| node.id.clone())
        .collect();
    if let Some(next) = hotkeys::cycled(&ids, store.selected.get_untracked().as_deref(), direction)
    {
        store.selected.set(Some(next));
    }
}
