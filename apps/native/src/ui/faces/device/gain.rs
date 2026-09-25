use sdrmm_wire::{
    device::{DeviceSettings, GainStage, GainValue, StreamSettings},
    state::DeviceSet,
};
use zgui::prelude::*;

use super::{
    caps::{
        automatic_gain_is_on, format_gain, gain_label, gain_unit, setting_index, snap_to_stage,
        stage_settings,
    },
    lanes::{agc_delta, agc_gain_db, agc_tip, lane_agc, rx_stream_count, stream_label},
    radio::Radio,
};
use crate::ui::{
    kit_sources::debounced,
    widgets::{check, pick, row_field, slide},
};

const AGC_HINT: &str = "The radio is setting this. Turn AGC off to set it by hand";

#[derive(Clone, Copy)]
pub struct Lane {
    pub stream: Option<u32>,
    pub advised: bool,
}

impl Lane {
    pub const WHOLE: Self = Self {
        stream: None,
        advised: false,
    };

    fn index(self) -> u32 {
        self.stream.unwrap_or(0)
    }

    fn resolved(self, set: &DeviceSet) -> DeviceSettings {
        match self.stream {
            Some(stream) => set
                .settings
                .for_stream(stream, &set.capabilities.per_stream),
            None => set.settings.clone(),
        }
    }

    fn automatic(self, set: &DeviceSet) -> bool {
        match self.stream {
            Some(stream) => lane_agc(set, stream).on,
            None => automatic_gain_is_on(&set.capabilities, &set.settings),
        }
    }

