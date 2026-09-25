use std::{cell::RefCell, rc::Rc, sync::Arc};

use sdrmm_wire::AudioRoute;
use zgui::prelude::*;

use crate::{
    audio::{
        Audio, Entry, SAMPLE_RATE,
        spectrogram::{self, AudioSpectrogram},
        use_audio,
    },
    binding::{self, Input},
    socket::Spectrum,
    store::Store,
    ui::{
        gpu::WaterfallSurface,
        kit_audio::{self, button},
        plot::Palette,
        widgets::{check, meter, row_field, slide},
    },
};

const TICKS_HZ: [f32; 4] = [3_000.0, 6_000.0, 12_000.0, 18_000.0];

const SHEET: &str = css!(
    r#"
.gram { position: relative; width: 100%; height: 96px; border-radius: 3px; overflow: hidden; background-color: var(--bg); }
.gram__fall { width: 100%; height: 96px; }
.gram__tick { position: absolute; bottom: 1px; padding: 0 2px; font-size: 9px; color: var(--ink-dim); background-color: var(--bg); pointer-events: none; }
.mute { flex-direction: row; gap: 4px; align-items: center; font-size: 10px; color: var(--ink-faint); }
"#
);

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    install_stylesheet("speaker", SHEET);
    let inputs = Signal::derive(move || {
        let state = store.state.get();
        binding::inputs_of(
            &store.graph.get(),
            &node,
            "audio",
            &state.device_sets,
            &state.trunk_systems,
        )
    });
    let Some(audio) = use_audio() else {
        return AnyView::new(
            view! { column(class = "face") { text(class = "hint") {"No audio output"} } },
        );
    };
    AnyView::new(view! {
        column(class = "face") {
            if move || inputs.get().is_empty() {
                text(class = "hint") {"Wire a channel's audio in"}
            }
            for input in move || inputs.get(), key = |input: &Input| input.route() {
                {lane(store, audio, input)}
            }
        }
    })
}

fn lane(store: Store, audio: Audio, input: Input) -> impl IntoView {
    let route = input.route();
    let entry = {
        let route = route.clone();
        Signal::derive(move || audio.entry(&route))
    };
    let name = {
        let node = input.node.clone();
        let kind = input.channel.settings.params.type_id().to_owned();
        move || kit_audio::node_label(store, &node, &kind)
    };
    let wanted = Signal::derive(move || entry.get().wanted);
    let toggle = {
        let route = route.clone();
        move || {
            if audio.entry(&route).wanted {
                audio.stop(&route);
            } else {
                audio.play(&route);
            }
        }
    };
    let volume = Signal::derive(move || f64::from(entry.get().volume));
    let set_volume = {
        let route = route.clone();
        move |value: f64| audio.set_volume(&route, value as f32)
    };
    let muted = Signal::derive(move || entry.get().muted);
    let level = Signal::derive(move || {
        store
            .levels
            .get()
            .get(&(input.device_set, input.channel.id))
            .map_or(-120.0, |level| level.level_db)
    });
    let mute = {
        let route = route.clone();
        move |on: bool| audio.set_muted(&route, on)
    };
    view! {
        column(class = "lane") {
            row(class = "lane__head") {
                {button(
                    move || String::from(if wanted.get() { "Stop" } else { "Play" }),
                    wanted,
                    Signal::stored(false),
                    toggle,
                )}
                text(class = "lane__name") {{name}}
                row(class = "mute") {
                    {check(muted, mute)}
                    text {"Mute"}
                }
            }
            {row_field("Level", meter(level))}
            {slide(volume, 0.0, 1.0, |value| format!("{:.0}%", value * 100.0), set_volume)}
            {gram(audio, route.clone())}
            {health(audio, route, entry)}
            {kit_audio::alert(move || entry.get().error)}
        }
    }
}

fn gram(audio: Audio, route: AudioRoute) -> impl IntoView {
    let rows = RwSignal::new(None::<Arc<Spectrum>>);
    let analyser = Rc::new(RefCell::new(AudioSpectrogram::new(
        spectrogram::FFT_SIZE,
        spectrogram::HOP,
    )));
    let seq = Rc::new(RefCell::new(0u32));
    audio.watch(route, move |pcm, channels| {
        analyser.borrow_mut().push(pcm, channels, |row| {
            let mut next = seq.borrow_mut();
            *next = next.wrapping_add(1);
            rows.set(Some(Arc::new(Spectrum {
                stream_id: 0,
                seq: *next,
                center_hz: 0.0,
                span_hz: SAMPLE_RATE as f32,
                db_min: spectrogram::DB_MIN,
                db_max: spectrogram::DB_MAX,
                bins: row.to_vec(),
            })));
        });
    });
    let ticks: Vec<_> = TICKS_HZ
        .into_iter()
        .map(|hz| {
            let left = spectrogram::tick_fraction(hz, SAMPLE_RATE as f32) * 100.0;
            view! {
                text(class = "gram__tick", style:left = Some(format!("{left}%"))) {
                    {format!("{:.0}k", hz / 1_000.0)}
                }
            }
        })
        .collect();
    view! {
        box(class = "gram") {
            {zgui::elements::surface()
                .class("gram__fall")
                .renderer(WaterfallSurface::new(rows.into(), Signal::stored(Palette::Viridis)))
                .into_view()}
            {ticks}
        }
    }
}

fn health(audio: Audio, route: AudioRoute, entry: Signal<Entry>) -> impl IntoView {
    let chips = move || {
        let entry = entry.get();
        let health = entry.health;
        let mut shown = Vec::new();
        if entry.live {
            shown.push(("Latency", format!("{:.0} ms", audio.latency_ms(&route))));
        }
        if health.trimmed_ms >= 1.0 {
            shown.push(("Trimmed", format!("{:.0} ms", health.trimmed_ms)));
        }
        if health.lost_ms >= 1.0 {
            shown.push(("Dropped", format!("{:.0} ms", health.lost_ms)));
        }
        if health.underruns > 0 {
            shown.push(("Stalls", health.underruns.to_string()));
        }
        shown
            .into_iter()
            .map(|(label, value)| {
                view! {
                    row(class = "chip") {
                        text(class = "legend") {{label}}
                        text {{value}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! { row(class = "chips") {{chips}} }
}
