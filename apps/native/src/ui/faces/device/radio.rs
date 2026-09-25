use std::collections::BTreeSet;

use sdrmm_wire::{
    device::{
        AgcSetting, BandwidthSetting, Capabilities, DeviceSettings, ExtraSetting, ExtraValue,
    },
    state::DeviceSet,
};
use zgui::prelude::*;

use super::{
    caps::{
        agc_state, automatic_gain_is_on, dc_block_on, filter_hz, filter_is_auto, fits_slider,
        has_dc_artifact, has_filter, snap_to_ranges, span_of,
    },
    gain::{Lane, gain_control, lane_controls},
    lanes::{rx_stream_count, stream_label},
    patch,
};
use crate::{
    store::Store,
    ui::{
        kit_sources::{
            NumberSpec, debounced,
            dial::{is_tunable, tuning_range},
            group, number_field, number_field_in, text_field, units,
        },
        widgets::{check, pick, row_field, slide},
    },
};

pub const LOOP_SETTING: &str = "loop";

#[derive(Clone, Copy)]
pub struct Radio {
    pub store: Store,
    pub set: Signal<Option<DeviceSet>>,
}

impl Radio {
    pub fn patch(self, delta: DeviceSettings) {
        if let Some(id) = self
            .set
            .with_untracked(|set| set.as_ref().map(|set| set.id))
        {
            patch::apply(self.store, id, delta);
        }
    }

    pub fn read<T>(self, read: impl FnOnce(&DeviceSet) -> T) -> Option<T> {
        self.set.with(|set| set.as_ref().map(read))
    }

    pub fn read_untracked<T>(self, read: impl FnOnce(&DeviceSet) -> T) -> Option<T> {
        self.set.with_untracked(|set| set.as_ref().map(read))
    }

    fn settings<T: Send + Sync + Clone + 'static>(
        self,
        read: impl Fn(&DeviceSettings) -> T + Send + Sync + 'static,
        fallback: T,
    ) -> Signal<T> {
        Signal::derive(move || {
            self.read(|set| read(&set.settings))
                .unwrap_or_else(|| fallback.clone())
        })
    }
}

#[derive(Clone, PartialEq)]
struct Shape {
    caps: Capabilities,
    playing: bool,
}

pub fn radio_settings(
    radio: Radio,
    lanes_shown: bool,
    advised: Signal<BTreeSet<u32>>,
) -> impl IntoView {
    let shape = Memo::new(move |_| {
        radio.read(|set| Shape {
            caps: set.capabilities.clone(),
            playing: set.playback.is_some(),
        })
    });
    move || {
        shape.get().map(|shape| {
            AnyView::new(view! {
                column(class = "kit-radio") {
                    {rows(radio, &shape, lanes_shown, advised)}
                }
            })
        })
    }
}

fn rows(
    radio: Radio,
    shape: &Shape,
    lanes_shown: bool,
    advised: Signal<BTreeSet<u32>>,
) -> Vec<AnyView> {
    let caps = &shape.caps;
    let streamed_antenna = caps.per_stream.antenna && caps.antennas.len() > 1;
    let streamed_gain = caps.per_stream.gain && !caps.gains.is_empty();
    let agc_on_gain = caps.agc.offered() && !caps.gains.is_empty();
    let mut rows = vec![AnyView::new(row_field("Rate", rate_control(radio, caps)))];
    if has_filter(caps) {
        rows.push(AnyView::new(row_field(
            "Filter",
            filter_control(radio, caps),
        )));
    }
    if caps.antennas.len() > 1 && !streamed_antenna {
        rows.push(AnyView::new(antenna_row(radio, caps)));
    }
    if caps.agc.offered() {
        rows.push(AnyView::new(agc_row(radio, caps, agc_on_gain)));
    }
    if !streamed_gain {
        let advised_first = advised.with_untracked(|lanes| lanes.contains(&0));
        for (index, stage) in caps.gains.iter().enumerate() {
            let lane = Lane {
                advised: advised_first,
                ..Lane::WHOLE
            };
            rows.push(AnyView::new(gain_control(
                radio,
                stage.clone(),
                lane,
                agc_on_gain && index == 0,
            )));
        }
    }
    if !lanes_shown && (streamed_antenna || streamed_gain) {
        let streams = rx_stream_count(caps);
        for stream in 0..streams {
            let advised = advised.with_untracked(|lanes| lanes.contains(&stream));
            rows.push(AnyView::new(group(
                stream_label("iq", stream, streams),
                lane_controls(radio, stream, advised),
            )));
        }
    }
    rows.extend(switches(radio, caps));
    let extras = caps
        .extra
        .iter()
        .filter(|setting| !shape.playing || setting.name() != LOOP_SETTING)
        .map(|setting| AnyView::new(extra_control(radio, setting.clone())));
    rows.extend(extras);
    rows
}

