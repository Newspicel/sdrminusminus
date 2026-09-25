use sdrmm_wire::{
    CreatedRowId,
    cps::{CpsCodeplugInfo, CpsDeviceRequest, CpsUserRequest},
};
use zgui::prelude::*;
use zgui_ui::prelude::*;

use super::{Cps, logic::model_label};
use crate::ui::{tools::kit::button, widgets::pick};

#[derive(Clone, Copy)]
enum Kind {
    User,
    Device,
    Codeplug,
}

impl Kind {
    fn path(self, id: i64) -> String {
        let table = match self {
            Self::User => "users",
            Self::Device => "devices",
            Self::Codeplug => "codeplugs",
        };
        format!("/api/cps/{table}/{id}")
    }
}

fn drop_row(cps: Cps, kind: Kind, id: i64) {
    let api = cps.store.api();
    let path = kind.path(id);
    cps.act(async move { api.delete(&path).await }, move |()| {
        if matches!(kind, Kind::Codeplug) && cps.selected.get_untracked() == Some(id) {
            cps.selected.set(None);
        }
        cps.refresh_library();
    });
}

pub fn parse_dmr_id(text: &str) -> Result<Option<u32>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|_| format!("{trimmed} is not a DMR ID"))
}

fn remove(cps: Cps, kind: Kind, id: i64) -> impl IntoView {
    button(
        "btn small",
        || "Remove".to_owned(),
        || false,
        move || drop_row(cps, kind, id),
    )
}

pub fn panel(cps: Cps) -> impl IntoView {
    view! {
        {operators(cps)}
        {radios(cps)}
        {codeplugs(cps)}
    }
}

