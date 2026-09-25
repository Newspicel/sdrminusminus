use zgui::prelude::*;

use super::{
    ScopeCx,
    actions::{radio, selected_channel},
    live::FrameMeta,
    pick::format_mhz,
    plot::{AXIS_H, format_tick},
    radio::{Radio, bandwidth_hz, marker_hint, marker_name},
    settings,
    traces::{DbWindow, trace_unit},
    view::{
        Placed, SpectrumView, cluster_markers, decibel_ticks, frequency_ticks, label_width,
        offset_to_span, span_to_offset,
    },
};

const READOUT_CHAR_PX: f64 = 6.0;

fn css_size(node: NodeRef) -> Signal<(f64, f64), LocalStorage> {
    let size = node.observe_content_size();
    Signal::derive_local(move || {
        let measured = size.get();
        let scale = f64::from(node.scale().max(0.01));
        (
            f64::from(measured.width.0) / scale,
            f64::from(measured.height.0) / scale,
        )
    })
}

fn percent(at: f64) -> Option<String> {
    Some(format!("{:.4}%", at * 100.0))
}

fn px(value: f64) -> Option<String> {
    Some(format!("{value:.1}px"))
}

pub fn trace_labels(cx: ScopeCx) -> impl IntoView {
    let size = css_size(cx.trace_box);
    let decibels = move || {
        let (_, height) = size.get();
        let plot_h = (height - AXIS_H).max(1.0);
        let window = cx.shown_window();
        cx.meta.get().map(|_| {
            decibel_labels(window, plot_h)
                .into_iter()
                .map(|(top, label)| {
                    view! { text(class = "scope__db", style:top = px(top)) {{label}} }
                })
                .collect::<Vec<_>>()
        })
    };
    let hertz = move || {
        let (width, _) = size.get();
        let view = cx.view.get();
        cx.meta.get().map(|meta| {
            frequency_labels(meta, view, width)
                .into_iter()
                .map(|(at, label)| {
                    view! { text(class = "scope__hz", style:left = percent(at)) {{label}} }
                })
                .collect::<Vec<_>>()
        })
    };
    let readout = move || {
        let (width, _) = size.get();
        let (at, text) = (cx.hover.get()?, cx.readout.get()?);
        let (left, right) = readout_place(at, width, &text);
        Some(view! {
            text(class = "scope__readout", style:left = left.and_then(px), style:right = right.and_then(px)) {{text}}
        })
    };
    view! {
        box(class = "scope__labels") {
            {decibels}
            {hertz}
            {readout}
        }
    }
}

#[must_use]
pub fn decibel_labels(window: DbWindow, plot_h: f64) -> Vec<(f64, String)> {
    decibel_ticks(window.min, window.max, 4.0)
        .into_iter()
        .filter_map(|db| {
            let y = (plot_h * (1.0 - trace_unit(db, window))).round() + 0.5;
            (y > 12.0 && y < plot_h - 4.0).then(|| (y - 13.0, format!("{db:.0}")))
        })
        .collect()
}

#[must_use]
pub fn frequency_labels(meta: FrameMeta, view: SpectrumView, width: f64) -> Vec<(f64, String)> {
    let visible = meta.span_hz * view.width();
    let target = (width / 110.0).floor().max(2.0);
    frequency_ticks(meta.centre_hz, meta.span_hz, view, target)
        .into_iter()
        .map(|tick| (tick.at, format_tick(tick.hz, visible)))
        .collect()
}

