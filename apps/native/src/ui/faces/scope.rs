#[allow(unused_imports)]
use super::*;

pub fn face(store: Store, node: String) -> impl IntoView {
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
            plot::markers(spectrum.center_hz, f64::from(spectrum.span_hz), &channels)
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
            |spectrum| plot::readout(&spectrum),
        )
    };
    let db_ticks = move || {
        let (db_min, db_max) = spectrum.get().map_or((-120.0, -20.0), |spectrum| {
            (spectrum.db_min, spectrum.db_max)
        });
        plot::decibel_ticks(db_min, db_max)
            .into_iter()
            .map(|label| view! { text(class = "scope__read") {{label}} })
            .collect::<Vec<_>>()
    };
    let hz_ticks = move || {
        let Some(spectrum) = spectrum.get() else {
            return Vec::new();
        };
        plot::frequency_ticks(spectrum.center_hz, f64::from(spectrum.span_hz), 9)
            .into_iter()
            .map(|label| view! { text(class = "scope__read") {{label}} })
            .collect::<Vec<_>>()
    };

    view! {
        column(class = "scope") {
            box(class = "scope__plot") {
                {plot::trace(spectrum, marks)}
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