fn operators(cps: Cps) -> impl IntoView {
    let name = RwSignal::new_local(String::new());
    let callsign = RwSignal::new_local(String::new());
    let dmr = RwSignal::new_local(String::new());
    let rows = move || {
        let users = cps.library.data.with(|data| {
            data.as_ref()
                .map(|data| data.users.clone())
                .unwrap_or_default()
        });
        users
            .into_iter()
            .map(|user| {
                let label = format!(
                    "{}{}",
                    user.callsign.clone().unwrap_or_else(|| user.name.clone()),
                    user.dmr_id
                        .map(|id| format!(" \u{b7} {id}"))
                        .unwrap_or_default()
                );
                AnyView::new(view! {
                    row(class = "cps__item") {
                        text(class = "tool-ink") {{label}}
                        {remove(cps, Kind::User, user.id)}
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    let add = move || {
        let dmr_id = match parse_dmr_id(&dmr.get_untracked()) {
            Ok(id) => id,
            Err(error) => {
                cps.failure.set(Some(error));
                return;
            }
        };
        let callsign_text = callsign.get_untracked().trim().to_owned();
        let body = CpsUserRequest {
            name: name.get_untracked().trim().to_owned(),
            callsign: (!callsign_text.is_empty()).then_some(callsign_text),
            dmr_id,
            note: None,
        };
        let api = cps.store.api();
        cps.act(
            async move { api.post::<_, CreatedRowId>("/api/cps/users", &body).await },
            move |_| {
                name.set(String::new());
                callsign.set(String::new());
                dmr.set(String::new());
                cps.refresh_library();
            },
        );
    };
    view! {
        column(class = "tool-group") {
            text(class = "legend") {"Operators"}
            {rows}
            row(class = "tool-bar") {
                box(class = "tool-text narrow") { Input(class = "native-input", value = name, label = "Operator name", placeholder = "Name") }
                box(class = "tool-text narrow") { Input(class = "native-input", value = callsign, label = "Callsign", placeholder = "Callsign") }
                box(class = "tool-text narrow") { Input(class = "native-input", value = dmr, label = "DMR ID", placeholder = "DMR ID") }
                {button("btn small", || "Add".to_owned(), move || name.with(|name| name.trim().is_empty()) || cps.pending.get(), add)}
            }
        }
    }
}

fn radios(cps: Cps) -> impl IntoView {
    let name = RwSignal::new_local(String::new());
    let model = RwSignal::new(String::new());
    let model_id = move || {
        let picked = model.get();
        if picked.is_empty() {
            cps.model_list()
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_default()
        } else {
            picked
        }
    };
    let rows = move || {
        let devices = cps.library.data.with(|data| {
            data.as_ref()
                .map(|data| data.devices.clone())
                .unwrap_or_default()
        });
        devices
            .into_iter()
            .map(|device| {
                AnyView::new(view! {
                    row(class = "cps__item") {
                        row(class = "tool-bar") {
                            text(class = "tool-ink") {{device.name.clone()}}
                            text(class = "tool-faint") {{device.model_id.clone()}}
                        }
                        {remove(cps, Kind::Device, device.id)}
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    let models = move || {
        let options: Vec<(String, String)> = cps
            .model_list()
            .iter()
            .map(|entry| (entry.id.clone(), model_label(entry)))
            .collect();
        pick(
            options,
            Signal::derive(move || Some(model_id())),
            move |picked| model.set(picked),
        )
    };
    let add = move || {
        let body = CpsDeviceRequest {
            name: name.get_untracked().trim().to_owned(),
            model_id: model_id(),
            ..CpsDeviceRequest::default()
        };
        let api = cps.store.api();
        cps.act(
            async move { api.post::<_, CreatedRowId>("/api/cps/devices", &body).await },
            move |_| {
                name.set(String::new());
                cps.refresh_library();
            },
        );
    };
    view! {
        column(class = "tool-group") {
            text(class = "legend") {"Radios"}
            {rows}
            row(class = "tool-bar") {
                box(class = "tool-text narrow") { Input(class = "native-input", value = name, label = "Radio name", placeholder = "Name") }
                box(class = "cps__pick") {{models}}
                {button("btn small", || "Add".to_owned(), move || name.with(|name| name.trim().is_empty()) || model_id().is_empty() || cps.pending.get(), add)}
            }
        }
    }
}

fn codeplugs(cps: Cps) -> impl IntoView {
    let rows = move || {
        let infos = cps.library.data.with(|data| {
            data.as_ref()
                .map(|data| data.codeplugs.clone())
                .unwrap_or_default()
        });
        if infos.is_empty() {
            return vec![AnyView::new(
                view! { text(class = "tool-dim") {"Nothing stored yet."} },
            )];
        }
        infos
            .into_iter()
            .map(|info| codeplug_row(cps, info))
            .collect()
    };
    view! {
        column(class = "tool-group") {
            text(class = "legend") {"Codeplugs"}
            {rows}
        }
    }
}

#[must_use]
pub fn codeplug_summary(info: &CpsCodeplugInfo) -> String {
    format!(
        "{} \u{b7} {} ch \u{b7} {} contacts \u{b7} {} zones",
        info.model_id, info.counts.channels, info.counts.contacts, info.counts.zones
    )
}

fn codeplug_row(cps: Cps, info: CpsCodeplugInfo) -> AnyView {
    let id = info.id;
    let summary = codeplug_summary(&info);
    AnyView::new(view! {
        row(class = "cps__item", class:on = move || cps.selected.get() == Some(id)) {
            control(
                class = "cps__pick",
                a11y:role = Role::Button,
                tabindex = Focus::Sequential,
                on:click:stop = move |_| cps.selected.update(|selected| {
                    *selected = if *selected == Some(id) { None } else { Some(id) };
                })
            ) {
                text(class = "tool-ink") {{info.name.clone()}}
                text(class = "tool-faint") {{summary}}
            }
            {remove(cps, Kind::Codeplug, id)}
        }
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::CodeplugCounts;

    use super::*;

    #[test]
    fn a_dmr_id_is_optional_but_must_be_a_number() {
        assert_eq!(parse_dmr_id("  "), Ok(None));
        assert_eq!(parse_dmr_id(" 2320001 "), Ok(Some(2_320_001)));
        assert!(parse_dmr_id("abc").is_err());
    }

    #[test]
    fn a_stored_codeplug_reads_as_model_and_counts() {
        let info = CpsCodeplugInfo {
            id: 3,
            name: "Home".to_owned(),
            model_id: "radtel-rt4d".to_owned(),
            device_id: None,
            user_id: None,
            counts: CodeplugCounts {
                channels: 35,
                contacts: 2,
                group_lists: 0,
                zones: 4,
                scan_lists: 0,
                radio_ids: 1,
            },
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(
            codeplug_summary(&info),
            "radtel-rt4d \u{b7} 35 ch \u{b7} 2 contacts \u{b7} 4 zones"
        );
        assert_eq!(Kind::Codeplug.path(3), "/api/cps/codeplugs/3");
    }
}
