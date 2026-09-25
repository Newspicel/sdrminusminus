use serde_json::Value;
use zgui::prelude::*;
use zgui_ui::prelude::*;

use super::widgets::{check, pick, row_field};
use crate::{
    params::{self, Control, Field},
    store::Store,
};

pub fn panel(store: Store, node: String, type_id: &str) -> AnyView {
    match params::fields(type_id) {
        Ok(fields) => AnyView::new(view! {
            column(class = "params") {
                {fields.into_iter().map(|field| {
                    let owner = node.clone();
                    let name = field.name.clone();
                    let visible = move || store.channel_of(&owner).is_none_or(|channel| params::visible(&channel.settings.params, &name));
                    let content = control(store, node.clone(), field);
                    view! { box(class = "params__row", style:display = move || Some(if visible() { "flex" } else { "none" }.to_owned())) {{content}} }
                }).collect::<Vec<_>>()}
            }
        }),
        Err(error) => AnyView::new(view! { text(class = "field__error") {{error.to_string()}} }),
    }
}

fn control(store: Store, node: String, field: Field) -> AnyView {
    let source = node.clone();
    let name = field.name.clone();
    let value = Signal::derive(move || {
        store
            .channel_of(&source)
            .and_then(|channel| serde_json::to_value(channel.settings.params).ok())
            .map(|params| params["settings"][&name].clone())
            .unwrap_or(Value::Null)
    });
    let change_field = field.clone();
    let commit = move |value| -> Result<(), String> {
        let Some(channel) = store.channel_of(&node) else {
            return Err("Connect a radio first".into());
        };
        let mut settings = channel.settings;
        let limits = store
            .descriptor_of(settings.params.type_id())
            .map(|descriptor| descriptor.limits)
            .unwrap_or_default();
        settings.params = params::edited(&settings.params, &change_field, value, &limits)
            .map_err(|error| error.to_string())?;
        store.set_channel(node.clone(), settings);
        Ok(())
    };
    let field_label = field.label.clone();
    let body = match &field.control {
        Control::Toggle => AnyView::new(check(
            Signal::derive(move || value.get().as_bool().unwrap_or(false)),
            move |on| {
                if let Err(error) = commit(Value::Bool(on)) {
                    store.say(error);
                }
            },
        )),
        Control::Choice(choices) => {
            let mut options = choices
                .iter()
                .map(|choice| (Some(choice.clone()), params::label(choice)))
                .collect::<Vec<_>>();
            if field.optional {
                options.insert(0, (None, "Auto".into()));
            }
            let selected = Signal::derive(move || Some(value.get().as_str().map(str::to_owned)));
            AnyView::new(pick(options, selected, move |chosen| {
                if let Err(error) = commit(chosen.map(Value::String).unwrap_or(Value::Null)) {
                    store.say(error);
                }
            }))
        }
        Control::Text | Control::Number { .. } => {
            let shown = Signal::derive(move || match value.get() {
                Value::Null => String::new(),
                Value::String(value) => value,
                Value::Number(number) => number
                    .as_f64()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| number.to_string()),
                value => value.to_string(),
            });
            let label = field.label.clone();
            let optional = field.optional;
            AnyView::new(entry(shown, label, optional, move |text| {
                let value = params::parse(&field, &text).map_err(|error| error.to_string())?;
                commit(value)
            }))
        }
    };
    AnyView::new(row_field(field_label, body))
}

pub fn entry(
    value: Signal<String>,
    label: String,
    optional: bool,
    commit: impl Fn(String) -> Result<(), String> + Clone + 'static,
) -> impl IntoView {
    let draft = RwSignal::new_local(value.get_untracked());
    let focused = RwSignal::new_local(false);
    let error = RwSignal::new_local(None::<String>);
    let sync = zgui::reactive::RenderEffect::new(move |_| {
        let current = value.get();
        if !focused.get() {
            draft.set(current);
        }
    });
    on_cleanup_local(move || drop(sync));
    let submit = move || {
        if draft.get_untracked() != value.get_untracked() {
            error.set(commit(draft.get_untracked()).err());
        }
    };
    let blur = submit.clone();
    view! {
        column(class = "entry", on:key_down = crate::ui::kit_shell::typing) {
            Input(
                value = draft,
                class = "native-input",
                label = label,
                placeholder = if optional { "Auto / off" } else { "" },
                invalid = Signal::derive_local(move || error.get().is_some()),
                on:focus_in = move |_| focused.set(true),
                on:focus_out = move |_| { blur(); focused.set(false); },
                on:key_down = move |ev: &mut EventCx<'_, events::KeyDown>| {
                    match ev.key {
                        Key::Named(NamedKey::Enter) => { submit(); ev.prevent_default(); },
                        Key::Named(NamedKey::Escape) => { draft.set(value.get_untracked()); error.set(None); ev.prevent_default(); },
                        _ => {}
                    }
                },
            )
            if move || error.get().is_some() {
                text(class = "field__error") {{move || error.get().unwrap_or_default()}}
            }
        }
    }
}
