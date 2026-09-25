pub mod protocols;

use sdrmm_wire::{SpectrumMonitorNode, patch::NodeBody};
use zgui::prelude::*;

use self::protocols::{
    ProtocolChoice, ProtocolGroup, active_preset, choice_summary, enabled_kinds, protocol_groups,
    protocol_presets, set_enabled,
};
use crate::{
    store::Store,
    ui::{
        faces::channel::settings::NumberLimit,
        kit_channel::{
            self, CHEVRON_ICON, NumberSpec, Popover, icon, number_field, setting_row, toggle_row,
        },
        widgets::check,
    },
};

const SHEET: &str = css!(
    r#"
.sm-face { flex-direction: column; gap: 8px; padding: 8px; }
.sm-trigger {
    position: relative; flex: 1 1 auto; min-width: 0; max-width: 208px; height: 28px;
    align-items: center; gap: 6px; padding: 0 8px 2px 8px; overflow: hidden;
    border: 1px solid var(--line); border-radius: 3px; background-color: var(--panel-2);
    font-size: 12px; color: var(--ink); display: flex;
}
.sm-trigger:hover { border-color: var(--line-strong); }
.sm-trigger.on { border-color: var(--accent-dim); }
.sm-trigger__text { flex: 1 1 auto; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.sm-lights { position: absolute; left: 6px; right: 6px; bottom: 3px; gap: 4px; }
.sm-light { flex: 1 1 0; height: 2px; border-radius: 999px; background-color: color-mix(in oklab, var(--line-strong) 40%, transparent); overflow: hidden; }
.sm-light__on { height: 2px; border-radius: 999px; background-color: var(--accent); }
.sm-panel {
    position: absolute; left: 0; top: 32px; z-index: 70; width: 336px;
    flex-direction: column;
    border: 1px solid var(--line-strong); border-radius: 6px; background-color: var(--panel-3);
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
}
.sm-presets { flex-wrap: wrap; gap: 4px; padding: 8px; border-bottom: 1px solid var(--line); }
.sm-family { flex-direction: column; gap: 6px; padding: 8px 12px; border-top: 1px solid var(--line); }
.sm-family:first-child { border-top-width: 0; }
.sm-family__head { align-items: center; gap: 8px; font-size: 12px; color: var(--ink); }
.sm-family__count { font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.sm-keys { flex-wrap: wrap; gap: 4px; padding-left: 22px; }
.sm-key {
    align-items: center; gap: 6px; height: 24px; padding: 0 6px; display: flex;
    border: 1px solid var(--line); border-radius: 3px; background-color: var(--panel-2);
    font-family: var(--mono); font-size: 11px; color: var(--ink-faint);
}
.sm-key:hover { border-color: var(--line-strong); color: var(--ink-dim); }
.sm-key.on { border-color: var(--accent-dim); background-color: color-mix(in oklab, var(--accent) 10%, transparent); color: var(--ink); }
.sm-dot { width: 6px; height: 6px; border-radius: 999px; background-color: var(--line-strong); }
.sm-key.on .sm-dot { background-color: var(--accent); }
.sm-unidentified { align-items: center; gap: 8px; padding: 8px 12px; border-top: 1px solid var(--line); font-size: 12px; color: var(--ink-dim); }
"#
);

fn settings_of(store: Store, node: &str) -> SpectrumMonitorNode {
    match store.graph.get().node(node).map(|found| &found.body) {
        Some(NodeBody::SpectrumMonitor(settings)) => settings.clone(),
        _ => SpectrumMonitorNode::default(),
    }
}

fn edit(store: Store, node: &str, change: impl FnOnce(&mut SpectrumMonitorNode) + 'static) {
    store.edit_node(node, move |body| {
        if let NodeBody::SpectrumMonitor(settings) = body {
            change(settings);
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("spectrum-monitor", SHEET);
    kit_channel::install();
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let groups = Memo::new(move |_| protocol_groups(&store.channel_types.get()));
    let choice = Memo::new(move |_| {
        let held = settings.get();
        ProtocolChoice {
            disabled: held.disabled_protocols,
            unidentified: held.report_unidentified,
        }
    });
    let pick_choice = {
        let node = node.clone();
        move |next: ProtocolChoice| {
            edit(store, &node, move |held| {
                held.disabled_protocols = next.disabled;
                held.report_unidentified = next.unidentified;
            });
        }
    };
    let confidence = {
        let node = node.clone();
        move |next: Option<f64>| {
            if let Some(next) = next {
                edit(store, &node, move |held| {
                    held.min_confidence = (next / 100.0) as f32
                });
            }
        }
    };
    let record = move |on: bool| edit(store, &node, move |held| held.record_audio = on);
    view! {
        column(class = "sm-face kc-grid") {
            {setting_row("Protocols", None, protocol_picker(groups, choice, pick_choice))}
            {setting_row(
                "Min confidence",
                Some("Ignore signals below this identification confidence; 0 accepts all detections"),
                number_field(
                    Signal::derive(move || Some(f64::from((settings.get().min_confidence * 100.0).round()))),
                    NumberSpec::new("Minimum confidence").limit(NumberLimit::new(0.0, 100.0, 5.0)).unit("%"),
                    confidence,
                ),
            )}
            {toggle_row(
                "Record audio",
                Some("Attach temporary audio clips to transmission events"),
                Signal::derive(move || settings.get().record_audio),
                record,
            )}
        }
    }
}

fn protocol_picker(
    groups: Memo<Vec<ProtocolGroup>>,
    choice: Memo<ProtocolChoice>,
    on_change: impl Fn(ProtocolChoice) + Clone + 'static,
) -> impl IntoView {
    let popover = Popover::new();
    let lights = move || {
        let disabled = choice.get().disabled;
        groups
            .get()
            .iter()
            .map(|group| {
                let on = enabled_kinds(std::slice::from_ref(group), &disabled).len();
                let share = on as f32 * 100.0 / group.protocols.len().max(1) as f32;
                view! {
                    box(class = "sm-light") {
                        box(class = "sm-light__on", style:width = Some(format!("{share}%"))) {}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        box(class = "kc-pop") {
            control(
                class = "sm-trigger",
                class:on = move || popover.open(),
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                a11y:label = "Protocols to decode",
                on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                    ev.stop_propagation();
                    popover.toggle();
                }
            ) {
                text(class = "sm-trigger__text") {{move || choice_summary(&groups.get(), &choice.get())}}
                {icon(CHEVRON_ICON, "kc-glyph kc-glyph--sm")}
                row(class = "sm-lights") {{lights}}
            }
            {move || popover.open().then(|| AnyView::new(panel(groups, choice, on_change.clone())))}
        }
    }
}

fn panel(
    groups: Memo<Vec<ProtocolGroup>>,
    choice: Memo<ProtocolChoice>,
    on_change: impl Fn(ProtocolChoice) + Clone + 'static,
) -> impl IntoView {
    let presets = {
        let on_change = on_change.clone();
        move || {
            let active = active_preset(&groups.get(), &choice.get()).map(|preset| preset.id);
            protocol_presets(&groups.get())
                .into_iter()
                .map(|preset| {
                    let on = active.as_deref() == Some(preset.id.as_str());
                    let on_change = on_change.clone();
                    let picked = preset.choice.clone();
                    view! {
                        control(
                            class = "kc-btn kc-btn--sm",
                            class:on = on,
                            tabindex = Focus::Sequential,
                            a11y:role = Role::Button,
                            on:click:stop = move |_| on_change(picked.clone())
                        ) {{preset.label}}
                    }
                })
                .collect::<Vec<_>>()
        }
    };
    let toggle_kinds = {
        let on_change = on_change.clone();
        move |kinds: Vec<String>, enabled: bool| {
            let mut next = choice.get_untracked();
            next.disabled = set_enabled(&next.disabled, &kinds, enabled);
            on_change(next);
        }
    };
    let families = move || {
        groups
            .get()
            .into_iter()
            .map(|group| family(group, choice, toggle_kinds.clone()))
            .collect::<Vec<_>>()
    };
    let unidentified = Signal::derive(move || choice.get().unidentified);
    view! {
        column(class = "sm-panel", on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation()) {
            row(class = "sm-presets") {{presets}}
            column {{families}}
            row(class = "sm-unidentified") {
                {check(unidentified, move |on| {
                    let mut next = choice.get_untracked();
                    next.unidentified = on;
                    on_change(next);
                })}
                text {"Unidentified signals"}
            }
        }
    }
}

fn family(
    group: ProtocolGroup,
    choice: Memo<ProtocolChoice>,
    toggle: impl Fn(Vec<String>, bool) + Clone + 'static,
) -> AnyView {
    let kinds: Vec<String> = group
        .protocols
        .iter()
        .map(|protocol| protocol.kind.clone())
        .collect();
    let total = kinds.len();
    let counted = group.clone();
    let on_count =
        move || enabled_kinds(std::slice::from_ref(&counted), &choice.get().disabled).len();
    let all_on = {
        let on_count = on_count.clone();
        Signal::derive(move || on_count() == total)
    };
    let whole = {
        let toggle = toggle.clone();
        let kinds = kinds.clone();
        move |on: bool| toggle(kinds.clone(), on)
    };
    let keys = group
        .protocols
        .into_iter()
        .map(|protocol| {
            let kind = protocol.kind.clone();
            let lit = move || !choice.get().disabled.contains(&kind);
            let toggle = toggle.clone();
            let flip = protocol.kind.clone();
            view! {
                control(
                    class = "sm-key",
                    class:on = lit.clone(),
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    a11y:label = protocol.name.clone(),
                    on:click:stop = move |_| toggle(vec![flip.clone()], !lit())
                ) {
                    box(class = "sm-dot") {}
                    {protocol.label}
                }
            }
        })
        .collect::<Vec<_>>();
    AnyView::new(view! {
        column(class = "sm-family") {
            row(class = "sm-family__head") {
                {check(all_on, whole)}
                text {{group.title}}
                spacer() {}
                text(class = "sm-family__count") {{move || format!("{}/{total}", on_count())}}
            }
            row(class = "sm-keys") {{keys}}
        }
    })
}
