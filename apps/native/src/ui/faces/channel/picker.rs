use sdrmm_wire::channel::{ChannelDescriptor, DecoderFamily};
use zgui::{prelude::*, view::OverlayLayer};
use zgui_ui::prelude::*;

use super::swap::swap_decoder;
use crate::{
    store::Store,
    ui::kit_channel::{button, format_hz},
};

pub const FAMILIES: [(DecoderFamily, &str); 9] = [
    (DecoderFamily::AnalogVoice, "Analog voice"),
    (DecoderFamily::DigitalVoice, "Digital voice"),
    (DecoderFamily::Aviation, "Aviation"),
    (DecoderFamily::Marine, "Marine"),
    (DecoderFamily::Amateur, "Amateur and HF"),
    (DecoderFamily::Paging, "Paging and telemetry"),
    (DecoderFamily::Video, "Pictures and video"),
    (DecoderFamily::Broadcast, "Broadcast digital"),
    (DecoderFamily::Utility, "Utility"),
];

const SHEET: &str = css!(
    r#"
.cp-scrim { position: absolute; left: 0; top: 0; right: 0; bottom: 0; background-color: rgba(10, 11, 13, 0.7); align-items: center; justify-content: center; display: flex; }
.cp-dialog {
    width: 512px; max-height: 80%;
    flex-direction: column; padding: 16px;
    border: 1px solid var(--line-strong); border-radius: 6px;
    background-color: var(--panel-3);
    box-shadow: 0 22px 52px rgba(0, 0, 0, 0.62);
}
.cp-title { font-family: var(--mono); font-size: 13px; color: var(--ink); }
.cp-note { margin-top: 4px; font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.cp-search { margin-top: 12px; }
.cp-search .native-input {
    height: 28px; width: 100%; padding: 3px 8px;
    font-family: var(--mono); font-size: 12px;
    background-color: var(--panel-2); color: var(--ink);
    border: 1px solid var(--line); border-radius: 5px;
}
.cp-search .native-input:focus-visible { border-color: var(--accent); outline: 1px solid var(--accent); }
.cp-list { margin-top: 8px; flex-direction: column; gap: 12px; overflow: auto; flex: 1 1 auto; min-height: 0; }
.cp-group { flex-direction: column; gap: 4px; }
.cp-group__title { padding: 0 8px 4px 8px; border-bottom: 1px solid var(--line); font-family: var(--mono); font-size: 10px; color: var(--ink-faint); }
.cp-grid { flex-direction: row; flex-wrap: wrap; }
.cp-entry {
    width: 50%; flex-direction: column; gap: 1px; padding: 4px 8px;
    border: 1px solid transparent; border-radius: 3px;
    display: flex;
}
.cp-entry:hover, .cp-entry:focus-visible { background-color: var(--panel-2); border-color: var(--accent-dim); }
.cp-entry__head { justify-content: space-between; gap: 8px; align-items: baseline; }
.cp-entry__name { font-family: var(--mono); font-size: 12px; color: var(--ink); overflow: hidden; }
.cp-entry:hover .cp-entry__name { color: var(--accent); }
.cp-entry__width { font-family: var(--mono); font-size: 10px; color: var(--ink-faint); flex: 0 0 auto; }
.cp-entry__summary { font-size: 11px; color: var(--ink-faint); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cp-empty { padding: 40px 0; text-align: center; color: var(--ink-faint); }
.cp-foot { margin-top: 16px; justify-content: flex-end; }
"#
);

#[derive(Clone, Debug, PartialEq)]
pub struct DecoderGroup {
    pub title: &'static str,
    pub items: Vec<ChannelDescriptor>,
}

#[must_use]
pub fn decoder_groups(types: &[ChannelDescriptor]) -> Vec<DecoderGroup> {
    FAMILIES
        .iter()
        .map(|(family, title)| DecoderGroup {
            title,
            items: types
                .iter()
                .filter(|descriptor| descriptor.family == *family)
                .cloned()
                .collect(),
        })
        .filter(|group| !group.items.is_empty())
        .collect()
}

#[must_use]
pub fn decoder_replacements(types: &[ChannelDescriptor], current: &str) -> Vec<DecoderGroup> {
    let others: Vec<ChannelDescriptor> = types
        .iter()
        .filter(|descriptor| descriptor.type_id != current)
        .cloned()
        .collect();
    decoder_groups(&others)
}

#[must_use]
pub fn channel_picker(types: &[ChannelDescriptor], suggested: &str) -> Vec<DecoderGroup> {
    let groups = decoder_groups(types);
    let Some(found) = types
        .iter()
        .find(|descriptor| descriptor.type_id == suggested)
    else {
        return groups;
    };
    let mut listed = vec![DecoderGroup {
        title: "Suggested",
        items: vec![found.clone()],
    }];
    listed.extend(groups);
    listed
}

fn matches(descriptor: &ChannelDescriptor, needle: &str) -> bool {
    [
        descriptor.name.as_str(),
        &format!("channel:{}", descriptor.type_id),
        descriptor.summary.as_str(),
    ]
    .iter()
    .any(|text| text.to_lowercase().contains(needle))
}

#[must_use]
pub fn filter_groups(groups: &[DecoderGroup], query: &str) -> Vec<DecoderGroup> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return groups.to_vec();
    }
    groups
        .iter()
        .map(|group| DecoderGroup {
            title: group.title,
            items: group
                .items
                .iter()
                .filter(|item| matches(item, &needle))
                .cloned()
                .collect(),
        })
        .filter(|group| !group.items.is_empty())
        .collect()
}

