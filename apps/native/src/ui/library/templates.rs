use sdrmm_wire::{
    rest::{ApplyTemplateRequest, TemplateInfo, TemplatesResponse},
    units,
};
use zgui::prelude::*;

use crate::{
    shell::templates::{supports, templates_hint},
    store::Store,
    ui::{
        kit_shell::{Row, gated_button, glyph, hint, list, list_row, row_action},
        library::{active_set, load},
    },
};

pub fn panel(store: Store) -> impl IntoView {
    let templates = load::<TemplatesResponse>(store, String::from("/api/templates"));
    let active = active_set(store);
    let applied = RwSignal::new(None::<TemplateInfo>);
    let explained = RwSignal::new(None::<String>);
    let listed = move || {
        templates
            .get()
            .and_then(Result::ok)
            .map(|found| found.templates.clone())
            .unwrap_or_default()
    };
    let top_hint = move || templates_hint(&listed(), active.get().as_ref()).map(hint);
    let banner = move || {
        applied.get().map(|template| {
            AnyView::new(view! {
                column(class = "sk-list") {
                    column(class = "sk-row") {
                        text(class = "sk-row__primary") {{template.name.clone()}}
                        text(class = "sk-text") {{template.explainer.clone()}}
                    }
                }
            })
        })
    };
    let rows = move || {
        listed()
            .into_iter()
            .map(|template| AnyView::new(template_row(store, template, applied, explained)))
            .collect::<Vec<_>>()
    };
    let failed = move || {
        matches!(templates.get(), Some(Err(_))).then(|| hint("Could not load the templates."))
    };
    view! {
        column(class = "sk-panel") {
            {top_hint}
            {failed}
            {banner}
            {list(None, rows)}
        }
    }
}

fn template_row(
    store: Store,
    template: TemplateInfo,
    applied: RwSignal<Option<TemplateInfo>>,
    explained: RwSignal<Option<String>>,
) -> impl IntoView {
    let active = active_set(store);
    let fits = {
        let template = template.clone();
        Signal::derive_local(move || supports(&template, active.get().as_ref()))
    };
    let badge = format!(
        "{} \u{b7} {} \u{b7} {} ch",
        units::hertz(template.center_hz),
        units::sample_rate(template.sample_rate),
        template.channels.len()
    );
    let chosen = template.clone();
    let apply = move || {
        let Some(set) = active.get_untracked() else {
            return;
        };
        let template = chosen.clone();
        zgui::task::spawn_local(async move {
            let body = ApplyTemplateRequest { device_set: set.id };
            let result = store
                .api()
                .post::<_, serde::de::IgnoredAny>(
                    &format!("/api/templates/{}/apply", template.id),
                    &body,
                )
                .await;
            match result {
                Ok(_) => {
                    applied.set(Some(template));
                    store.apply().await;
                }
                Err(error) => store.fail("Cannot apply the template", &error),
            }
            store.refresh_state();
        });
    };
    let id = template.id.clone();
    let open_id = template.id.clone();
    let explainer = template.explainer.clone();
    let below = move || {
        (explained.get().as_deref() == Some(open_id.as_str()))
            .then(|| AnyView::new(view! { text(class = "sk-text") {{explainer.clone()}} }))
    };
    let actions = view! {
        {row_action(glyph::INFO, "About this template", false, move || {
            explained.update(|open| {
                *open = if open.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
            });
        })}
        {gated_button("Apply", "sm", fits, apply)}
    };
    list_row(
        Row {
            primary: format!("{}  {badge}", template.name),
            secondary: Some(template.description.clone()),
        },
        None,
        Signal::stored_local(true),
        AnyView::new(actions),
        AnyView::new(below),
    )
}
