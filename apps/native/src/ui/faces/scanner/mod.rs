pub mod scan;

use std::collections::HashMap;

use sdrmm_wire::{
    patch::NodeBody,
    scan::{DEFAULT_SCAN_RANGE, ScanAction, ScanMode, ScanRequest, ScannerNode, ScannerStatus},
    state::DeviceSet,
    ws::ServerEvent,
};
use zgui::prelude::*;

use self::scan::{
    MIN_STEP_KHZ, RangeValues, check_ranges, format_db, format_mhz, live_status, sweep_kind,
    target_count,
};
use crate::{
    store::Store,
    ui::{
        faces::channel::{binding::controlled_node_of, settings::NumberLimit},
        kit_channel::{
            self, CLOSE_ICON, NumberSpec, button, group, icon, number_field, readout_row,
            setting_row, tip, toggle_row,
        },
        widgets::pick,
    },
};

const SHEET: &str = css!(
    r#"
.sc-face { flex-direction: column; }
.sc-body { flex-direction: column; gap: 8px; padding: 8px; }
.sc-readout { padding: 8px; border-top: 1px solid var(--line); }
.sc-foot { justify-content: flex-end; gap: 8px; padding: 6px 8px; border-top: 1px solid var(--line); }
"#
);

#[derive(Clone, Debug, PartialEq)]
struct Decoder {
    set: DeviceSet,
    channel: u32,
    feeds: String,
}

fn decoder_of(store: Store, node: &str) -> Option<Decoder> {
    let driven = controlled_node_of(&store.graph.get(), node)?;
    let channel = store.channel_of(&driven)?;
    let set = store.set_of(store.device_set_of(&driven)?)?;
    Some(Decoder {
        feeds: format!(
            "{} at {}",
            channel.settings.params.type_id(),
            format_mhz(Some(channel.settings.frequency_hz))
        ),
        set,
        channel: channel.id,
    })
}

fn settings_of(store: Store, node: &str) -> ScannerNode {
    match store.graph.get().node(node).map(|found| &found.body) {
        Some(NodeBody::Scanner(settings)) => settings.clone(),
        _ => ScannerNode::default(),
    }
}

fn edit_settings(store: Store, node: &str, edit: impl FnOnce(&mut ScannerNode) + 'static) {
    store.edit_node(node, move |body| {
        if let NodeBody::Scanner(settings) = body {
            edit(settings);
        }
    });
}