#[must_use]
pub fn first_item(groups: &[DecoderGroup]) -> Option<&ChannelDescriptor> {
    groups.first().and_then(|group| group.items.first())
}

pub fn channel_picker_dialog(
    title: &'static str,
    note: Signal<String>,
    groups: Signal<Vec<DecoderGroup>>,
    on_channel: impl Fn(String) + Clone + 'static,
    on_close: impl Fn() + Clone + 'static,
) -> impl IntoView {
    install_stylesheet("channel-picker", SHEET);
    let query = RwSignal::new_local(String::new());
    let shown = move || filter_groups(&groups.get(), &query.get());
    let enter = {
        let on_channel = on_channel.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| match ev.key {
            Key::Named(NamedKey::Enter) => {
                if let Some(first) = first_item(&filter_groups(
                    &groups.get_untracked(),
                    &query.get_untracked(),
                )) {
                    on_channel(first.type_id.clone());
                }
                ev.prevent_default();
            }
            _ => {}
        }
    };
    let escape = {
        let on_close = on_close.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                on_close();
                ev.stop_propagation();
            }
        }
    };
    let dismiss = on_close.clone();
    let list = move || {
        let groups = shown();
        if groups.is_empty() {
            return AnyView::new(
                view! { text(class = "cp-empty") {"Nothing matches. Try a mode or a task."} },
            );
        }
        let on_channel = on_channel.clone();
        AnyView::new(
            groups
                .into_iter()
                .map(|group| {
                    let on_channel = on_channel.clone();
                    view! {
                        column(class = "cp-group") {
                            text(class = "cp-group__title") {{group.title}}
                            row(class = "cp-grid") {{group.items.into_iter().map(|item| AnyView::new(entry(item, on_channel.clone()))).collect::<Vec<_>>()}}
                        }
                    }
                })
                .collect::<Vec<_>>(),
        )
    };
    zgui::view::Portal::new(move || {
        AnyView::new(view! {
            box(
                class = "cp-scrim",
                on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| { ev.stop_propagation(); dismiss(); },
                on:key_down = escape,
            ) {
                column(
                    class = "cp-dialog",
                    a11y:role = Role::Dialog,
                    a11y:label = title,
                    on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                ) {
                    text(class = "cp-title") {{title}}
                    text(class = "cp-note") {{move || note.get()}}
                    box(class = "cp-search") {
                        Input(
                            value = query,
                            class = "native-input",
                            label = "Search channel modes",
                            placeholder = "nfm, adsb…",
                            on:key_down = enter,
                        )
                    }
                    column(class = "cp-list", on:wheel = |ev: &mut EventCx<'_, events::Wheel>| ev.stop_propagation()) {{list}}
                    row(class = "cp-foot") {
                        {button("Cancel", "kc-btn", Signal::stored(false), on_close)}
                    }
                }
            }
        })
    })
    .layer(OverlayLayer::Modal)
}

