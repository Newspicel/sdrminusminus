pub mod codeplug;
pub mod library;
pub mod logic;
pub mod report;

use std::{future::Future, time::Duration};

use sdrmm_wire::cps::{
    ConversionReport, CpsCodeplugDetail, CpsConvertRequest, CpsConvertResponse, CpsIdentifyRequest,
    CpsJob, CpsJobsResponse, CpsLibraryResponse, CpsMergeRequest, CpsPortsResponse, CpsReadRequest,
    CpsWriteRequest, MergeMode, RadioIdent, RadioModelDescriptor, RadioModelsResponse,
};
use zgui::prelude::*;
use zgui_ui::prelude::*;

use self::logic::{
    any_active, candidate_models, describe_job, job_is_active, job_percent, latest_job,
    model_label, port_options,
};
use crate::{
    store::Store,
    ui::{
        tools::kit::{Query, alert, button, labelled},
        widgets::pick,
    },
};

const POLL: Duration = Duration::from_millis(400);
const IDLE_POLLS: u32 = 5;

#[derive(Clone, Copy)]
pub struct Cps {
    pub store: Store,
    pub ports: Query<CpsPortsResponse>,
    pub models: Query<RadioModelsResponse>,
    pub library: Query<CpsLibraryResponse>,
    pub jobs: Query<CpsJobsResponse>,
    pub codeplug: Query<CpsCodeplugDetail>,
    pub identify: Query<RadioIdent>,
    pub selected: RwSignal<Option<i64>>,
    pub report: RwSignal<Option<ConversionReport>>,
    pub pending: RwSignal<bool>,
    pub failure: RwSignal<Option<String>>,
}

impl Cps {
    fn new(store: Store) -> Self {
        Self {
            store,
            ports: Query::new(),
            models: Query::new(),
            library: Query::new(),
            jobs: Query::new(),
            codeplug: Query::new(),
            identify: Query::new(),
            selected: RwSignal::new(None),
            report: RwSignal::new(None),
            pending: RwSignal::new(false),
            failure: RwSignal::new(None),
        }
    }

    pub fn refresh_library(self) {
        let api = self.store.api();
        self.library
            .run(async move { api.get("/api/cps/library").await });
    }

    pub fn refresh_jobs(self) {
        let api = self.store.api();
        self.jobs.run(async move { api.get("/api/cps/jobs").await });
    }

    fn refresh_all(self) {
        let api = self.store.api();
        self.ports
            .run(async move { api.get("/api/cps/ports").await });
        let api = self.store.api();
        self.models
            .run(async move { api.get("/api/cps/models").await });
        self.refresh_library();
        self.refresh_jobs();
    }

    fn load_codeplug(self, id: Option<i64>) {
        let Some(id) = id else {
            self.codeplug.clear();
            return;
        };
        let api = self.store.api();
        self.codeplug
            .run(async move { api.get(&format!("/api/cps/codeplugs/{id}")).await });
    }

    pub fn act<T: 'static>(
        self,
        future: impl Future<Output = anyhow::Result<T>> + 'static,
        done: impl FnOnce(T) + 'static,
    ) {
        self.pending.set(true);
        zgui::task::spawn_local(async move {
            let result = future.await;
            if self.pending.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(value) => {
                    self.failure.set(None);
                    done(value);
                }
                Err(error) => self.failure.set(Some(format!("{error:#}"))),
            }
        });
    }

    pub fn model_list(self) -> Vec<RadioModelDescriptor> {
        self.models.data.with(|data| {
            data.as_ref()
                .map(|data| data.models.clone())
                .unwrap_or_default()
        })
    }

    fn running(self) -> bool {
        self.jobs
            .data
            .with(|data| data.as_ref().is_some_and(|data| any_active(&data.jobs)))
    }
}

#[derive(Clone, Copy)]
struct Choice {
    port: RwSignal<String>,
    model: RwSignal<String>,
    user: RwSignal<i64>,
    name: RwSignal<String, LocalStorage>,
    target: RwSignal<String>,
    source: RwSignal<i64>,
    mode: RwSignal<MergeMode>,
}

impl Choice {
    fn new() -> Self {
        Self {
            port: RwSignal::new(String::new()),
            model: RwSignal::new(String::new()),
            user: RwSignal::new(0),
            name: RwSignal::new_local(String::new()),
            target: RwSignal::new(String::new()),
            source: RwSignal::new(0),
            mode: RwSignal::new(MergeMode::Union),
        }
    }

    fn user_id(self) -> Option<i64> {
        let user = self.user.get_untracked();
        (user != 0).then_some(user)
    }
}

fn offered(cps: Cps, choice: Choice) -> Vec<RadioModelDescriptor> {
    let port = choice.port.get();
    let chosen = cps.ports.data.with(|data| {
        data.as_ref()
            .and_then(|data| data.ports.iter().find(|entry| entry.port == port).cloned())
    });
    candidate_models(chosen.as_ref(), &cps.model_list())
}

fn model_of(cps: Cps, choice: Choice) -> String {
    let picked = choice.model.get();
    if picked.is_empty() {
        offered(cps, choice)
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default()
    } else {
        picked
    }
}

