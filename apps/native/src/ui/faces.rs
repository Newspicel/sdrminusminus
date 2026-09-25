use std::sync::Arc;

use sdrmm_wire::{
    audio::{
        AudioAgcMode, MAX_BLANKER_THRESHOLD, MAX_CLICK_THRESHOLD, MIN_BLANKER_THRESHOLD,
        MIN_CLICK_THRESHOLD,
    },
    channel::{
        ChannelInfo, ChannelSettings, MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB,
        Squelch,
    },
    device::{DeviceInfo, DeviceSettings},
    patch::{DeviceRef, NodeBody, PatchNode},
    state::{DeviceSet, DeviceSetStatus},
};
use zgui::prelude::*;

use crate::{
    binding, format,
    socket::Spectrum,
    store::Store,
    ui::{
        gpu,
        scope::{self, Palette},
        widgets::{check, dial, gated_slide, level_bar, meter, pick, row_field, segments, slide},
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SquelchMode {
    Off,
    Manual,
    Auto,
}

pub fn face(store: Store, node: &PatchNode) -> AnyView {
    match &node.body {
        NodeBody::Device(_) => AnyView::new(device(store, node.id.clone())),
        NodeBody::Channel(_) => AnyView::new(channel(store, node.id.clone())),
        NodeBody::Scope => AnyView::new(scope_face(store, node.id.clone())),
        NodeBody::Speaker => AnyView::new(speaker(store, node.id.clone())),
        NodeBody::DecoderLog => AnyView::new(decoder_log(store)),
        body => AnyView::new(plain(body.kind())),
    }
}

pub fn status_of(store: Store, node: &str) -> (&'static str, &'static str) {
    if !carries_status(store, node) {
        return ("idle", "");
    }
    let Some(set) = store.device_set_of(node) else {
        return ("idle", "UNBOUND");
    };
    match store.set_of(set).map(|set| set.status) {
        Some(DeviceSetStatus::Running) => ("run", "RUNNING"),
        Some(DeviceSetStatus::Error) => ("err", "ERROR"),
        _ => ("idle", "IDLE"),
    }
}

fn carries_status(store: Store, node: &str) -> bool {
    store.graph.get().nodes.iter().any(|found| {
        found.id == node && matches!(found.body, NodeBody::Device(_) | NodeBody::Channel(_))
    })
}

fn plain(kind: &str) -> impl IntoView {
    let kind = kind.replace('_', " ");
    view! {
        column(class = "face") {
            text(class = "hint") {{kind}}
        }
    }
}

fn set_signal(store: Store, node: String) -> Signal<Option<DeviceSet>> {
    Signal::derive(move || store.device_set_of(&node).and_then(|id| store.set_of(id)))
}

fn device(store: Store, node: String) -> impl IntoView {
    let set = set_signal(store, node.clone());
    let hz = Signal::derive(move || {
        set.get()
            .and_then(|set| set.settings.center_hz)
            .unwrap_or_default()
    });
    let tune = {
        let node = node.clone();
        move |value: f64| store.tune_device(node.clone(), value)
    };

    let rates = Signal::derive(move || {
        set.get()
            .map(|set| set.capabilities.sample_rates.clone())
            .unwrap_or_default()
    });
    let chosen_rate = Signal::derive(move || set.get().and_then(|set| set.settings.sample_rate));
    let pick_rate = move |value: f64| {
        if let Some(id) = set.get_untracked().map(|set| set.id) {
            store.set_device(
                id,
                DeviceSettings {
                    sample_rate: Some(value),
                    ..DeviceSettings::default()
                },
            );
        }
    };

    let radios = Signal::derive(move || (*store.devices.get()).clone());
    let chosen_radio = {
        let node = node.clone();
        Signal::derive(move || {
            let graph = store.graph.get();
            graph.nodes.iter().find_map(|found| match &found.body {
                NodeBody::Device(device) if found.id == node => {
                    device.device.as_ref().map(|reference| {
                        reference.backend.clone()
                            + ":"
                            + reference.key.as_deref().unwrap_or_default()
                    })
                }
                _ => None,
            })
        })
    };
    let pick_radio = {
        let node = node.clone();
        move |key: String| {
            let Some(info) = store
                .devices
                .get_untracked()
                .iter()
                .find(|info| device_key(info) == key)
                .cloned()
            else {
                return;
            };
            let node = node.clone();
            store.edit_graph(move |graph| {
                if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
                    && let NodeBody::Device(device) = &mut found.body
                {
                    device.device = Some(DeviceRef::from_info(&info));
                }
            });
        }
    };

    let dc = Signal::derive(move || {
        set.get()
            .and_then(|set| set.settings.dc_block)
            .unwrap_or(false)
    });
    let toggle_dc = move |on: bool| {
        if let Some(id) = set.get_untracked().map(|set| set.id) {
            store.set_device(
                id,
                DeviceSettings {
                    dc_block: Some(on),
                    ..DeviceSettings::default()
                },
            );
        }
    };

    view! {
        column(class = "face") {
            {dial(hz, tune)}
            {move || {
                let options = radios
                    .get()
                    .iter()
                    .map(|info| (device_key(info), info.label.clone()))
                    .collect::<Vec<_>>();
                row_field("Radio", pick(options, chosen_radio, pick_radio.clone()))
            }}
            {move || {
                let options = rates
                    .get()
                    .iter()
                    .map(|rate| (*rate, format::rate(*rate)))
                    .collect::<Vec<_>>();
                row_field("Rate", pick(options, chosen_rate, pick_rate))
            }}
            {row_field("DC block", check(dc, toggle_dc))}
        }
    }
}

fn device_key(info: &DeviceInfo) -> String {
    format!("{}:{}", info.driver, info.key)
}

fn channel_signal(store: Store, node: String) -> Signal<Option<ChannelInfo>> {
    Signal::derive(move || store.channel_of(&node))
}

type ChannelEdit = Box<dyn FnOnce(&mut ChannelSettings)>;

fn channel_writer(
    store: Store,
    node: String,
    channel: Signal<Option<ChannelInfo>>,
) -> impl Fn(ChannelEdit) + Clone {
    move |change| {
        if let Some(mut settings) = channel.get_untracked().map(|channel| channel.settings) {
            change(&mut settings);
            store.set_channel(node.clone(), settings);
        }
    }
}

fn channel(store: Store, node: String) -> impl IntoView {
    let decoder_controls =
        store
            .graph
            .get_untracked()
            .node(&node)
            .and_then(|node| match &node.body {
                NodeBody::Channel(channel) => Some(super::params::panel(
                    store,
                    node.id.clone(),
                    &channel.channel_type,
                )),
                _ => None,
            });
    let channel = channel_signal(store, node.clone());
    let hz = Signal::derive(move || {
        channel
            .get()
            .map(|channel| channel.settings.frequency_hz)
            .unwrap_or_default()
    });
    let write = channel_writer(store, node.clone(), channel);

    let tune = {
        let write = write.clone();
        move |value: f64| write(Box::new(move |settings| settings.frequency_hz = value))
    };

    let mode =
        Signal::derive(
            move || match channel.get().map(|channel| channel.settings.squelch) {
                Some(Squelch::Manual { .. }) => SquelchMode::Manual,
                Some(Squelch::Auto { .. }) => SquelchMode::Auto,
                _ => SquelchMode::Off,
            },
        );
    let pick_mode = {
        let write = write.clone();
        move |mode: SquelchMode| {
            let squelch = match mode {
                SquelchMode::Off => Squelch::Off,
                SquelchMode::Manual => Squelch::Manual { level_db: -60.0 },
                SquelchMode::Auto => Squelch::Auto { margin_db: 6.0 },
            };
            write(Box::new(move |settings| settings.squelch = squelch));
        }
    };

    let level = Signal::derive(move || {
        channel
            .get()
            .and_then(|channel| match channel.settings.squelch {
                Squelch::Manual { level_db } => Some(f64::from(level_db)),
                Squelch::Auto { margin_db } => Some(f64::from(margin_db)),
                Squelch::Off => None,
            })
            .unwrap_or(-60.0)
    });
    let set_level = {
        let write = write.clone();
        move |value: f64| {
            write(Box::new(move |settings| {
                settings.squelch = match settings.squelch {
                    Squelch::Auto { .. } => Squelch::Auto {
                        margin_db: value as f32,
                    },
                    _ => Squelch::Manual {
                        level_db: value as f32,
                    },
                };
            }));
        }
    };

    let strength = {
        let node = node.clone();
        Signal::derive(move || {
            let Some(set) = store.device_set_of(&node) else {
                return -120.0;
            };
            let Some(channel) = store.channel_of(&node) else {
                return -120.0;
            };
            store
                .levels
                .get()
                .get(&(set, channel.id))
                .map_or(-120.0, |level| level.level_db)
        })
    };

    view! {
        column(class = "face") {
            {dial(hz, tune)}
            {level_bar(strength)}
            {decoder_controls}
            {row_field("Squelch", view! {
                row(class = "field__body") {
                    {segments(
                        vec![
                            (SquelchMode::Off, "Off"),
                            (SquelchMode::Manual, "Manual"),
                            (SquelchMode::Auto, "Auto"),
                        ],
                        mode,
                        pick_mode,
                    )}
                    {move || (mode.get() != SquelchMode::Off).then(|| AnyView::new(
                        slide(level, if mode.get() == SquelchMode::Auto { MIN_SQUELCH_AUTO_MARGIN_DB.into() } else { -120.0 }, if mode.get() == SquelchMode::Auto { MAX_SQUELCH_AUTO_MARGIN_DB.into() } else { 0.0 }, |value| format::decibels(value as f32), set_level.clone()),
                    ))}
                }
            })}
        }
    }
}

fn spectrum_signal(store: Store, node: String) -> Signal<Option<Arc<Spectrum>>> {
    Signal::derive(move || {
        let set = store.device_set_of(&node)?;
        store.spectra.get().get(&set).cloned()
    })
}

fn scope_face(store: Store, node: String) -> impl IntoView {
    let spectrum = spectrum_signal(store, node.clone());
    let palette = RwSignal::new(Palette::Viridis);

    let watching = {
        let node = node.clone();
        zgui::reactive::RenderEffect::new(move |_| {
            if let Some(set) = store.device_set_of(&node) {
                store.watch_spectrum(set);
            }
        })
    };
    on_cleanup_local(move || drop(watching));

    let marks = {
        let node = node.clone();
        Signal::derive(move || {
            let Some(spectrum) = spectrum.get() else {
                return Vec::new();
            };
            let Some(set) = store.device_set_of(&node).and_then(|id| store.set_of(id)) else {
                return Vec::new();
            };
            let channels: Vec<(String, f64)> = set
                .channels
                .iter()
                .map(|channel| {
                    (
                        channel.settings.params.type_id().to_uppercase(),
                        channel.settings.frequency_hz,
                    )
                })
                .collect();
            scope::markers(spectrum.center_hz, f64::from(spectrum.span_hz), &channels)
        })
    };
    let mark_labels = move || {
        marks
            .get()
            .into_iter()
            .map(|mark| {
                view! {
                    box(
                        class = "scope__mark",
                        style:left = Some(format!("{}%", mark.at * 100.0))
                    ) {
                        text(class = "scope__mark_text") {{mark.label}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };

    let strip = move || {
        spectrum.get().map_or_else(
            || String::from("waiting for the radio"),
            |spectrum| scope::readout(&spectrum),
        )
    };
    let db_ticks = move || {
        let (db_min, db_max) = spectrum.get().map_or((-120.0, -20.0), |spectrum| {
            (spectrum.db_min, spectrum.db_max)
        });
        scope::decibel_ticks(db_min, db_max)
            .into_iter()
            .map(|label| view! { text(class = "scope__read") {{label}} })
            .collect::<Vec<_>>()
    };
    let hz_ticks = move || {
        let Some(spectrum) = spectrum.get() else {
            return Vec::new();
        };
        scope::frequency_ticks(spectrum.center_hz, f64::from(spectrum.span_hz), 9)
            .into_iter()
            .map(|label| view! { text(class = "scope__read") {{label}} })
            .collect::<Vec<_>>()
    };

    view! {
        column(class = "scope") {
            box(class = "scope__plot") {
                {scope::trace(spectrum, marks)}
                column(class = "scope__db") {{db_ticks}}
                {mark_labels}
            }
            row(class = "scope__axis") {{hz_ticks}}
            {zgui::elements::surface()
                .class("scope__fall")
                .renderer(gpu::WaterfallSurface::new(spectrum, palette.into()))
                .into_view()}
            row(class = "scope__strip") {
                {segments(
                    vec![
                        (Palette::Viridis, Palette::Viridis.label()),
                        (Palette::Classic, Palette::Classic.label()),
                    ],
                    palette.into(),
                    move |chosen| palette.set(chosen),
                )}
                spacer()
                text(class = "scope__read") {{strip}}
            }
        }
    }
}

fn speaker(store: Store, node: String) -> impl IntoView {
    let wired = Signal::derive(move || {
        let graph = store.graph.get();
        binding::sources_of(&graph, &node, "audio")
    });
    let names = move || {
        let sources = wired.get();
        if sources.is_empty() {
            String::from("nothing wired")
        } else {
            sources.join(", ")
        }
    };
    let strength = Signal::derive(move || {
        let graph = store.graph.get();
        let state = store.state.get();
        let devices = binding::device_sets(&graph, &state.device_sets);
        let channels = binding::channels(&graph, &state.device_sets, &devices);
        let levels = store.levels.get();
        wired
            .get()
            .iter()
            .filter_map(|source| {
                let channel = channels.get(source)?;
                let owner = binding::device_node_of(&graph, source)?;
                let set = devices.get(&owner)?;
                levels.get(&(*set, channel.id)).map(|level| level.level_db)
            })
            .fold(-120.0f32, f32::max)
    });

    view! {
        column(class = "face") {
            {row_field("Source", view! { text(class = "mono") {{names}} })}
            {row_field("Level", meter(strength))}
        }
    }
}

fn decoder_log(store: Store) -> impl IntoView {
    let rows = move || {
        let decoded = store.decoded.get();
        decoded
            .iter()
            .rev()
            .take(14)
            .map(|record| {
                let when = format::clock(&record.at);
                let what = format!("{} {}", record.event.kind(), record.event.summary());
                view! {
                    row(class = "log__row") {
                        text(class = "log__when") {{when}}
                        text(class = "log__what") {{what}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "face") {
            column(class = "log") {{rows}}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radio_is_named_by_its_driver_and_key() {
        let info = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "siggen".to_owned(),
            label: "Signal Generator (virtual)".to_owned(),
            serial: None,
            profile: None,
        };
        assert_eq!(device_key(&info), "virtual:siggen");
    }
}
