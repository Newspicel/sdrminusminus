use sdrmm_wire::{
    device::{AgcSetting, Capabilities, DeviceSettings, GainUnit, StreamSettings},
    state::DeviceSet,
};
use zgui::prelude::*;

use super::{NumberSpec, button, format_mhz, number_field, tune_to};
use crate::{
    store::Store,
    ui::{
        faces::channel::settings::{NumberLimit, clamp_offset_hz, offset_for_frequency_hz},
        widgets::check,
    },
};

const DOWN_HZ: [f64; 2] = [-25_000.0, -5_000.0];
const UP_HZ: [f64; 2] = [5_000.0, 25_000.0];

#[must_use]
pub fn step_label(hz: f64) -> String {
    let sign = if hz > 0.0 { "+" } else { "−" };
    format!("{sign}{}k", super::format_number(hz.abs() / 1000.0, None))
}

pub fn offset_stepper(
    offset_hz: Signal<f64>,
    limit_hz: Signal<Option<f64>>,
    center_hz: Signal<Option<f64>>,
    on_offset: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let step_button = {
        let on_offset = on_offset.clone();
        move |hz: f64| {
            let on_offset = on_offset.clone();
            button(step_label(hz), "kc-btn kc-btn--mono", false, move || {
                on_offset(clamp_offset_hz(
                    offset_hz.get_untracked() + hz,
                    limit_hz.get_untracked(),
                ));
            })
        }
    };
    let field = {
        let on_offset = on_offset.clone();
        move || {
            let limit_khz = limit_hz.get().map(|limit| limit / 1000.0);
            let spec = NumberSpec::new("Offset")
                .limit(NumberLimit {
                    min: limit_khz.map(|limit| -limit),
                    max: limit_khz,
                    step: Some(0.5),
                })
                .unit("kHz")
                .size("kc-num kc-num--narrow");
            let on_offset = on_offset.clone();
            number_field(
                Signal::derive(move || Some(offset_hz.get() / 1000.0)),
                spec,
                move |khz| {
                    if let Some(khz) = khz {
                        on_offset(clamp_offset_hz(
                            (khz * 1000.0).round(),
                            limit_hz.get_untracked(),
                        ));
                    }
                },
            )
        }
    };
    let tune = move || {
        let center = center_hz.get().filter(|center| center.is_finite())?;
        let hz = Signal::derive(move || center_hz.get().unwrap_or(center) + offset_hz.get());
        let hint = Signal::derive(move || match limit_hz.get() {
            Some(limit) => format!(
                "Reaches {} to {}",
                format_mhz(center - limit),
                format_mhz(center + limit)
            ),
            None => format!("Center {}", format_mhz(center)),
        });
        Some(AnyView::new(tune_to(
            "Type a frequency to sit on",
            hz,
            hint,
            move |entered| offset_for_frequency_hz(entered, center, limit_hz.get_untracked()),
            Signal::stored(false),
            on_offset.clone(),
        )))
    };
    view! {
        row(class = "kc-cell") {
            {DOWN_HZ.into_iter().map(|hz| AnyView::new(step_button.clone()(hz))).collect::<Vec<_>>()}
            {field}
            {UP_HZ.into_iter().map(|hz| AnyView::new(step_button.clone()(hz))).collect::<Vec<_>>()}
            {tune}
        }
    }
}

fn for_stream(set: &DeviceSet, stream: u32) -> Option<AgcSetting> {
    if set.capabilities.per_stream.agc
        && let Some(own) = set
            .settings
            .streams
            .iter()
            .find(|entry| entry.stream == stream)
            .and_then(|entry| entry.agc.clone())
    {
        return Some(own);
    }
    set.settings.agc.clone()
}

#[must_use]
pub fn agc_state(capabilities: &Capabilities, reported: Option<AgcSetting>) -> AgcSetting {
    let offered = !matches!(capabilities.agc, sdrmm_wire::device::Agc::None);
    let mode = reported
        .as_ref()
        .and_then(|agc| agc.mode.clone())
        .or_else(|| capabilities.agc.first_mode().map(str::to_owned));
    AgcSetting {
        on: offered && reported.is_some_and(|agc| agc.on),
        mode,
    }
}

#[must_use]
pub fn lane_agc(set: &DeviceSet, stream: u32) -> AgcSetting {
    agc_state(&set.capabilities, for_stream(set, stream))
}

#[must_use]
pub fn agc_delta(capabilities: &Capabilities, stream: u32, agc: AgcSetting) -> DeviceSettings {
    if capabilities.per_stream.agc {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                agc: Some(agc),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    } else {
        DeviceSettings {
            agc: Some(agc),
            ..DeviceSettings::default()
        }
    }
}