fn switches(radio: Radio, caps: &Capabilities) -> Vec<AnyView> {
    let mut rows = Vec::new();
    if caps.bias_tee {
        let on = radio.settings(|s| s.bias_tee.unwrap_or(false), false);
        rows.push(AnyView::new(row_field(
            "Bias tee",
            check(on, move |bias_tee| {
                radio.patch(DeviceSettings {
                    bias_tee: Some(bias_tee),
                    ..DeviceSettings::default()
                });
            }),
        )));
    }
    if caps.ppm {
        let ppm = radio.settings(|s| s.ppm.unwrap_or(0.0), 0.0);
        rows.push(AnyView::new(row_field(
            "PPM",
            number_field(
                "Frequency correction",
                ppm,
                NumberSpec::unit("ppm").step(1.0),
                Signal::stored(false),
                move |ppm| {
                    radio.patch(DeviceSettings {
                        ppm: Some(ppm),
                        ..DeviceSettings::default()
                    });
                },
            ),
        )));
    }
    if is_tunable(tuning_range(caps)) {
        let offset = radio.settings(|s| s.offset_hz.unwrap_or(0.0) / 1e6, 0.0);
        rows.push(AnyView::new(row_field(
            "Converter",
            number_field(
                "Converter offset",
                offset,
                NumberSpec::unit("MHz").step(0.001),
                Signal::stored(false),
                move |mhz| {
                    radio.patch(DeviceSettings {
                        offset_hz: Some((mhz * 1e6).round()),
                        ..DeviceSettings::default()
                    });
                },
            ),
        )));
    }
    if has_dc_artifact(caps) {
        let dc = Signal::derive(move || {
            radio
                .read(|set| dc_block_on(&set.capabilities, &set.settings))
                .unwrap_or(false)
        });
        rows.push(AnyView::new(row_field(
            "DC block",
            check(dc, move |dc_block| {
                radio.patch(DeviceSettings {
                    dc_block: Some(dc_block),
                    ..DeviceSettings::default()
                });
            }),
        )));
    }
    rows
}

fn with_current(
    current: f64,
    mut options: Vec<(f64, String)>,
    show: fn(f64) -> String,
) -> Vec<(f64, String)> {
    if current > 0.0 && !options.iter().any(|(value, _)| *value == current) {
        options.insert(0, (current, show(current)));
    }
    options
}

fn rate_control(radio: Radio, caps: &Capabilities) -> AnyView {
    let rate = radio.settings(|s| s.sample_rate.unwrap_or(0.0), 0.0);
    let span = span_of(&caps.sample_rate_ranges);
    let commit = move |sample_rate: f64| {
        radio.patch(DeviceSettings {
            sample_rate: Some(sample_rate),
            ..DeviceSettings::default()
        });
    };
    if caps.sample_rates.len() == 1 && span.is_none() {
        return AnyView::new(
            view! { text(class = "kit-mono") {{move || units::sample_rate(rate.get())}} },
        );
    }
    if !caps.sample_rates.is_empty() {
        let menu: Vec<(f64, String)> = caps
            .sample_rates
            .iter()
            .map(|r| (*r, units::sample_rate(*r)))
            .collect();
        let options =
            Memo::new(move |_| with_current(rate.get(), menu.clone(), units::sample_rate));
        let chosen = Signal::derive(move || Some(rate.get()));
        return AnyView::new(move || pick(options.get(), chosen, commit));
    }
    let ranges = caps.sample_rate_ranges.clone();
    let mut spec =
        NumberSpec::unit("MS/s").step(span.and_then(|s| s.step).map_or(0.001, |step| step / 1e6));
    if let Some(span) = span {
        spec = spec.within(span.min / 1e6, span.max / 1e6);
    }
    let shown = Signal::derive(move || rate.get() / 1e6);
    AnyView::new(number_field(
        "Sample rate",
        shown,
        spec,
        Signal::stored(false),
        move |msps| {
            commit(snap_to_ranges(&ranges, (msps * 1e6).round()));
        },
    ))
}