    fn gain_delta(self, stage: &str, value_db: f64) -> DeviceSettings {
        let gains = vec![GainValue {
            stage: stage.to_owned(),
            value_db,
        }];
        match self.stream {
            Some(stream) => DeviceSettings {
                streams: vec![StreamSettings {
                    stream,
                    gains,
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
            None => DeviceSettings {
                gains,
                ..DeviceSettings::default()
            },
        }
    }
}

pub fn agc_auto(radio: Radio, lane: Lane) -> impl IntoView {
    let stream = lane.index();
    let on = Signal::derive(move || radio.read(|set| lane_agc(set, stream).on).unwrap_or(false));
    let tip = Signal::derive(move || {
        radio
            .read(|set| agc_tip(set, stream, lane.advised))
            .unwrap_or_default()
    });
    view! {
        row(class = "kit-auto", attr:data-tip = move || Some(tip.get())) {
            {check(on, move |next| {
                let delta = radio.read(|set| {
                    let mut agc = lane_agc(set, stream);
                    agc.on = next;
                    agc_delta(&set.capabilities, stream, agc)
                });
                if let Some(delta) = delta {
                    radio.patch(delta);
                }
            })}
            text(class = "kit-legend", class:warn = move || on.get() && lane.advised) {"Auto"}
        }
    }
}

pub fn gain_control(radio: Radio, stage: GainStage, lane: Lane, agc: bool) -> impl IntoView {
    let name = stage.name.clone();
    let value = {
        let name = name.clone();
        let min = stage.range.min;
        Signal::derive(move || {
            radio
                .read(|set| lane.resolved(set).gain(&name))
                .flatten()
                .unwrap_or(min)
        })
    };
    let disabled = Signal::derive(move || radio.read(|set| lane.automatic(set)).unwrap_or(false));
    let measured =
        Signal::derive(move || radio.read(|set| agc_gain_db(set, lane.index())).flatten());
    let commit = {
        let name = name.clone();
        move |db: f64| radio.patch(lane.gain_delta(&name, db))
    };
    let (pending, change) = debounced(commit.clone());
    let shown = Signal::derive(move || {
        disabled
            .get()
            .then(|| measured.get())
            .flatten()
            .or_else(|| pending.get())
            .unwrap_or_else(|| value.get())
    });
    let label = gain_label(&stage);
    let title = if gain_unit(stage.unit).is_empty() {
        "Firmware step, not dB"
    } else {
        ""
    };
    let tip = Signal::derive(move || {
        Some(if disabled.get() {
            AGC_HINT.to_owned()
        } else {
            title.to_owned()
        })
    });
    let auto = agc.then(|| AnyView::new(agc_auto(radio, lane)));
    let body = if stage.is_switch() {
        AnyView::new(switch_stage(stage, shown, disabled, commit))
    } else {
        AnyView::new(stepped_stage(stage, shown, disabled, change))
    };
    view! {
        box(class = "kit-gain", attr:data-tip = tip) {
            {row_field(label, view! { row(class = "field__body") { {auto} {body} } })}
        }
    }
}

fn switch_stage(
    stage: GainStage,
    shown: Signal<f64>,
    disabled: Signal<bool>,
    commit: impl Fn(f64) + 'static,
) -> impl IntoView {
    let (min, max) = (stage.range.min, stage.range.max);
    let on = Signal::derive(move || shown.get() > min);
    view! {
        row(class = "field__body", class:kit-muted = disabled) {
            {check(on, move |next| commit(if next { max } else { min }))}
            text(class = "kit-read-out") {{move || if on.get() { format!("+{max:.0} dB") } else { "0 dB".to_owned() }}}
        }
    }
}

fn stepped_stage(
    stage: GainStage,
    shown: Signal<f64>,
    disabled: Signal<bool>,
    change: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let settings = stage_settings(&stage);
    let unit = stage.unit;
    let read = move |db: f64| {
        let symbol = gain_unit(unit);
        if symbol.is_empty() {
            format_gain(unit, db)
        } else {
            format!("{} {symbol}", format_gain(unit, db))
        }
    };
    let slider = if settings.is_empty() {
        let snap = stage.clone();
        AnyView::new(slide(
            shown,
            stage.range.min,
            stage.range.max,
            read,
            move |db| {
                change(snap_to_stage(&snap, db));
            },
        ))
    } else {
        let top = settings.len().saturating_sub(1) as f64;
        let table = settings.clone();
        let index = Signal::derive(move || setting_index(&table, shown.get()) as f64);
        let lookup = settings.clone();
        let shown_db = move |at: f64| {
            let at = at.round().clamp(0.0, top) as usize;
            read(lookup.get(at).copied().unwrap_or_default())
        };
        AnyView::new(slide(index, 0.0, top, shown_db, move |at| {
            let at = at.round().clamp(0.0, top) as usize;
            if let Some(db) = settings.get(at) {
                change(*db);
            }
        }))
    };
    view! { box(class = "kit-slot", class:kit-muted = disabled) {{slider}} }
}

pub fn lane_controls(radio: Radio, stream: u32, advised: bool) -> impl IntoView {
    let lane = Lane {
        stream: Some(stream),
        advised,
    };
    let Some((caps, port)) = radio.read_untracked(|set| {
        let caps = set.capabilities.clone();
        let port = stream_label("iq", stream, rx_stream_count(&caps));
        (caps, port)
    }) else {
        return AnyView::new(());
    };
    let antenna = (caps.per_stream.antenna && caps.antennas.len() > 1).then(|| {
        let options: Vec<(String, String)> = caps
            .antennas
            .iter()
            .map(|a| (a.clone(), a.clone()))
            .collect();
        let first = caps.antennas.first().cloned();
        let chosen = Signal::derive(move || {
            radio
                .read(|set| lane.resolved(set).antenna)
                .flatten()
                .or_else(|| first.clone())
        });
        AnyView::new(row_field(
            format!("{port} antenna"),
            pick(options, chosen, move |antenna| {
                radio.patch(DeviceSettings {
                    streams: vec![StreamSettings {
                        stream,
                        antenna: Some(antenna),
                        ..StreamSettings::default()
                    }],
                    ..DeviceSettings::default()
                });
            }),
        ))
    });
    let agc_here = caps.agc.offered() && (caps.per_stream.agc || stream == 0);
    let gains: Vec<AnyView> = if caps.per_stream.gain {
        caps.gains
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                AnyView::new(gain_control(
                    radio,
                    stage.clone(),
                    lane,
                    agc_here && index == 0,
                ))
            })
            .collect()
    } else {
        Vec::new()
    };
    AnyView::new(view! { column(class = "kit-lane") { {antenna} {gains} } })
}
