use sdrmm_wire::{CreatedRowId, channel::ChannelDescriptor, rest::CreateBookmarkRequest};
use zgui::prelude::*;
use zgui_ui::prelude::*;

use super::{
    MenuAt, ScopeCx,
    actions::{add_channel_at, tunable_now, tune_to, untracked_radio},
    bands::{identify, suggested_at},
    pick::{BookmarkDraft, bookmark_draft, channel_type_at, format_hz, pick_text},
};

const MENU_HALF_PX: f64 = 112.0;
const MENU_HEIGHT_PX: f64 = 192.0;

#[must_use]
pub fn menu_place(x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
    let left = (x * width).clamp(MENU_HALF_PX, (width - MENU_HALF_PX).max(MENU_HALF_PX));
    let top = (y * height).clamp(0.0, (height - MENU_HEIGHT_PX).max(0.0));
    (left, top)
}

fn plot_size(cx: ScopeCx) -> (f64, f64) {
    let scale = f64::from(cx.plot.scale().max(0.01));
    cx.plot.bounds().map_or((0.0, 0.0), |bounds| {
        (
            f64::from(bounds.size.width.0) / scale,
            f64::from(bounds.size.height.0) / scale,
        )
    })
}

fn fresh(cx: ScopeCx, held: RwSignal<Option<MenuAt>>) -> Option<MenuAt> {
    held.get().filter(|at| at.stamp == cx.frame_stamp())
}

fn row(label: &'static str, run: impl Fn() + 'static) -> impl IntoView {
    view! {
        control(
            class = "scope__menu-row",
            a11y:role = Role::MenuItem,
            on:click:stop = move |_| run()
        ) {
            {label}
        }
    }
}

fn copy(cx: ScopeCx, what: &'static str, value: String) {
    match try_use_clipboard() {
        Some(clipboard) => {
            clipboard.set_text(ClipboardKind::Standard, value.clone());
            cx.store.say(format!("{what} copied: {value}"));
        }
        None => cx.store.say("No clipboard here"),
    }
    cx.menu.set(None);
}

fn save_bookmark(cx: ScopeCx, label: String, hz: f64, mode: Option<String>) {
    let store = cx.store;
    zgui::task::spawn_local(async move {
        let body = CreateBookmarkRequest {
            label: label.trim().to_owned(),
            freq_hz: hz,
            mode,
            group: None,
        };
        match store
            .api()
            .post::<_, CreatedRowId>("/api/bookmarks", &body)
            .await
        {
            Ok(_) => cx.menu.set(None),
            Err(error) => store.say(format!("cannot save the bookmark: {error}")),
        }
    });
}

fn bookmark_form(cx: ScopeCx, at: MenuAt, draft: BookmarkDraft) -> impl IntoView {
    let label = RwSignal::new_local(draft.label.clone());
    let mode = draft.mode.clone();
    let heading = match &draft.mode {
        Some(mode) => format!("Bookmark · {mode}"),
        None => String::from("Bookmark"),
    };
    let submit = move || {
        let text = label.get_untracked();
        if !text.trim().is_empty() {
            save_bookmark(cx, text, at.pick.hz, mode.clone());
        }
    };
    let keys = {
        let submit = submit.clone();
        move |ev: &mut EventCx<'_, events::KeyDown>| {
            if matches!(ev.key, Key::Named(NamedKey::Enter)) {
                submit();
            }
        }
    };
    view! {
        column(class = "scope__form") {
            text(class = "scope__faint") {{heading}}
            Input(class = "scope__field", value = label, label = "Bookmark label", on:key_down = keys)
            control(
                class = "scope__menu-row",
                state:disabled = move || label.with(|text| text.trim().is_empty()),
                on:click:stop = move |_| submit()
            ) {
                "Save bookmark"
            }
        }
    }
}