fn filter_control(radio: Radio, caps: &Capabilities) -> AnyView {
    let auto = radio.settings(filter_is_auto, false);
    let hz = Signal::derive(move || {
        radio
            .read(|set| filter_hz(&set.capabilities, &set.settings))
            .unwrap_or(0.0)
    });
    let commit = move |bandwidth: BandwidthSetting| {
        radio.patch(DeviceSettings {
            bandwidth: Some(bandwidth),
            ..DeviceSettings::default()
        });
    };
    let toggle = caps.bandwidth_auto.then(|| AnyView::new(view! {
        row(class = "kit-auto") {
            {check(auto, move |on| commit(if on { BandwidthSetting::Auto } else { BandwidthSetting::Manual { hz: hz.get_untracked() } }))}
            text(class = "kit-legend") {"Auto"}
        }
    }));
    let width = if caps.bandwidths.is_empty() {
        span_of(&caps.bandwidth_ranges).map(|span| {
            let ranges = caps.bandwidth_ranges.clone();
            let shown = Signal::derive(move || hz.get() / 1e6);
            let spec = NumberSpec::unit("MHz")
                .within(span.min / 1e6, span.max / 1e6)
                .step(0.01);
            AnyView::new(number_field(
                "Analog bandwidth",
                shown,
                spec,
                auto,
                move |mhz| {
                    commit(BandwidthSetting::Manual {
                        hz: snap_to_ranges(&ranges, (mhz * 1e6).round()),
                    });
                },
            ))
        })
    } else {
        let menu: Vec<(f64, String)> = caps
            .bandwidths
            .iter()
            .map(|w| (*w, units::hz(*w)))
            .collect();
        let options = Memo::new(move |_| with_current(hz.get(), menu.clone(), units::hz));
        let chosen = Signal::derive(move || Some(hz.get()));
        Some(AnyView::new(view! {
            box(class = "kit-slot", class:kit-muted = auto) {
                {move || pick(options.get(), chosen, move |hz| commit(BandwidthSetting::Manual { hz }))}
            }
        }))
    };
    AnyView::new(view! { row(class = "field__body") { {toggle} {width} } })
}

fn antenna_row(radio: Radio, caps: &Capabilities) -> impl IntoView {
    let options: Vec<(String, String)> = caps
        .antennas
        .iter()
        .map(|a| (a.clone(), a.clone()))
        .collect();
    let first = caps.antennas.first().cloned();
    let chosen = Signal::derive(move || {
        radio
            .read(|set| set.settings.antenna.clone())
            .flatten()
            .or_else(|| first.clone())
    });
    row_field(
        "Antenna",
        pick(options, chosen, move |antenna| {
            radio.patch(DeviceSettings {
                antenna: Some(antenna),
                ..DeviceSettings::default()
            });
        }),
    )
}

