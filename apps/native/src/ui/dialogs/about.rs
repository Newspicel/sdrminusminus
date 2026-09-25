use std::sync::Arc;

use sdrmm_wire::about::{AboutResponse, Attribution, LicenseTextResponse};
use zgui::prelude::*;

use crate::{
    shell::about::{group_components, noted_components, summary_line},
    store::Store,
    ui::{
        dialogs::frame,
        files::open_link,
        kit_shell::{Entry, button, entry},
        shell::Shell,
    },
};

const SHEET: &str = css!(
    r#"
.about__link { font-size: 12px; color: var(--accent); }
.about__link:hover { color: var(--ink); }
.about__section { font-size: 12px; font-weight: 600; color: var(--ink); margin-top: 6px; }
.about__row { align-items: baseline; gap: 8px; flex-wrap: wrap; padding: 1px 0; }
.about__name { font-size: 12px; color: var(--ink); }
.about__name.linked:hover { color: var(--accent); }
.about__version { font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.about__license { font-size: 12px; color: var(--ink-dim); }
.about__text { font-size: 11px; color: var(--accent-dim); }
.about__text:hover { color: var(--accent); }
.about__note { font-size: 11px; color: var(--ink-dim); }
.about__filter { align-items: center; gap: 12px; margin-top: 6px; }
.about__overlay { position: absolute; left: 0; top: 0; right: 0; bottom: 0; z-index: 5; flex-direction: column; gap: 10px; padding: 16px; border-radius: 8px; background-color: var(--panel); }
.about__frame { position: relative; flex-direction: column; gap: 8px; min-height: 0; }
"#
);

type Loaded<T> = RwSignal<Option<Result<Arc<T>, String>>>;

fn fetch<T: serde::de::DeserializeOwned + Send + Sync + 'static>(
    store: Store,
    path: String,
    into: Loaded<T>,
) {
    zgui::task::spawn_local(async move {
        let result = store.api().get::<T>(&path).await;
        into.set(Some(
            result.map(Arc::new).map_err(|error| error.to_string()),
        ));
    });
}

pub fn dialog(store: Store, shell: Shell) -> impl IntoView {
    install_stylesheet("shell-about", SHEET);
    let about: Loaded<AboutResponse> = RwSignal::new(None);
    fetch(store, String::from("/api/about"), about);
    let reading = RwSignal::new(None::<String>);
    let title = move || match about.get() {
        Some(Ok(about)) => format!("SDR-- {}", about.version),
        _ => String::from("SDR--"),
    };
    let aside = move || match about.get() {
        Some(Ok(about)) => format!("{} licensed", about.license),
        Some(Err(_)) => String::new(),
        None => String::from("Loading"),
    };
    let body = move || match about.get() {
        Some(Ok(about)) => AnyView::new(contents(store, about, reading)),
        Some(Err(_)) => {
            AnyView::new(view! { text(class = "sk-text bad") {"Could not load the notices."} })
        }
        None => AnyView::new(()),
    };
    frame(
        "wide",
        title,
        view! { text(class = "legend") {{aside}} },
        view! {
            column(class = "about__frame") {
                {body}
                {move || reading.get().map(|id| AnyView::new(license_text(store, id, reading)))}
            }
        },
        view! {
            spacer() {}
            {button("Close", "", move || shell.close())}
        },
    )
}

fn contents(
    store: Store,
    about: Arc<AboutResponse>,
    reading: RwSignal<Option<String>>,
) -> impl IntoView {
    let query = RwSignal::new_local(String::new());
    let repository = about.repository.clone();
    let license = RwSignal::new(false);
    let license_text = about.license_text.clone();
    let noted: Vec<AnyView> = noted_components(&about.components)
        .into_iter()
        .map(|component| {
            let note = component.note.clone().unwrap_or_default();
            AnyView::new(view! {
                column {
                    {component_row(store, component, reading)}
                    text(class = "about__note") {{note}}
                }
            })
        })
        .collect();
    let count = about.components.len();
    let summary = summary_line(&about.components);
    let components = about.clone();
    let groups = move || {
        let groups = group_components(&components.components, &query.get());
        if groups.is_empty() {
            return vec![AnyView::new(view! {
                text(class = "sk-text") {{format!("Nothing matches \u{201c}{}\u{201d}.", query.get_untracked())}}
            })];
        }
        groups
            .into_iter()
            .map(|group| {
                let heading = format!("{} ({})", group.label, group.components.len());
                let rows: Vec<AnyView> = group
                    .components
                    .into_iter()
                    .map(|component| AnyView::new(component_row(store, component, reading)))
                    .collect();
                AnyView::new(view! {
                    column {
                        text(class = "legend") {{heading}}
                        {rows}
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    view! {
        control(
            class = "about__link",
            tabindex = Focus::Sequential,
            a11y:role = Role::Link,
            on:click:stop = move |_| open_link(store, &repository)
        ) {
            {about.repository.clone()}
        }
        control(
            class = "about__section",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            on:click:stop = move |_| license.update(|open| *open = !*open)
        ) {
            "License"
        }
        {move || license.get().then(|| view! { text(class = "sk-pre") {{license_text.clone()}} })}
        {(!noted.is_empty()).then(|| view! {
            text(class = "about__section") {"Worth knowing"}
            {noted}
        })}
        row(class = "about__filter") {
            text(class = "about__section") {{format!("Third-party components ({count})")}}
            spacer() {}
            {entry(Entry::new(query, "Filter by name or license").narrow(), || {}, || {})}
        }
        text(class = "legend") {{summary}}
        {groups}
    }
}

fn component_row(
    store: Store,
    component: Attribution,
    reading: RwSignal<Option<String>>,
) -> impl IntoView {
    let name = component.name.clone();
    let name_view = match component.url.clone() {
        Some(url) => AnyView::new(view! {
            control(
                class = "about__name linked",
                tabindex = Focus::Sequential,
                a11y:role = Role::Link,
                on:click:stop = move |_| open_link(store, &url)
            ) {
                {name}
            }
        }),
        None => AnyView::new(view! { text(class = "about__name") {{name}} }),
    };
    let several = component.texts.len() > 1;
    let texts: Vec<AnyView> = component
        .texts
        .iter()
        .enumerate()
        .map(|(at, id)| {
            let id = id.clone();
            let label = if several {
                format!("text {}", at + 1)
            } else {
                String::from("text")
            };
            AnyView::new(view! {
                control(
                    class = "about__text",
                    tabindex = Focus::Sequential,
                    a11y:role = Role::Button,
                    on:click:stop = move |_| reading.set(Some(id.clone()))
                ) {
                    {label}
                }
            })
        })
        .collect();
    view! {
        row(class = "about__row") {
            {name_view}
            {component.version.clone().map(|version| view! { text(class = "about__version") {{version}} })}
            text(class = "about__license") {{component.license.clone()}}
            {texts}
        }
    }
}

fn license_text(store: Store, id: String, reading: RwSignal<Option<String>>) -> impl IntoView {
    let text: Loaded<LicenseTextResponse> = RwSignal::new(None);
    fetch(store, format!("/api/about/licenses/{id}"), text);
    let shown = move || match text.get() {
        Some(Ok(license)) => license.text.clone(),
        Some(Err(_)) => String::from("Could not load this license text."),
        None => String::from("Loading"),
    };
    view! {
        column(class = "about__overlay") {
            row(class = "sk-toolbar") {
                text(class = "sk-dialog__title") {"License text"}
                spacer() {}
                {button("Back", "", move || reading.set(None))}
            }
            text(class = "sk-pre") {{shown}}
        }
    }
}