fn entry(item: ChannelDescriptor, on_channel: impl Fn(String) + 'static) -> impl IntoView {
    let type_id = item.type_id.clone();
    let width = format_hz(item.bandwidth_hz);
    let summary = item.summary.clone();
    view! {
        control(
            class = "cp-entry",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:description = summary.clone(),
            on:click:stop = move |_| on_channel(type_id.clone())
        ) {
            row(class = "cp-entry__head") {
                text(class = "cp-entry__name") {{item.name}}
                text(class = "cp-entry__width") {{width}}
            }
            {(!summary.is_empty()).then(|| view! { text(class = "cp-entry__summary") {{summary}} })}
        }
    }
}

pub fn replace_decoder(
    store: Store,
    node: String,
    on_close: impl Fn() + Clone + 'static,
) -> impl IntoView {
    let current = {
        let node = node.clone();
        Signal::derive(move || {
            super::swap::current_type(&store.graph.get(), &node).unwrap_or_default()
        })
    };
    let note = {
        let node = node.clone();
        Signal::derive(move || {
            let type_id = current.get();
            let name = store
                .descriptor_of(&type_id)
                .map_or_else(|| type_id.to_uppercase(), |descriptor| descriptor.name);
            match store.channel_settings(&node) {
                Some(settings) => format!("{name}: {}", format_hz(settings.frequency_hz)),
                None => name,
            }
        })
    };
    let groups =
        Signal::derive(move || decoder_replacements(&store.channel_types.get(), &current.get()));
    let close = on_close.clone();
    channel_picker_dialog(
        "Replace the decoder",
        note,
        groups,
        move |type_id| {
            swap_decoder(store, &node, &type_id);
            close();
        },
        on_close,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(type_id: &str, name: &str, family: DecoderFamily) -> ChannelDescriptor {
        ChannelDescriptor {
            type_id: type_id.into(),
            name: name.into(),
            family,
            ..ChannelDescriptor::default()
        }
    }

    fn types() -> Vec<ChannelDescriptor> {
        vec![
            descriptor("nfm", "NFM", DecoderFamily::AnalogVoice),
            descriptor("am", "AM", DecoderFamily::AnalogVoice),
            descriptor("dmr", "DMR", DecoderFamily::DigitalVoice),
            descriptor("adsb", "ADS-B", DecoderFamily::Aviation),
        ]
    }

    #[test]
    fn decoders_group_by_family_in_family_order() {
        let groups = decoder_groups(&types());
        let titles: Vec<&str> = groups.iter().map(|group| group.title).collect();
        assert_eq!(titles, vec!["Analog voice", "Digital voice", "Aviation"]);
        assert_eq!(groups[0].items.len(), 2);
    }

    #[test]
    fn a_replacement_never_offers_the_decoder_already_there() {
        let groups = decoder_replacements(&types(), "nfm");
        assert!(
            groups
                .iter()
                .all(|group| group.items.iter().all(|item| item.type_id != "nfm"))
        );
        assert_eq!(
            first_item(&groups).map(|item| item.type_id.as_str()),
            Some("am")
        );
    }

    #[test]
    fn a_suggested_type_comes_first() {
        let groups = channel_picker(&types(), "adsb");
        assert_eq!(groups[0].title, "Suggested");
        assert_eq!(
            first_item(&groups).map(|item| item.type_id.as_str()),
            Some("adsb")
        );
        assert_eq!(channel_picker(&types(), "none").len(), 3);
    }

    #[test]
    fn a_search_keeps_matching_items_and_drops_empty_groups() {
        let found = filter_groups(&decoder_groups(&types()), " ADS ");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].items[0].type_id, "adsb");
        assert_eq!(filter_groups(&decoder_groups(&types()), "").len(), 3);
        assert!(filter_groups(&decoder_groups(&types()), "zzz").is_empty());
        assert_eq!(
            filter_groups(&decoder_groups(&types()), "channel:dmr").len(),
            1
        );
    }
}