fn agc_row(radio: Radio, caps: &Capabilities, agc_on_gain: bool) -> impl IntoView {
    let state = Signal::derive(move || {
        radio
            .read(|set| agc_state(&set.capabilities, &set.settings))
            .unwrap_or_else(AgcSetting::off)
    });
    let on = Signal::derive(move || state.get().on);
    let modes: Vec<(String, String)> = match &caps.agc {
        sdrmm_wire::device::Agc::Modes { options } => options
            .iter()
            .map(|option| {
                (
                    option.value.clone(),
                    option.label.clone().unwrap_or_else(|| option.value.clone()),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let has_modes = !modes.is_empty();
    let shown = Signal::derive(move || {
        let automatic = radio
            .read(|set| automatic_gain_is_on(&set.capabilities, &set.settings))
            .unwrap_or(false);
        !agc_on_gain || (has_modes && automatic)
    });
    let commit = move |agc: AgcSetting| {
        radio.patch(DeviceSettings {
            agc: Some(agc),
            ..DeviceSettings::default()
        })
    };
    let toggle = (!agc_on_gain).then(|| {
        AnyView::new(check(on, move |next| {
            commit(AgcSetting {
                on: next,
                ..state.get_untracked()
            })
        }))
    });
    let menu = has_modes.then(|| {
        let chosen = Signal::derive(move || state.get().mode);
        AnyView::new(view! {
            box(class = "kit-slot", class:kit-muted = move || !on.get()) {
                {pick(modes, chosen, move |mode| commit(AgcSetting::in_mode(true, mode)))}
            }
        })
    });
    view! {
        box(style:display = move || Some(if shown.get() { "block" } else { "none" }.to_owned())) {
            {row_field("AGC", view! { row(class = "field__body") { {toggle} {menu} } })}
        }
    }
}

fn extra_raw(radio: Radio, name: String) -> Signal<Option<serde_json::Value>> {
    Signal::derive(move || {
        radio
            .read(|set| {
                set.settings
                    .extra
                    .iter()
                    .find(|extra| extra.name == name)
                    .map(|extra| extra.value.clone())
            })
            .flatten()
    })
}

fn extra_patch(radio: Radio, name: String) -> impl Fn(serde_json::Value) + Clone + 'static {
    move |value| {
        radio.patch(DeviceSettings {
            extra: vec![ExtraValue {
                name: name.clone(),
                value,
            }],
            ..DeviceSettings::default()
        });
    }
}

fn extra_control(radio: Radio, setting: ExtraSetting) -> AnyView {
    let name = setting
        .label()
        .map_or_else(|| units::setting_label(setting.name()), str::to_owned);
    let raw = extra_raw(radio, setting.name().to_owned());
    let commit = extra_patch(radio, setting.name().to_owned());
    match setting {
        ExtraSetting::Bool { default, .. } => {
            let on = Signal::derive(move || raw.get().and_then(|v| v.as_bool()).unwrap_or(default));
            AnyView::new(row_field(
                name,
                check(on, move |next| commit(serde_json::Value::Bool(next))),
            ))
        }
        ExtraSetting::Enum {
            options, default, ..
        } => {
            let options: Vec<(String, String)> = options
                .into_iter()
                .map(|option| (option.value.clone(), option.label.unwrap_or(option.value)))
                .collect();
            let chosen = Signal::derive(move || {
                Some(
                    raw.get()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| default.clone()),
                )
            });
            AnyView::new(row_field(
                name,
                pick(options, chosen, move |value| {
                    commit(serde_json::Value::String(value))
                }),
            ))
        }
        ExtraSetting::Range { range, unit, .. } => {
            let value =
                Signal::derive(move || raw.get().and_then(|v| v.as_f64()).unwrap_or(range.min));
            let send = move |number: f64| {
                if let Some(number) = serde_json::Number::from_f64(number) {
                    commit(serde_json::Value::Number(number));
                }
            };
            if fits_slider(&range) {
                AnyView::new(range_slider(name, unit, range, value, send))
            } else {
                let mut spec = NumberSpec::unit("").within(range.min, range.max);
                spec.step = range.step;
                AnyView::new(row_field(
                    name.clone(),
                    number_field_in(name, unit, value, spec, Signal::stored(false), send),
                ))
            }
        }
        ExtraSetting::String { default, .. } => {
            let text = Signal::derive(move || {
                raw.get()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| default.clone())
            });
            AnyView::new(row_field(
                name.clone(),
                text_field(
                    name,
                    text,
                    "",
                    Signal::stored(false),
                    |_| true,
                    move |value| commit(serde_json::Value::String(value)),
                ),
            ))
        }
    }
}

fn range_slider(
    name: String,
    unit: String,
    range: sdrmm_wire::device::Range,
    value: Signal<f64>,
    commit: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let (pending, change) = debounced(commit);
    let shown = Signal::derive(move || pending.get().unwrap_or_else(|| value.get()));
    let digits = usize::from(range.step.is_some_and(|step| step < 1.0));
    let step = range.step.filter(|step| *step > 0.0).unwrap_or(1.0);
    let min = range.min;
    let read = move |v: f64| format!("{v:.digits$} {unit}").trim().to_owned();
    row_field(
        name,
        slide(shown, range.min, range.max, read, move |v| {
            change(min + ((v - min) / step).round() * step);
        }),
    )
}