pub fn panel(store: Store) -> impl IntoView {
    let cps = Cps::new(store);
    let choice = Choice::new();
    cps.refresh_all();
    let ticks = RwSignal::new(0u32);
    let was_active = RwSignal::new(false);
    let poll = set_interval(POLL, move || {
        let active = cps.running();
        let tick = ticks.get_untracked().wrapping_add(1);
        ticks.set(tick);
        if active || tick.is_multiple_of(IDLE_POLLS) {
            cps.refresh_jobs();
        }
        if was_active.get_untracked() && !active {
            cps.refresh_library();
        }
        was_active.set(active);
    });
    on_cleanup_local(move || drop(poll));
    let follow = zgui::reactive::RenderEffect::new(move |_| cps.load_codeplug(cps.selected.get()));
    on_cleanup_local(move || drop(follow));

    view! {
        column(class = "cps") {
            {top_bar(cps, choice)}
            {job_bar(cps)}
            {alert(move || cps.failure.get().or_else(|| cps.identify.error.get()))}
            row(class = "cps__main") {
                column(class = "cps__side") {{library::panel(cps)}}
                column(class = "cps__work") {
                    {copy_bar(cps, choice)}
                    {report::view(cps)}
                    {codeplug_area(cps)}
                }
            }
        }
    }
}

fn top_bar(cps: Cps, choice: Choice) -> impl IntoView {
    let ports = move || {
        let mut options = vec![(String::new(), "Pick a port\u{2026}".to_owned())];
        options.extend(cps.ports.data.with(|data| {
            data.as_ref()
                .map(|data| port_options(&data.ports))
                .unwrap_or_default()
        }));
        pick(
            options,
            Signal::derive(move || Some(choice.port.get())),
            move |port| {
                choice.port.set(port);
                choice.model.set(String::new());
            },
        )
    };
    let models = move || {
        let options: Vec<(String, String)> = offered(cps, choice)
            .iter()
            .map(|model| (model.id.clone(), model_label(model)))
            .collect();
        pick(
            options,
            Signal::derive(move || Some(model_of(cps, choice))),
            move |model| choice.model.set(model),
        )
    };
    let users = move || {
        let mut options = vec![(0, "Leave as read".to_owned())];
        options.extend(cps.library.data.with(|data| {
            data.as_ref().map_or_else(Vec::new, |data| {
                data.users
                    .iter()
                    .map(|user| {
                        (
                            user.id,
                            user.callsign.clone().unwrap_or_else(|| user.name.clone()),
                        )
                    })
                    .collect()
            })
        }));
        pick(
            options,
            Signal::derive(move || Some(choice.user.get())),
            move |user| choice.user.set(user),
        )
    };
    let unready =
        move || choice.port.get().is_empty() || model_of(cps, choice).is_empty() || cps.running();
    view! {
        row(class = "tool-box") {
            {labelled("Port", ports)}
            {labelled("Radio", models)}
            {labelled("Operator", users)}
            {button("btn", || "Identify".to_owned(), move || unready() || cps.identify.busy.get(), move || identify(cps, choice))}
            box(class = "tool-text") {
                Input(class = "native-input", value = choice.name, label = "Name for the read codeplug", placeholder = "Name the read")
            }
            {button("btn primary", || "Read radio".to_owned(), unready, move || read(cps, choice))}
            {button("btn danger", || "Write to radio".to_owned(), move || unready() || cps.selected.get().is_none(), move || write(cps, choice))}
            {move || cps.identify.data.get().map(|ident| {
                let firmware = ident.firmware.map(|firmware| format!(" \u{b7} {firmware}")).unwrap_or_default();
                AnyView::new(view! { text(class = "tool-chip") {{format!("{}{firmware}", ident.reported_model)}} })
            })}
        }
    }
}

fn identify(cps: Cps, choice: Choice) {
    let body = CpsIdentifyRequest {
        model_id: model_of(cps, choice),
        port: choice.port.get_untracked(),
    };
    let api = cps.store.api();
    cps.identify
        .run(async move { api.post("/api/cps/identify", &body).await });
}

fn read(cps: Cps, choice: Choice) {
    let name = choice.name.get_untracked().trim().to_owned();
    let body = CpsReadRequest {
        model_id: model_of(cps, choice),
        port: choice.port.get_untracked(),
        name: (!name.is_empty()).then_some(name),
        device_id: None,
        user_id: choice.user_id(),
    };
    let api = cps.store.api();
    cps.act(
        async move { api.post::<_, CpsJob>("/api/cps/read", &body).await },
        move |_| {
            cps.refresh_library();
            cps.refresh_jobs();
        },
    );
}

fn write(cps: Cps, choice: Choice) {
    let Some(codeplug_id) = cps.selected.get_untracked() else {
        cps.failure.set(Some("Pick a codeplug first".to_owned()));
        return;
    };
    let body = CpsWriteRequest {
        model_id: model_of(cps, choice),
        port: choice.port.get_untracked(),
        codeplug_id,
        user_id: choice.user_id(),
        device_id: None,
        confirm: true,
        restore_image: false,
    };
    let api = cps.store.api();
    cps.act(
        async move { api.post::<_, CpsJob>("/api/cps/write", &body).await },
        move |_| {
            cps.refresh_library();
            cps.refresh_jobs();
        },
    );
}