fn menu_body(cx: ScopeCx, at: MenuAt) -> impl IntoView {
    let (frequency, offset) = pick_text(at.pick);
    let plan = cx.plan.get_untracked();
    let draft = bookmark_draft(at.pick.hz, plan.as_deref());
    let marking = RwSignal::new(false);
    let (width, height) = plot_size(cx);
    let (left, top) = menu_place(at.x, at.y, width, height);
    let form = move || {
        let draft = draft.clone();
        if marking.get() {
            AnyView::new(bookmark_form(cx, at, draft))
        } else {
            AnyView::new(row("Mark this frequency…", move || marking.set(true)))
        }
    };
    view! {
        column(
            class = "scope__menu",
            a11y:role = Role::Menu,
            style:left = Some(format!("{left:.1}px")),
            style:top = Some(format!("{top:.1}px")),
            on:pointer_down:stop = move |_| {},
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                    cx.menu.set(None);
                }
            }
        ) {
            text(class = "scope__menu-head") {{format_hz(at.pick.hz)}}
            {row("Tune here", move || {
                tune_to(cx, at.pick);
                cx.menu.set(None);
            })}
            {row("New channel here…", move || {
                cx.picker.set(Some(at));
                cx.menu.set(None);
            })}
            {row("Copy frequency", move || copy(cx, "Frequency", frequency.clone()))}
            {row("Copy offset", move || copy(cx, "Offset", offset.clone()))}
            {form}
        }
    }
}

pub fn menu(cx: ScopeCx) -> impl IntoView {
    move || fresh(cx, cx.menu).map(|at| AnyView::new(menu_body(cx, at)))
}

#[must_use]
pub fn picker_order(descriptors: &[ChannelDescriptor], suggested: &str) -> Vec<(String, String)> {
    let mut listed: Vec<(String, String)> = descriptors
        .iter()
        .map(|descriptor| (descriptor.type_id.clone(), descriptor.name.clone()))
        .collect();
    listed.sort_by(|a, b| {
        (a.0 != suggested)
            .cmp(&(b.0 != suggested))
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    listed
}

fn suggested_type(cx: ScopeCx, hz: f64) -> String {
    let plan = cx.plan.get_untracked();
    let suggested = plan
        .as_deref()
        .and_then(|plan| suggested_at(&identify(plan, hz)));
    let radio = untracked_radio(cx);
    let listening = tunable_now(cx, &radio).and_then(|channel| radio.channel(channel).cloned());
    channel_type_at(suggested.as_ref(), listening.as_ref())
}

fn picker_body(cx: ScopeCx, at: MenuAt) -> impl IntoView {
    let suggested = suggested_type(cx, at.pick.hz);
    let rows: Vec<_> = picker_order(&cx.store.channel_types.get_untracked(), &suggested)
        .into_iter()
        .map(|(type_id, name)| {
            let on = type_id == suggested;
            view! {
                control(
                    class = "scope__menu-row",
                    class:on = on,
                    a11y:role = Role::MenuItem,
                    on:click:stop = move |_| {
                        add_channel_at(cx, at.pick, type_id.clone());
                        cx.picker.set(None);
                    }
                ) {
                    {name}
                }
            }
        })
        .collect();
    view! {
        column(
            class = "scope__picker",
            on:pointer_down:stop = move |_| {},
            on:wheel = move |ev: &mut EventCx<'_, events::Wheel>| ev.stop_propagation(),
            on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                if matches!(ev.key, Key::Named(NamedKey::Escape)) {
                    cx.picker.set(None);
                }
            }
        ) {
            row(class = "scope__picker-head") {
                text {"New channel"}
                text(class = "scope__faint") {{format_hz(at.pick.hz)}}
                spacer()
                control(class = "scope__close", a11y:label = "Close", on:click:stop = move |_| cx.picker.set(None)) {"x"}
            }
            column(class = "scope__picker-list") {{rows}}
        }
    }
}

pub fn picker(cx: ScopeCx) -> impl IntoView {
    move || fresh(cx, cx.picker).map(|at| AnyView::new(picker_body(cx, at)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(type_id: &str, name: &str) -> ChannelDescriptor {
        serde_json::from_value(serde_json::json!({
            "type_id": type_id,
            "name": name,
            "bandwidth_hz": 12500.0,
            "input_rate_hz": 48000.0
        }))
        .expect("a descriptor")
    }

    #[test]
    fn the_suggested_decoder_leads_the_picker() {
        let listed = [
            descriptor("wfm", "Wide FM"),
            descriptor("am", "AM"),
            descriptor("nfm", "Narrow FM"),
        ];
        let order: Vec<String> = picker_order(&listed, "nfm")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(order, ["nfm", "am", "wfm"]);
    }

    #[test]
    fn the_menu_stays_inside_the_plot() {
        assert_eq!(menu_place(0.0, 0.0, 500.0, 400.0), (112.0, 0.0));
        assert_eq!(menu_place(1.0, 1.0, 500.0, 400.0), (388.0, 208.0));
        assert_eq!(menu_place(0.5, 0.25, 500.0, 400.0), (250.0, 100.0));
    }
}
