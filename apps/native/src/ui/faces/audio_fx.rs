use std::time::Duration;

use sdrmm_wire::{
    AudioAgcMode, AudioProcessing, DenoiseMode, MAX_AUDIO_NOTCHES, MAX_AUDIO_TONE_HZ,
    MAX_CLICK_THRESHOLD, MAX_NOTCH_WIDTH_HZ, MIN_AUDIO_TONE_HZ, MIN_CLICK_THRESHOLD,
    MIN_NOTCH_WIDTH_HZ, NotchSettings, patch::NodeBody,
};
use zgui::prelude::*;

use crate::{
    binding,
    store::Store,
    ui::{
        kit_audio::{self, button, edit_body},
        params::entry,
        widgets::{check, row_field, segments, slide},
    },
};

const SLIDER_SETTLE: Duration = Duration::from_millis(250);

type Edit = Box<dyn FnOnce(&mut AudioProcessing)>;

#[derive(Clone, Copy)]
struct Chain {
    store: Store,
    settings: Signal<AudioProcessing>,
}

impl Chain {
    fn edit(self, node: &str, change: impl FnOnce(&mut AudioProcessing) + 'static) {
        let change: Edit = Box::new(change);
        edit_body(self.store, node, move |body| {
            if let NodeBody::AudioFx(fx) = body {
                change(&mut fx.settings);
            }
        });
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    let settings = {
        let node = node.clone();
        Signal::derive(move || {
            store
                .graph
                .get()
                .node(&node)
                .and_then(|found| match &found.body {
                    NodeBody::AudioFx(fx) => Some(fx.settings.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        })
    };
    let chain = Chain { store, settings };
    let wired = {
        let node = node.clone();
        Signal::derive(move || !binding::sources_of(&store.graph.get(), &node, "audio").is_empty())
    };
    view! {
        column(class = "face") {
            if move || !wired.get() {
                text(class = "hint") {"Wire channel audio in"}
            }
            {agc(chain, node.clone())}
            {declick(chain, node.clone())}
            {denoise(chain, node.clone())}
            {auto_notch(chain, node.clone())}
            {passband(chain, node.clone())}
            {notches(chain, node)}
        }
    }
}

fn agc(chain: Chain, node: String) -> impl IntoView {
    let mode = Signal::derive(move || chain.settings.get().agc);
    row_field(
        "AGC",
        segments(
            vec![
                (AudioAgcMode::Off, "Off"),
                (AudioAgcMode::Slow, "Slow"),
                (AudioAgcMode::Medium, "Med"),
                (AudioAgcMode::Fast, "Fast"),
            ],
            mode,
            move |agc| chain.edit(&node, move |settings| settings.agc = agc),
        ),
    )
}

fn settled(
    commit: impl Fn(f64) + Clone + 'static,
) -> (RwSignal<Option<f64>>, impl Fn(f64) + Clone) {
    let pending = RwSignal::new(None::<f64>);
    let waiting = StoredValue::new_local(None::<zgui::view::TimeoutHandle>);
    let clock = Timers::current();
    let change = move |value: f64| {
        pending.set(Some(value));
        let commit = commit.clone();
        let land = move || {
            pending.set(None);
            commit(value);
        };
        match &clock {
            Some(clock) => waiting.set_value(Some(clock.set_timeout(SLIDER_SETTLE, land))),
            None => land(),
        }
    };
    (pending, change)
}

fn declick(chain: Chain, node: String) -> impl IntoView {
    let on = Signal::derive(move || chain.settings.get().click_removal.enabled);
    let toggle = {
        let node = node.clone();
        move |enabled: bool| {
            chain.edit(&node, move |settings| {
                settings.click_removal.enabled = enabled
            })
        }
    };
    let (pending, change) = settled(move |threshold: f64| {
        chain.edit(&node, move |settings| {
            settings.click_removal.threshold = threshold as f32
        });
    });
    let threshold = Signal::derive(move || {
        pending
            .get()
            .unwrap_or_else(|| f64::from(chain.settings.get().click_removal.threshold))
    });
    row_field(
        "De-click",
        view! {
            row(class = "field__body") {
                {check(on, toggle)}
                {slide(
                    threshold,
                    f64::from(MIN_CLICK_THRESHOLD),
                    f64::from(MAX_CLICK_THRESHOLD),
                    |value| format!("{value:.1}\u{d7}"),
                    change,
                )}
            }
        },
    )
}

fn denoise(chain: Chain, node: String) -> impl IntoView {
    let on = Signal::derive(move || chain.settings.get().denoise.enabled);
    let mode = Signal::derive(move || chain.settings.get().denoise.mode);
    let toggle = {
        let node = node.clone();
        move |enabled: bool| chain.edit(&node, move |settings| settings.denoise.enabled = enabled)
    };
    let pick = {
        let node = node.clone();
        move |mode: DenoiseMode| chain.edit(&node, move |settings| settings.denoise.mode = mode)
    };
    let (pending, change) = settled(move |strength: f64| {
        chain.edit(&node, move |settings| {
            settings.denoise.strength = strength as f32
        });
    });
    let strength = Signal::derive(move || {
        pending
            .get()
            .unwrap_or_else(|| f64::from(chain.settings.get().denoise.strength))
    });
    view! {
        column(class = "audio-controls") {
            {row_field("Denoise", view! {
                row(class = "field__body") {
                    {check(on, toggle)}
                    {segments(vec![(DenoiseMode::Spectral, "Spectral"), (DenoiseMode::Neural, "Neural")], mode, pick)}
                }
            })}
            {row_field("Strength", slide(strength, 0.0, 1.0, |value| format!("{:.0}%", value * 100.0), change))}
        }
    }
}

fn auto_notch(chain: Chain, node: String) -> impl IntoView {
    let on = Signal::derive(move || chain.settings.get().auto_notch);
    row_field(
        "Auto notch",
        check(on, move |enabled: bool| {
            chain.edit(&node, move |settings| settings.auto_notch = enabled);
        }),
    )
}

fn hz_entry(
    value: Signal<f64>,
    label: &str,
    range: (f64, f64),
    commit: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let shown = Signal::derive(move || format!("{:.0}", value.get()));
    entry(shown, label.to_owned(), false, move |text| {
        let hz = parse_hz(&text, range)?;
        commit(hz);
        Ok(())
    })
}

fn parse_hz(text: &str, (min, max): (f64, f64)) -> Result<f64, String> {
    let hz = text
        .trim()
        .trim_end_matches("Hz")
        .trim()
        .parse::<f64>()
        .map_err(|_| String::from("Enter a number of Hz"))?;
    if hz.is_finite() && (min..=max).contains(&hz) {
        Ok(hz)
    } else {
        Err(format!("{min:.0} to {max:.0} Hz"))
    }
}

fn passband(chain: Chain, node: String) -> impl IntoView {
    let on = Signal::derive(move || chain.settings.get().filter.enabled);
    let low = Signal::derive(move || chain.settings.get().filter.low_hz);
    let high = Signal::derive(move || chain.settings.get().filter.high_hz);
    let range = (MIN_AUDIO_TONE_HZ, MAX_AUDIO_TONE_HZ);
    let toggle = {
        let node = node.clone();
        move |enabled: bool| chain.edit(&node, move |settings| settings.filter.enabled = enabled)
    };
    let set_low = {
        let node = node.clone();
        move |hz: f64| chain.edit(&node, move |settings| settings.filter.low_hz = hz)
    };
    let set_high = move |hz: f64| chain.edit(&node, move |settings| settings.filter.high_hz = hz);
    view! {
        column(class = "audio-controls") {
            {row_field("Passband", view! {
                row(class = "field__body") {
                    {check(on, toggle)}
                    {hz_entry(low, "Low cut", range, set_low)}
                    {hz_entry(high, "High cut", range, set_high)}
                }
            })}
            {kit_audio::alert(move || (low.get() >= high.get()).then(|| String::from("Low cut must sit below high cut")))}
        }
    }
}

fn notches(chain: Chain, node: String) -> impl IntoView {
    let count = Signal::derive(move || chain.settings.get().notches.len());
    let add = {
        let node = node.clone();
        move || {
            chain.edit(&node, |settings| {
                if settings.notches.len() < MAX_AUDIO_NOTCHES {
                    settings.notches.push(NotchSettings::default());
                }
            });
        }
    };
    let rows = move || {
        (0..count.get())
            .map(|index| AnyView::new(notch_row(chain, node.clone(), index)))
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "audio-controls") {
            {rows}
            row(class = "field__body") {
                {button(
                    || String::from("+ Notch"),
                    Signal::stored(false),
                    Signal::derive(move || count.get() >= MAX_AUDIO_NOTCHES),
                    add,
                )}
            }
        }
    }
}

fn notch_row(chain: Chain, node: String, index: usize) -> impl IntoView {
    let notch = Signal::derive(move || {
        chain
            .settings
            .get()
            .notches
            .get(index)
            .copied()
            .unwrap_or_default()
    });
    let freq = Signal::derive(move || notch.get().freq_hz);
    let width = Signal::derive(move || notch.get().width_hz);
    let edit = move |node: &str, change: fn(&mut NotchSettings, f64), hz: f64| {
        chain.edit(node, move |settings| {
            if let Some(found) = settings.notches.get_mut(index) {
                change(found, hz);
            }
        });
    };
    let set_freq = {
        let node = node.clone();
        move |hz: f64| edit(&node, |found, hz| found.freq_hz = hz, hz)
    };
    let set_width = {
        let node = node.clone();
        move |hz: f64| edit(&node, |found, hz| found.width_hz = hz, hz)
    };
    let remove = move || {
        chain.edit(&node, move |settings| {
            if index < settings.notches.len() {
                settings.notches.remove(index);
            }
        });
    };
    row_field(
        format!("Notch {}", index + 1),
        view! {
            row(class = "field__body") {
                {hz_entry(freq, "Frequency", (MIN_AUDIO_TONE_HZ, MAX_AUDIO_TONE_HZ), set_freq)}
                {hz_entry(width, "Width", (MIN_NOTCH_WIDTH_HZ, MAX_NOTCH_WIDTH_HZ), set_width)}
                {button(|| String::from("x"), Signal::stored(false), Signal::stored(false), remove)}
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_is_read_in_hertz_inside_its_range() {
        let range = (MIN_AUDIO_TONE_HZ, MAX_AUDIO_TONE_HZ);
        assert_eq!(parse_hz("1000", range), Ok(1_000.0));
        assert_eq!(parse_hz(" 440 Hz ", range), Ok(440.0));
        assert!(parse_hz("10", range).is_err());
        assert!(parse_hz("loud", range).is_err());
        assert!(parse_hz("NaN", range).is_err());
    }
}