#[must_use]
pub fn agc_gain_db(set: &DeviceSet, stream: u32) -> Option<f64> {
    if set.capabilities.gains.len() != 1 || !lane_agc(set, stream).on {
        return None;
    }
    set.agc_gains
        .iter()
        .find(|reading| reading.stream == stream)
        .map(|reading| reading.value_db)
}

#[must_use]
pub fn agc_tip(set: &DeviceSet, stream: u32, advised: bool) -> String {
    if !lane_agc(set, stream).on {
        return String::from(if advised {
            "AGC off, as coherent lanes want"
        } else {
            "Let the radio set its own gain"
        });
    }
    let reading = match (agc_gain_db(set, stream), set.capabilities.gains.first()) {
        (Some(db), Some(stage)) if stage.unit == GainUnit::Index => {
            format!(" at {} dB", db.round())
        }
        (Some(db), Some(_)) => format!(" at {db:.1} dB"),
        _ => String::new(),
    };
    let advice = if advised {
        ". Fixed gain keeps coherent lanes calibrated"
    } else {
        ""
    };
    format!("AGC on{reading}{advice}")
}

pub fn agc_auto(
    store: Store,
    set: Signal<Option<DeviceSet>>,
    stream: u32,
    advised: bool,
) -> impl IntoView {
    let on = Signal::derive(move || set.get().is_some_and(|set| lane_agc(&set, stream).on));
    let warn = move || advised && on.get();
    let toggle = move |next: bool| {
        let Some(held) = set.get_untracked() else {
            return;
        };
        let mut agc = lane_agc(&held, stream);
        agc.on = next;
        store.set_device(held.id, agc_delta(&held.capabilities, stream, agc));
    };
    let hint = move || -> zgui::vocab::SharedString {
        set.get()
            .map(|set| agc_tip(&set, stream, advised))
            .unwrap_or_default()
            .into()
    };
    view! {
        row(class = "kc-cell", a11y:description = hint) {
            {check(on, toggle)}
            text(class = "kc-legend", class:kc-warn = warn) {"Auto"}
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::device::{Agc, AgcGain};

    use super::*;

    fn set(agc: Agc, per_stream: bool) -> DeviceSet {
        let mut set: DeviceSet = serde_json::from_value(serde_json::json!({
            "id": 1,
            "device": { "driver": "virtual", "key": "siggen", "label": "Signal Generator" },
            "capabilities": {
                "freq_ranges": [], "sample_rates": [], "antennas": [], "bandwidths": [],
                "gains": [{ "name": "LNA", "kind": "lna", "range": { "min": 0.0, "max": 40.0 } }]
            },
            "settings": {},
            "status": "running",
            "channels": []
        }))
        .expect("a device set");
        set.capabilities.agc = agc;
        set.capabilities.per_stream.agc = per_stream;
        set
    }

    #[test]
    fn a_step_button_reads_in_kilohertz_with_its_sign() {
        assert_eq!(step_label(-25_000.0), "−25k");
        assert_eq!(step_label(5_000.0), "+5k");
    }

    #[test]
    fn agc_is_only_on_when_the_radio_offers_it() {
        let mut none = set(Agc::None, false);
        none.settings.agc = Some(AgcSetting::switched(true));
        assert!(!lane_agc(&none, 0).on);
        let mut switch = set(Agc::Switch, false);
        switch.settings.agc = Some(AgcSetting::switched(true));
        assert!(lane_agc(&switch, 0).on);
    }

    #[test]
    fn a_per_lane_agc_is_written_to_its_own_lane() {
        let lanes = set(Agc::Switch, true);
        let delta = agc_delta(&lanes.capabilities, 2, AgcSetting::switched(true));
        assert_eq!(delta.agc, None);
        assert_eq!(delta.streams[0].stream, 2);
        assert_eq!(delta.streams[0].agc, Some(AgcSetting::switched(true)));
        let whole = agc_delta(&set(Agc::Switch, false).capabilities, 2, AgcSetting::off());
        assert_eq!(whole.agc, Some(AgcSetting::off()));
        assert!(whole.streams.is_empty());
    }

    #[test]
    fn the_tip_says_what_agc_is_doing_and_warns_coherent_lanes() {
        let mut radio = set(Agc::Switch, false);
        assert_eq!(agc_tip(&radio, 0, false), "Let the radio set its own gain");
        assert_eq!(agc_tip(&radio, 0, true), "AGC off, as coherent lanes want");
        radio.settings.agc = Some(AgcSetting::switched(true));
        radio.agc_gains = vec![AgcGain {
            stream: 0,
            value_db: 21.26,
        }];
        assert_eq!(agc_tip(&radio, 0, false), "AGC on at 21.3 dB");
        assert_eq!(
            agc_tip(&radio, 0, true),
            "AGC on at 21.3 dB. Fixed gain keeps coherent lanes calibrated"
        );
    }
}
