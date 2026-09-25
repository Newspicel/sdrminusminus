use std::net::Ipv4Addr;

use sdrmm_wire::{
    event_output::{EventOutputTarget, WebhookFormat},
    patch::NodeBody,
    ws::ServerEvent,
};
use zgui::{prelude::*, reactive::RenderEffect};

use crate::{
    decoders::output::{
        BeastState, SERVICES, Service, beast_line, empty_hint, new_target, service_of,
    },
    store::Store,
    ui::{
        kit_decoders,
        params::entry,
        widgets::{pick, row_field},
    },
};

type Commit = std::rc::Rc<dyn Fn(EventOutputTarget)>;

fn target_of(store: Store, node: &str) -> Option<EventOutputTarget> {
    match &store.graph.get().node(node)?.body {
        NodeBody::EventOutput(output) => Some(output.target.clone()),
        _ => None,
    }
}

fn inputs_of(store: Store, node: &str) -> usize {
    store.graph.get().sources_of(node, "events").count()
}

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_decoders::install();
    let target = {
        let node = node.clone();
        Memo::new(move |_| target_of(store, &node))
    };
    let inputs = {
        let node = node.clone();
        Memo::new(move |_| inputs_of(store, &node))
    };
    let commit: Commit = {
        let node = node.clone();
        std::rc::Rc::new(move |next: EventOutputTarget| {
            let node = node.clone();
            store.edit_graph(move |graph| {
                if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
                    && let NodeBody::EventOutput(output) = &mut found.body
                {
                    output.target = next;
                }
            });
        })
    };
    let options: Vec<(Service, String)> = SERVICES
        .iter()
        .map(|(service, label)| (*service, (*label).to_owned()))
        .collect();
    let chosen = Signal::derive(move || target.get().as_ref().map(service_of));
    let pick_service = {
        let commit = commit.clone();
        move |service: Service| {
            if chosen.get_untracked() != Some(service) {
                commit(new_target(service));
            }
        }
    };
    let fields = {
        let commit = commit.clone();
        move || target.get().map(|current| fields(current, commit.clone()))
    };
    let status = move || {
        let current = target.get()?;
        Some(match &current {
            EventOutputTarget::Beast { .. } => {
                AnyView::new(beast(store, node.clone(), target, inputs, commit.clone()))
            }
            _ => {
                AnyView::new(view! { text(class = "hint") {{empty_hint(inputs.get(), &current)}} })
            }
        })
    };
    view! {
        column(class = "face", {..kit_decoders::no_pan()}) {
            {row_field("Service", pick(options, chosen, pick_service))}
            {fields}
            {status}
        }
    }
}

fn text_field(
    name: &'static str,
    value: String,
    write: impl Fn(String) -> Result<(), String> + Clone + 'static,
) -> AnyView {
    AnyView::new(row_field(
        name,
        entry(Signal::stored(value), name.to_owned(), false, write),
    ))
}

fn secret_field(
    name: &'static str,
    value: String,
    write: impl Fn(String) -> Result<(), String> + Clone + 'static,
) -> AnyView {
    let editing = RwSignal::new(false);
    let set = !value.is_empty();
    let body = move || {
        if editing.get() {
            let write = write.clone();
            AnyView::new(entry(
                Signal::stored(String::new()),
                name.to_owned(),
                false,
                move |next| {
                    editing.set(false);
                    write(next)
                },
            ))
        } else {
            AnyView::new(view! {
                row(class = "dk-line") {
                    text(class = "dk-num") {{if set { "••••••" } else { "not set" }}}
                    control(class = "btn dk-push", on:click:stop = move |_| editing.set(true)) {"Change"}
                }
            })
        }
    };
    AnyView::new(row_field(name, body))
}

fn fields(target: EventOutputTarget, commit: Commit) -> Vec<AnyView> {
    let shown = target.clone();
    let edit = move |change: fn(&mut EventOutputTarget, String) -> Result<(), String>| {
        let commit = commit.clone();
        let current = target.clone();
        move |value: String| {
            let mut next = current.clone();
            change(&mut next, value)?;
            commit(next);
            Ok(())
        }
    };
    match &shown {
        EventOutputTarget::Beast { address, .. } => vec![text_field(
            "Listen on",
            address.clone(),
            edit(|t, v| {
                if let EventOutputTarget::Beast { address, enabled } = t {
                    *address = v;
                    *enabled = false;
                }
                Ok(())
            }),
        )],
        EventOutputTarget::Tunnel {
            interface,
            address,
            prefix,
        } => vec![
            text_field(
                "Interface",
                interface.clone(),
                edit(|t, v| {
                    if let EventOutputTarget::Tunnel { interface, .. } = t {
                        *interface = v;
                    }
                    Ok(())
                }),
            ),
            text_field(
                "Local IPv4",
                address.to_string(),
                edit(|t, v| {
                    let parsed = v
                        .trim()
                        .parse::<Ipv4Addr>()
                        .map_err(|_| "not an IPv4 address".to_owned())?;
                    if let EventOutputTarget::Tunnel { address, .. } = t {
                        *address = parsed;
                    }
                    Ok(())
                }),
            ),
            text_field(
                "Prefix",
                prefix.to_string(),
                edit(|t, v| {
                    let parsed = v
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .filter(|p| *p <= 32)
                        .ok_or_else(|| "0 to 32".to_owned())?;
                    if let EventOutputTarget::Tunnel { prefix, .. } = t {
                        *prefix = parsed;
                    }
                    Ok(())
                }),
            ),
        ],
        EventOutputTarget::Webhook { url, format } => {
            let formats = vec![
                (WebhookFormat::Json, "JSON".to_owned()),
                (WebhookFormat::Discord, "Discord".to_owned()),
            ];
            let chosen = Signal::stored(Some(*format));
            let pick_format = {
                let write = edit(|t, v| {
                    if let EventOutputTarget::Webhook { format, .. } = t {
                        *format = if v == "discord" {
                            WebhookFormat::Discord
                        } else {
                            WebhookFormat::Json
                        };
                    }
                    Ok(())
                });
                move |format: WebhookFormat| {
                    let name = if format == WebhookFormat::Discord {
                        "discord"
                    } else {
                        "json"
                    };
                    if let Err(error) = write(name.to_owned()) {
                        tracing::debug!(%error, "webhook format");
                    }
                }
            };
            vec![
                secret_field(
                    "Endpoint",
                    url.clone(),
                    edit(|t, v| {
                        if let EventOutputTarget::Webhook { url, .. } = t {
                            *url = v;
                        }
                        Ok(())
                    }),
                ),
                AnyView::new(row_field("Format", pick(formats, chosen, pick_format))),
            ]
        }
        _ => credential_fields(&shown, edit),
    }
}

