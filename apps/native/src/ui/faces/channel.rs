#[allow(unused_imports)]
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SquelchMode {
    Off,
    Manual,
    Auto,
}

pub fn face(store: Store, node: String) -> impl IntoView {
    let decoder_controls =
        store
            .graph
            .get_untracked()
            .node(&node)
            .and_then(|node| match &node.body {
                NodeBody::Channel(channel) => Some(crate::ui::params::panel(
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