fn send(store: Store, decoder: (u32, u32), request: ScanRequest) {
    zgui::task::spawn_local(async move {
        let path = format!(
            "/api/devicesets/{}/channels/{}/scanner",
            decoder.0, decoder.1
        );
        if let Err(error) = store
            .api()
            .post::<_, serde::de::IgnoredAny>(&path, &request)
            .await
        {
            store.say(error.to_string());
        }
        store.refresh_state();
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("scanner-face", SHEET);
    kit_channel::install();
    let pushed = RwSignal::new(HashMap::<(u32, u32), ScannerStatus>::new());
    store.on_event(move |event| {
        if let ServerEvent::ScannerUpdate { device_set, status } = event {
            let key = (*device_set, status.settings.channel);
            let status = (**status).clone();
            pushed.update(|held| {
                held.insert(key, status);
            });
        }
    });
    let decoder = {
        let node = node.clone();
        Memo::new(move |_| decoder_of(store, &node))
    };
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let status = Memo::new(move |_| {
        let decoder = decoder.get();
        let held = pushed.get();
        let pushed = decoder
            .as_ref()
            .and_then(|found| held.get(&(found.set.id, found.channel)));
        live_status(
            decoder.as_ref().map(|found| &found.set),
            decoder.as_ref().map(|found| found.channel),
            pushed,
        )
    });
    let running = Memo::new(move |_| status.get().is_some());
    let body = {
        let node = node.clone();
        move || {
            if running.get() {
                AnyView::new(running_view(status, decoder))
            } else {
                AnyView::new(idle_view(store, node.clone(), settings, decoder))
            }
        }
    };
    view! {
        column(class = "sc-face") {
            {body}
            {footer(store, node, settings, decoder, status, pushed)}
        }
    }
}

fn running_view(
    status: Memo<Option<ScannerStatus>>,
    decoder: Memo<Option<Decoder>>,
) -> impl IntoView {
    let read = move |pick: fn(&ScannerStatus) -> String| {
        move || status.get().map(|status| pick(&status)).unwrap_or_default()
    };
    let holding = move || {
        status
            .get()
            .is_some_and(|status| status.state == sdrmm_wire::scan::ScanState::Holding)
    };
    let skipped = move || {
        status
            .get()
            .is_some_and(|status| !status.settings.skip.is_empty())
    };
    let fault = move || status.get().and_then(|status| status.error);
    view! {
        column(class = "sc-body kc-readout") {
            {readout_row("State", view! {
                text(class:kc-accent = holding) {{move || if holding() { "holding" } else { "scanning" }}}
            })}
            {readout_row("Frequency", read(|status| format_mhz(Some(status.current_hz))))}
            {readout_row("Looking for", read(|status| match status.settings.mode {
                ScanMode::CloseCall => format!("anything {} dB over the noise", status.settings.margin_db),
                _ => String::from("the listed frequencies"),
            }))}
            {readout_row("Sweep", move || {
                let status = status.get();
                sweep_kind(decoder.get().as_ref().map(|found| &found.set), status.as_ref()).to_owned()
            })}
            {readout_row("Span", read(|status| format!("{} to {}", format_mhz(Some(status.first_hz)), format_mhz(Some(status.last_hz)))))}
            {readout_row("Level", read(|status| format_db(status.current_db)))}
            {readout_row("Targets", read(|status| status.targets.to_string()))}
            {readout_row("Sweeps", read(|status| status.sweeps.to_string()))}
            {readout_row("Hits", read(|status| status.hits.to_string()))}
            {move || skipped().then(|| readout_row("Skipped", read(|status| status.settings.skip.len().to_string())))}
            {move || fault().map(|fault| readout_row("Fault", view! { text(class = "kc-danger") {{fault}} }))}
        }
    }
}

fn idle_view(
    store: Store,
    node: String,
    settings: Memo<ScannerNode>,
    decoder: Memo<Option<Decoder>>,
) -> impl IntoView {
    let count = Memo::new(move |_| settings.get().ranges.len());
    let ranges = {
        let node = node.clone();
        move || {
            (0..count.get())
                .map(|index| range_group(store, node.clone(), settings, count, index))
                .collect::<Vec<_>>()
        }
    };
    let parsed = Memo::new(move |_| check_ranges(&settings.get().ranges));
    view! {
        column(class = "sc-body") {
            column(class = "kc-grid") {
                {ranges}
                {sweep_group(store, node, settings, decoder)}
            }
            column(class = "kc-readout sc-readout") {
                {move || decoder.get().map(|found| readout_row("Feeds", found.feeds))}
                {readout_row("Sweep", move || sweep_kind(decoder.get().as_ref().map(|found| &found.set), None).to_owned())}
                {readout_row("Targets", move || match parsed.get() {
                    Ok(ranges) => AnyView::new(format!("{} per sweep", target_count(&ranges))),
                    Err(error) => AnyView::new(view! { text(class = "kc-danger") {{error}} }),
                })}
            }
        }
    }
}

fn range_field(
    store: Store,
    node: String,
    settings: Memo<ScannerNode>,
    index: usize,
    spec: NumberSpec,
    read: fn(&RangeValues) -> f64,
    write: fn(&mut RangeValues, f64),
) -> impl IntoView {
    let value = Signal::derive(move || {
        settings
            .get()
            .ranges
            .get(index)
            .map(|range| read(&RangeValues::of(range)))
    });
    number_field(value, spec, move |next| {
        let Some(next) = next else { return };
        edit_settings(store, &node, move |held| {
            if let Some(range) = held.ranges.get_mut(index) {
                let mut values = RangeValues::of(range);
                write(&mut values, next);
                *range = values.wire();
            }
        });
    })
}

fn range_group(
    store: Store,
    node: String,
    settings: Memo<ScannerNode>,
    count: Memo<usize>,
    index: usize,
) -> AnyView {
    let label = if count.get_untracked() > 1 {
        format!("Range {}", index + 1)
    } else {
        String::from("Range")
    };
    let remove = {
        let node = node.clone();
        move || {
            (count.get() > 1).then(|| {
                let node = node.clone();
                AnyView::new(view! {
                    control(
                        class = "kc-icon",
                        tabindex = Focus::Sequential,
                        a11y:role = Role::Button,
                        a11y:label = format!("Remove range {}", index + 1),
                        on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                        on:click:stop = move |_| edit_settings(store, &node, move |held| {
                            if index < held.ranges.len() {
                                held.ranges.remove(index);
                            }
                        })
                    ) {
                        {icon(CLOSE_ICON, "kc-glyph kc-glyph--sm")}
                    }
                })
            })
        }
    };
    let invalid = Signal::derive(move || {
        settings
            .get()
            .ranges
            .get(index)
            .is_some_and(|range| range.stop_hz < range.start_hz)
    });
    let mhz = |label| {
        NumberSpec::new(label)
            .limit(NumberLimit {
                min: Some(0.0),
                max: None,
                step: Some(0.1),
            })
            .unit("MHz")
    };
    group(
        label,
        AnyView::new(remove),
        view! {
            {setting_row("From", None, range_field(store, node.clone(), settings, index, mhz("Range start"), |values| values.start_mhz, |values, next| values.start_mhz = next))}
            {setting_row("To", None, view! {
                box(class:kc-danger = invalid) {
                    {range_field(store, node.clone(), settings, index, mhz("Range stop"), |values| values.stop_mhz, |values, next| values.stop_mhz = next)}
                }
            })}
            {setting_row("Step", None, range_field(
                store, node.clone(), settings, index,
                NumberSpec::new("Range step").limit(NumberLimit { min: Some(MIN_STEP_KHZ), max: None, step: Some(MIN_STEP_KHZ) }).unit("kHz"),
                |values| values.step_khz,
                |values, next| values.step_khz = next,
            ))}
        },
    )
}

fn sweep_group(
    store: Store,
    node: String,
    settings: Memo<ScannerNode>,
    decoder: Memo<Option<Decoder>>,
) -> AnyView {
    let mode = Signal::derive(move || Some(settings.get().mode));
    let pick_mode = {
        let node = node.clone();
        move |next: ScanMode| edit_settings(store, &node, move |held| held.mode = next)
    };
    let level = {
        let node = node.clone();
        move || {
            let node = node.clone();
            if settings.get().mode == ScanMode::CloseCall {
                setting_row(
                    "Over noise",
                    None,
                    number_field(
                        Signal::derive(move || Some(f64::from(settings.get().margin_db))),
                        NumberSpec::new("Close call margin")
                            .limit(NumberLimit::new(1.0, 60.0, 1.0))
                            .unit("dB"),
                        move |next| {
                            if let Some(next) = next {
                                edit_settings(store, &node, move |held| {
                                    held.margin_db = next as f32
                                });
                            }
                        },
                    ),
                )
            } else {
                setting_row(
                    "Threshold",
                    None,
                    number_field(
                        Signal::derive(move || Some(f64::from(settings.get().threshold_db))),
                        NumberSpec::new("Scan threshold")
                            .limit(NumberLimit::new(-120.0, 0.0, 1.0))
                            .unit("dB"),
                        move |next| {
                            if let Some(next) = next {
                                edit_settings(store, &node, move |held| {
                                    held.threshold_db = next as f32
                                });
                            }
                        },
                    ),
                )
            }
        }
    };
    let firmware = {
        let node = node.clone();
        move || {
            let node = node.clone();
            decoder
                .get()
                .is_some_and(|found| found.set.capabilities.hardware_sweep)
                .then(|| {
                    toggle_row(
                        "Firmware sweep",
                        Some("Let the radio sweep itself"),
                        Signal::derive(move || settings.get().hardware_sweep),
                        move |on| edit_settings(store, &node, move |held| held.hardware_sweep = on),
                    )
                })
        }
    };
    group(
        "Sweep",
        AnyView::new(()),
        view! {
            {setting_row("Looking for", None, pick(
                vec![
                    (ScanMode::Targets, String::from("the listed frequencies")),
                    (ScanMode::CloseCall, String::from("the strongest signal near me")),
                ],
                mode,
                pick_mode,
            ))}
            {level}
            {firmware}
        },
    )
}

fn footer(
    store: Store,
    node: String,
    settings: Memo<ScannerNode>,
    decoder: Memo<Option<Decoder>>,
    status: Memo<Option<ScannerStatus>>,
    pushed: RwSignal<HashMap<(u32, u32), ScannerStatus>>,
) -> impl IntoView {
    let target = move || {
        decoder
            .get_untracked()
            .map(|found| (found.set.id, found.channel))
    };
    let holding = Signal::derive(move || {
        status
            .get()
            .is_some_and(|status| status.state == sdrmm_wire::scan::ScanState::Holding)
    });
    let blocked = Signal::derive(move || {
        decoder.get().is_none() || check_ranges(&settings.get().ranges).is_err()
    });
    let add = move || {
        edit_settings(store, &node, |held| {
            let next = held.ranges.last().copied().unwrap_or(DEFAULT_SCAN_RANGE);
            held.ranges.push(next);
        });
    };
    let start = move || {
        let Some(at) = target() else { return };
        let request = ScanRequest {
            action: ScanAction::Start,
            settings: Some(settings.get_untracked().settings_for(at.1)),
        };
        send(store, at, request);
    };
    let skip = move || {
        if let Some(at) = target() {
            send(
                store,
                at,
                ScanRequest {
                    action: ScanAction::Skip,
                    settings: None,
                },
            );
        }
    };
    let stop = move || {
        if let Some(at) = target() {
            pushed.update(|held| {
                held.remove(&at);
            });
            send(
                store,
                at,
                ScanRequest {
                    action: ScanAction::Stop,
                    settings: None,
                },
            );
        }
    };
    move || {
        let add = add.clone();
        if status.get().is_some() {
            AnyView::new(view! {
                row(class = "sc-foot") {
                    {tip("Leave this frequency and never hold on it again this scan", AnyView::new(button("Skip", "kc-btn", Signal::derive(move || !holding.get()), skip)))}
                    {button("Stop scan", "kc-btn kc-btn--danger", false, stop)}
                }
            })
        } else {
            AnyView::new(view! {
                row(class = "sc-foot") {
                    {button("Add range", "kc-btn", false, add)}
                    {tip("Wire this node's control out to a decoder", AnyView::new(button("Start scan", "kc-btn kc-btn--primary", blocked, start)))}
                }
            })
        }
    }
}
