use sdrmm_wire::{
    filter::{EventFilterNode, FilterMode, MAX_FILTER_DURATION_MS},
    patch::NodeBody,
};
use zgui::prelude::*;

use crate::{
    decoders::{
        filter::{
            Predicate, TriState, filter_said, format_ids, kinds_offered, parse_ids, parse_words,
            sections_for, station_label,
        },
        log::kind_label,
        wiring::{event_sources_of, wired_sources_of},
    },
    store::Store,
    ui::{
        kit_decoders,
        params::entry,
        widgets::{check, pick, row_field, segments},
    },
};

type Edit = std::rc::Rc<dyn Fn(Box<dyn FnOnce(&mut EventFilterNode)>)>;

fn settings_of(store: Store, node: &str) -> EventFilterNode {
    match store.graph.get().node(node).map(|found| &found.body) {
        Some(NodeBody::EventFilter(filter)) => filter.clone(),
        _ => EventFilterNode::default(),
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_decoders::install();
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let wired = {
        let node = node.clone();
        Memo::new(move |_| !event_sources_of(&store.graph.get(), &node).is_empty())
    };
    let offered = {
        let node = node.clone();
        Memo::new(move |_| {
            kinds_offered(
                &wired_sources_of(&store.graph.get(), &node),
                &store.channel_types.get(),
            )
        })
    };
    let edit: Edit = std::rc::Rc::new(move |change| {
        let node = node.clone();
        store.edit_graph(move |graph| {
            if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
                && let NodeBody::EventFilter(filter) = &mut found.body
            {
                change(filter);
            }
        });
    });
    let hint = move || {
        let said = if !wired.get() {
            "Wire decoder events in".to_owned()
        } else if offered.with(Vec::is_empty) {
            "Nothing wired in emits events".to_owned()
        } else {
            filter_said(&settings.get())
        };
        view! { text(class = "hint") {{said}} }
    };
    let mode = Signal::derive(move || settings.get().mode);
    let set_mode = {
        let edit = edit.clone();
        move |mode: FilterMode| edit(Box::new(move |filter| filter.mode = mode))
    };
    let kinds = {
        let edit = edit.clone();
        move || kind_chips(settings, offered, edit.clone())
    };
    let sections = move || {
        let narrowed = settings.with(|s| s.kinds.clone());
        let narrowed = if narrowed.is_empty() {
            offered.get()
        } else {
            narrowed
        };
        let current = settings.get_untracked();
        sections_for(&narrowed)
            .into_iter()
            .map(|section| {
                let applies = section.applies.join(" · ");
                let rows: Vec<AnyView> = section
                    .predicates
                    .iter()
                    .map(|which| predicate(*which, &current, &narrowed, edit.clone()))
                    .collect();
                view! {
                    column(class = "dk-pane") {
                        row(class = "dk-line") {
                            text(class = "legend") {{section.title}}
                            text(class = "dk-dim") {{applies}}
                        }
                        {rows}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "face", {..kit_decoders::no_pan()}) {
            {hint}
            {row_field("Mode", segments(vec![(FilterMode::Keep, "Keep"), (FilterMode::Drop, "Drop")], mode, set_mode))}
            {kinds}
            {sections}
        }
    }
}

fn kind_chips(
    settings: Memo<EventFilterNode>,
    offered: Memo<Vec<String>>,
    edit: Edit,
) -> Option<AnyView> {
    let offered = offered.get();
    if offered.len() < 2 {
        return None;
    }
    let chips: Vec<_> = offered
        .into_iter()
        .map(|kind| {
            let held = kind.clone();
            let on = Signal::derive(move || settings.with(|s| s.kinds.contains(&held)));
            let edit = edit.clone();
            let label = kind_label(&kind);
            let toggle = move |pressed: bool| {
                let kind = kind.clone();
                edit(Box::new(move |filter| {
                    filter.kinds.retain(|held| *held != kind);
                    if pressed {
                        filter.kinds.push(kind);
                        filter.kinds.sort();
                    }
                }));
            };
            view! {
                row(class = "dk-line dk-chip") {
                    {check(on, toggle)}
                    text {{label}}
                }
            }
        })
        .collect();
    Some(AnyView::new(view! {
        column(class = "dk-pane") {
            text(class = "legend") {"Kinds"}
            row(class = "dk-line") {{chips}}
        }
    }))
}

fn words(
    name: &'static str,
    value: String,
    edit: Edit,
    apply: fn(&mut EventFilterNode, String),
) -> AnyView {
    AnyView::new(row_field(
        name,
        entry(Signal::stored(value), name.to_owned(), false, move |text| {
            edit(Box::new(move |filter| apply(filter, text)));
            Ok(())
        }),
    ))
}

fn tri(
    name: &'static str,
    value: Option<bool>,
    edit: Edit,
    apply: fn(&mut EventFilterNode, Option<bool>),
) -> AnyView {
    let options = vec![
        (TriState::Any, "Either".to_owned()),
        (TriState::Yes, "Only".to_owned()),
        (TriState::No, "Never".to_owned()),
    ];
    let chosen = Signal::stored(Some(TriState::of(value)));
    AnyView::new(row_field(
        name,
        pick(options, chosen, move |state: TriState| {
            edit(Box::new(move |filter| apply(filter, state.value())));
        }),
    ))
}

fn predicate(which: Predicate, current: &EventFilterNode, kinds: &[String], edit: Edit) -> AnyView {
    match which {
        Predicate::Stations => words(
            station_label(kinds),
            current.stations.join(", "),
            edit,
            |f, t| {
                f.stations = parse_words(&t);
            },
        ),
        Predicate::Contains => words(
            "Contains",
            current.contains.clone().unwrap_or_default(),
            edit,
            |f, t| {
                let t = t.trim();
                f.contains = (!t.is_empty()).then(|| t.to_owned());
            },
        ),
        Predicate::Talkgroups => words(
            "Talkgroups",
            format_ids(&current.talkgroups),
            edit,
            |f, t| {
                f.talkgroups = parse_ids(&t);
            },
        ),
        Predicate::Radios => words("Radios", format_ids(&current.radios), edit, |f, t| {
            f.radios = parse_ids(&t);
        }),
        Predicate::HasPosition => tri("Has position", current.has_position, edit, |f, v| {
            f.has_position = v
        }),
        Predicate::Encrypted => tri("Encrypted", current.encrypted, edit, |f, v| f.encrypted = v),
        Predicate::Emergency => tri("Emergency", current.emergency, edit, |f, v| f.emergency = v),
        Predicate::MinDuration => duration(current.min_duration_ms, edit),
    }
}

fn duration(ms: u32, edit: Edit) -> AnyView {
    let shown = format!("{:.1}", f64::from(ms) / 1000.0);
    let field = entry(
        Signal::stored(shown),
        "Longer than, seconds".to_owned(),
        false,
        move |text| {
            let seconds = text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s >= 0.0)
                .ok_or_else(|| "seconds".to_owned())?;
            let ms = ((seconds * 1000.0).round() as u32).min(MAX_FILTER_DURATION_MS);
            edit(Box::new(move |filter| filter.min_duration_ms = ms));
            Ok(())
        },
    );
    AnyView::new(row_field(
        "Longer than",
        view! { row(class = "dk-line") { {field} text(class = "dk-dim") {"s"} } },
    ))
}