#[must_use]
pub fn readout_place(at: f64, width: f64, text: &str) -> (Option<f64>, Option<f64>) {
    let x = (at * width).round() + 0.5;
    let w = text.chars().count() as f64 * READOUT_CHAR_PX + 8.0;
    if x + 6.0 + w > width {
        (None, Some(width - x + 6.0))
    } else {
        (Some(x + 6.0), None)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    id: u32,
    hz: f64,
    at: f64,
    width: f64,
    name: String,
    hint: String,
    bandwidth: Option<f64>,
    held: bool,
}

impl Placed for Drawn {
    fn at(&self) -> f64 {
        self.at
    }

    fn width(&self) -> f64 {
        self.width
    }
}

fn drawn_markers(
    radio: &Radio,
    meta: FrameMeta,
    view: SpectrumView,
    preview: Option<(u32, f64)>,
    width: f64,
) -> Vec<Drawn> {
    radio
        .channels
        .iter()
        .filter_map(|channel| {
            let offset = match preview {
                Some((id, offset)) if id == channel.id => offset,
                _ => channel.settings.frequency_hz - meta.centre_hz,
            };
            let at = view.place(offset_to_span(offset, meta.span_hz));
            let owner = radio.owners.get(&channel.id);
            let name = marker_name(channel, owner);
            let hz = meta.centre_hz + offset;
            ((-0.02..=1.02).contains(&at)).then(|| Drawn {
                id: channel.id,
                hz,
                at,
                width: label_width(&name, width),
                hint: marker_hint(channel, hz, owner, radio.locked.contains(&channel.id)),
                name,
                bandwidth: bandwidth_hz(&channel.settings.params),
                held: radio.held(channel.id),
            })
        })
        .collect()
}

fn grab(cx: ScopeCx, id: u32) -> impl Fn(&mut EventCx<'_, events::PointerDown>) + 'static {
    move |_: &mut EventCx<'_, events::PointerDown>| cx.grabbed.set_value(Some(id))
}

fn marker_body(cx: ScopeCx, marker: &Drawn, visible: f64, selected: Option<u32>) -> impl IntoView {
    let on = selected == Some(marker.id);
    let shade = marker.bandwidth.filter(|_| visible > 0.0).map(|bandwidth| {
        view! {
            box(
                class = "scope__shade",
                class:on = on,
                style:left = percent(marker.at),
                style:width = percent(bandwidth / visible)
            )
        }
    });
    view! {
        {shade}
        box(class = "scope__line", class:on = on, style:left = percent(marker.at))
        box(
            class = "scope__grab",
            class:held = marker.held,
            style:left = percent(marker.at),
            a11y:role = Role::Button,
            a11y:label = marker.hint.clone(),
            on:pointer_down = grab(cx, marker.id)
        )
    }
}

fn marker_chip(
    cx: ScopeCx,
    marker: &Drawn,
    on: bool,
    extra: Option<usize>,
    class: &'static str,
) -> impl IntoView {
    let count = extra.map(|count| view! { text(class = "scope__count") {{format!("x{count}")}} });
    view! {
        row(
            class = format!("scope__chip {class}"),
            class:on = on,
            class:held = marker.held,
            a11y:label = marker.hint.clone(),
            on:pointer_down = grab(cx, marker.id)
        ) {
            text {{marker.name.clone()}}
            {count}
        }
    }
}

fn cluster_view(cx: ScopeCx, members: &[Drawn], visible: f64, selected: Option<u32>) -> AnyView {
    let Some(anchor) = members.first() else {
        return AnyView::new(());
    };
    let shown = members
        .iter()
        .find(|member| Some(member.id) == selected)
        .unwrap_or(anchor);
    let stacked = members.len() > 1;
    let bodies: Vec<_> = members
        .iter()
        .map(|member| AnyView::new(marker_body(cx, member, visible, selected)))
        .collect();
    let spread: Vec<_> = if stacked {
        members
            .iter()
            .map(|member| {
                AnyView::new(marker_chip(
                    cx,
                    member,
                    Some(member.id) == selected,
                    None,
                    "scope__chip--member",
                ))
            })
            .collect()
    } else {
        Vec::new()
    };
    let head = marker_chip(
        cx,
        shown,
        Some(shown.id) == selected,
        stacked.then_some(members.len()),
        if stacked {
            "scope__chip--stack"
        } else {
            "scope__chip--single"
        },
    );
    AnyView::new(view! {
        {bodies}
        column(class = "scope__chips", style:left = percent(shown.at)) {
            {head}
            {spread}
        }
    })
}

pub fn markers(cx: ScopeCx) -> impl IntoView {
    let size = css_size(cx.plot);
    let drawn = move || {
        let meta = cx.meta.get()?;
        let radio = radio(cx);
        let view = cx.view.get();
        let selected = selected_channel(cx, &radio);
        let (width, _) = size.get();
        let markers = drawn_markers(&radio, meta, view, cx.preview.get(), width);
        let visible = meta.span_hz * view.width();
        Some(
            cluster_markers(&markers)
                .iter()
                .map(|members| cluster_view(cx, members, visible, selected))
                .collect::<Vec<_>>(),
        )
    };
    view! { box(class = "scope__markers") {{drawn}} }
}

#[derive(Clone, Debug, PartialEq)]
struct Mark {
    label: String,
    hz: f64,
    at: f64,
    width: f64,
}

impl Placed for Mark {
    fn at(&self) -> f64 {
        self.at
    }

    fn width(&self) -> f64 {
        self.width
    }
}

fn bookmark_marks(cx: ScopeCx, width: f64) -> Vec<Mark> {
    let Some(meta) = cx.meta.get().filter(|meta| meta.span_hz > 0.0) else {
        return Vec::new();
    };
    let view = cx.view.get();
    cx.bookmarks
        .get()
        .iter()
        .map(|bookmark| Mark {
            label: bookmark.label.clone(),
            hz: bookmark.freq_hz,
            at: view.place(offset_to_span(
                bookmark.freq_hz - meta.centre_hz,
                meta.span_hz,
            )),
            width: label_width(&bookmark.label, width),
        })
        .filter(|mark| (0.0..=1.0).contains(&mark.at))
        .collect()
}

pub fn bookmark_lines(cx: ScopeCx) -> impl IntoView {
    let size = css_size(cx.plot);
    let lines = move || {
        bookmark_marks(cx, size.get().0)
            .into_iter()
            .map(|mark| view! { box(class = "scope__bookmark", style:left = percent(mark.at)) })
            .collect::<Vec<_>>()
    };
    view! { box(class = "scope__markers") {{lines}} }
}

pub fn bookmark_labels(cx: ScopeCx) -> impl IntoView {
    let size = css_size(cx.plot);
    let labels = move || {
        cluster_markers(&bookmark_marks(cx, size.get().0))
            .into_iter()
            .filter_map(|members| {
                let anchor = members.first()?.clone();
                let count = (members.len() > 1)
                    .then(|| view! { text(class = "scope__count") {{format!("x{}", members.len())}} });
                let tip: Vec<_> = members
                    .iter()
                    .map(|mark| view! { text {{format!("{}: {}", mark.label, format_mhz(mark.hz))}} })
                    .collect();
                Some(view! {
                    column(class = "scope__tag", style:left = percent(anchor.at)) {
                        row(class = "scope__tag-label") {
                            text {{anchor.label}}
                            {count}
                        }
                        column(class = "scope__tip") {{tip}}
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    view! { box(class = "scope__markers") {{labels}} }
}

#[must_use]
pub fn legend(meta: FrameMeta, view: SpectrumView, window: DbWindow) -> String {
    let visible = meta.span_hz * view.width();
    let centre = meta.centre_hz + span_to_offset(f64::midpoint(view.start, view.end), meta.span_hz);
    let span = if visible >= 1e6 {
        format!("{:.3} MHz", visible / 1e6)
    } else {
        format!("{:.1} kHz", visible / 1e3)
    };
    format!(
        "{:.4} MHz   {span}   {:.0}…{:.0} dBFS",
        centre / 1e6,
        window.min,
        window.max
    )
}

pub fn chrome(cx: ScopeCx) -> impl IntoView {
    let legend_text = move || {
        let meta = cx.meta.get()?;
        let manual = if cx.range.get().is_some() {
            " · manual"
        } else {
            ""
        };
        Some(format!(
            "{}{manual}",
            legend(meta, cx.view.get(), cx.shown_window())
        ))
    };
    let zoom = move || {
        let view = cx.view.get();
        (!view.is_full()).then(|| {
            view! {
                control(
                    class = "scope__button",
                    on:pointer_down:stop = move |_| {},
                    on:click:stop = move |_| cx.view.set(super::view::FULL_VIEW)
                ) {
                    {format!("{:.1}x · reset", 1.0 / view.width())}
                }
            }
        })
    };
    view! {
        box(class = "scope__chrome") {
            text(class = "scope__legend") {{legend_text}}
            row(class = "scope__tools") {
                {settings::button(cx)}
                {zoom}
            }
            {move || cx.settings_open.get().then(|| AnyView::new(settings::panel(cx)))}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{super::view::FULL_VIEW, *};

    const WINDOW: DbWindow = DbWindow {
        min: -100.0,
        max: -20.0,
    };

    #[test]
    fn decibel_labels_skip_the_crowded_edges() {
        let labels = decibel_labels(WINDOW, 100.0);
        assert!(!labels.is_empty());
        assert!(
            labels
                .iter()
                .all(|(top, _)| *top + 13.0 > 12.0 && *top + 13.0 < 96.0)
        );
        assert!(labels.iter().any(|(_, label)| label == "-60"));
    }

    #[test]
    fn frequency_labels_follow_the_plot_width() {
        let meta = FrameMeta {
            centre_hz: 100e6,
            span_hz: 2e6,
            db_min: -100.0,
            db_max: -20.0,
        };
        let narrow = frequency_labels(meta, FULL_VIEW, 220.0);
        let wide = frequency_labels(meta, FULL_VIEW, 1100.0);
        assert!(wide.len() > narrow.len());
        assert!(narrow.iter().all(|(at, _)| (0.0..=1.0).contains(at)));
    }

    #[test]
    fn the_readout_flips_side_at_the_right_edge() {
        assert_eq!(readout_place(0.1, 400.0, "100.0000 MHz").0, Some(46.5));
        let (left, right) = readout_place(0.95, 400.0, "100.0000 MHz  -60.0 dBFS");
        assert!(left.is_none() && right.is_some());
    }

    #[test]
    fn the_legend_reads_the_visible_centre_span_and_levels() {
        let meta = FrameMeta {
            centre_hz: 100e6,
            span_hz: 2e6,
            db_min: -100.0,
            db_max: -20.0,
        };
        assert_eq!(
            legend(meta, FULL_VIEW, WINDOW),
            "100.0000 MHz   2.000 MHz   -100…-20 dBFS"
        );
        let zoomed = SpectrumView {
            start: 0.5,
            end: 0.55,
        };
        assert_eq!(
            legend(meta, zoomed, WINDOW),
            "100.0500 MHz   100.0 kHz   -100…-20 dBFS"
        );
    }
}