fn credential_fields<W>(
    target: &EventOutputTarget,
    edit: impl Fn(fn(&mut EventOutputTarget, String) -> Result<(), String>) -> W,
) -> Vec<AnyView>
where
    W: Fn(String) -> Result<(), String> + Clone + 'static,
{
    macro_rules! slot {
        ($variant:ident, $field:ident) => {
            |t: &mut EventOutputTarget, v: String| {
                if let EventOutputTarget::$variant { $field, .. } = t {
                    *$field = v;
                }
                Ok(())
            }
        };
    }
    match target {
        EventOutputTarget::Matrix {
            homeserver_url,
            room_id,
            access_token,
        } => vec![
            text_field(
                "Homeserver",
                homeserver_url.clone(),
                edit(slot!(Matrix, homeserver_url)),
            ),
            text_field("Room ID", room_id.clone(), edit(slot!(Matrix, room_id))),
            secret_field(
                "Access token",
                access_token.clone(),
                edit(slot!(Matrix, access_token)),
            ),
        ],
        EventOutputTarget::Mqtt {
            broker_url,
            topic,
            username,
            password,
        } => vec![
            text_field("Broker", broker_url.clone(), edit(slot!(Mqtt, broker_url))),
            text_field("Topic", topic.clone(), edit(slot!(Mqtt, topic))),
            text_field("Username", username.clone(), edit(slot!(Mqtt, username))),
            secret_field("Password", password.clone(), edit(slot!(Mqtt, password))),
        ],
        EventOutputTarget::Postgres {
            url,
            table,
            username,
            password,
        } => vec![
            text_field("Server", url.clone(), edit(slot!(Postgres, url))),
            text_field("Table", table.clone(), edit(slot!(Postgres, table))),
            text_field(
                "Username",
                username.clone(),
                edit(slot!(Postgres, username)),
            ),
            secret_field(
                "Password",
                password.clone(),
                edit(slot!(Postgres, password)),
            ),
        ],
        EventOutputTarget::Influx {
            url,
            bucket,
            org,
            token,
        } => vec![
            text_field("Server", url.clone(), edit(slot!(Influx, url))),
            text_field("Bucket", bucket.clone(), edit(slot!(Influx, bucket))),
            text_field("Org", org.clone(), edit(slot!(Influx, org))),
            secret_field("Token", token.clone(), edit(slot!(Influx, token))),
        ],
        _ => Vec::new(),
    }
}

fn beast(
    store: Store,
    node: String,
    target: Memo<Option<EventOutputTarget>>,
    inputs: Memo<usize>,
    commit: Commit,
) -> impl IntoView {
    let status = RwSignal::new(None::<(String, BeastState)>);
    let heard = node.clone();
    store.on_event(move |event: &ServerEvent| {
        if let ServerEvent::BeastExportStatus(update) = event
            && update.node == heard
        {
            status.set(Some((
                update.address.clone(),
                BeastState {
                    listening: update.listening,
                    clients: update.clients,
                    frames: update.frames,
                    error: update.error.clone(),
                },
            )));
        }
    });
    let reset = RenderEffect::new(move |_| {
        store.connected.track();
        status.set(None);
    });
    on_cleanup_local(move || drop(reset));
    let listener = move || match target.get() {
        Some(EventOutputTarget::Beast { address, enabled }) => Some((address, enabled)),
        _ => None,
    };
    let current = move || {
        let (address, enabled) = listener()?;
        let connected = inputs.get() > 0;
        status
            .get()
            .filter(|(heard, _)| connected && enabled && *heard == address)
            .map(|(_, state)| state)
    };
    let line = move || {
        let (_, enabled) = listener().unwrap_or_default();
        beast_line(inputs.get() > 0, enabled, current().as_ref())
    };
    let error = move || {
        current()
            .and_then(|state| state.error)
            .map(|error| AnyView::new(view! { text(class = "dk-danger") {{error}} }))
    };
    let toggle = move |_: &mut EventCx<'_, events::Click>| {
        if let Some((address, enabled)) = listener() {
            status.set(None);
            commit(EventOutputTarget::Beast {
                address,
                enabled: !enabled,
            });
        }
    };
    let enabled = move || listener().is_some_and(|(_, enabled)| enabled);
    let blocked = move || {
        !enabled()
            && (inputs.get() == 0
                || listener().is_none_or(|(address, _)| address.trim().is_empty()))
    };
    view! {
        column(class = "dk-pane") {
            {error}
            text(class = "dk-num") {{line}}
            row(class = "face__foot") {
                control(class = "btn", state:disabled = blocked, on:click:stop = toggle) {
                    {move || if enabled() { "Close server" } else { "Open server" }}
                }
            }
        }
    }
}