fn job_bar(cps: Cps) -> impl IntoView {
    move || {
        let job = cps.jobs.data.with(|data| {
            data.as_ref()
                .and_then(|data| latest_job(&data.jobs).cloned())
        })?;
        let percent = job_percent(&job);
        let id = job.id;
        let stop = job_is_active(&job).then(|| {
            AnyView::new(button(
                "btn small",
                || "Stop".to_owned(),
                || false,
                move || {
                    let api = cps.store.api();
                    cps.act(
                        async move { api.delete(&format!("/api/cps/jobs/{id}")).await },
                        move |()| cps.refresh_jobs(),
                    );
                },
            ))
        });
        Some(AnyView::new(view! {
            row(class = "cps__job") {
                text(class = "cps__job-text") {{describe_job(&job)}}
                box(class = "cps__progress") {
                    box(class = "cps__progress-fill", style:width = Some(format!("{percent:.1}%")))
                }
                {stop}
            }
        }))
    }
}

const MERGE_MODES: [(MergeMode, &str); 3] = [
    (MergeMode::Union, "Add what is missing"),
    (MergeMode::Append, "Append"),
    (MergeMode::Replace, "Replace"),
];

fn copy_bar(cps: Cps, choice: Choice) -> impl IntoView {
    let targets = move || {
        let options: Vec<(String, String)> = cps
            .model_list()
            .iter()
            .map(|model| (model.id.clone(), model_label(model)))
            .collect();
        let shown = Signal::derive(move || {
            let target = choice.target.get();
            Some(if target.is_empty() {
                model_of(cps, choice)
            } else {
                target
            })
        });
        pick(options, shown, move |model| choice.target.set(model))
    };
    let sources = move || {
        let selected = cps.selected.get();
        let mut options = vec![(0, "Pick a codeplug\u{2026}".to_owned())];
        options.extend(cps.library.data.with(|data| {
            data.as_ref().map_or_else(Vec::new, |data| {
                data.codeplugs
                    .iter()
                    .filter(|info| Some(info.id) != selected)
                    .map(|info| (info.id, info.name.clone()))
                    .collect()
            })
        }));
        pick(
            options,
            Signal::derive(move || Some(choice.source.get())),
            move |source| choice.source.set(source),
        )
    };
    let modes: Vec<(MergeMode, String)> = MERGE_MODES
        .iter()
        .map(|(mode, label)| (*mode, (*label).to_owned()))
        .collect();
    view! {
        row(class = "tool-row") {
            {labelled("Copy to", targets)}
            {button("btn", || "Copy for that radio".to_owned(), move || cps.selected.get().is_none() || cps.pending.get(), move || convert(cps, choice))}
            {labelled("Take entries from", sources)}
            {labelled("Mode", pick(modes, Signal::derive(move || Some(choice.mode.get())), move |mode| choice.mode.set(mode)))}
            {button("btn", || "Merge in".to_owned(), move || cps.selected.get().is_none() || choice.source.get() == 0 || cps.pending.get(), move || merge(cps, choice))}
        }
    }
}

fn convert(cps: Cps, choice: Choice) {
    let Some(id) = cps.selected.get_untracked() else {
        return;
    };
    let target = choice.target.get_untracked();
    let body = CpsConvertRequest {
        target_model_id: if target.is_empty() {
            model_of(cps, choice)
        } else {
            target
        },
        name: None,
        user_id: choice.user_id(),
        device_id: None,
        store: true,
    };
    let api = cps.store.api();
    cps.act(
        async move {
            api.post::<_, CpsConvertResponse>(&format!("/api/cps/codeplugs/{id}/convert"), &body)
                .await
        },
        move |result| {
            cps.report.set(Some(result.report));
            cps.refresh_library();
        },
    );
}

fn merge(cps: Cps, choice: Choice) {
    let (Some(id), source) = (cps.selected.get_untracked(), choice.source.get_untracked()) else {
        return;
    };
    if source == 0 {
        return;
    }
    let body = CpsMergeRequest {
        source_id: source,
        mode: choice.mode.get_untracked(),
        parts: Vec::new(),
    };
    let api = cps.store.api();
    cps.act(
        async move {
            api.post::<_, CpsConvertResponse>(&format!("/api/cps/codeplugs/{id}/merge"), &body)
                .await
        },
        move |result| {
            cps.report.set(Some(result.report));
            cps.refresh_library();
            cps.load_codeplug(Some(id));
        },
    );
}

fn codeplug_area(cps: Cps) -> impl IntoView {
    let loaded = Memo::new(move |_| cps.codeplug.data.with(Option::is_some));
    move || {
        if loaded.get() {
            return AnyView::new(codeplug::view(cps.codeplug));
        }
        let hint = if cps.selected.get().is_none() {
            "Pick a codeplug on the left, or read one off a radio."
        } else {
            "Loading the codeplug\u{2026}"
        };
        AnyView::new(view! { text(class = "tool-dim") {{hint}} })
    }
}
