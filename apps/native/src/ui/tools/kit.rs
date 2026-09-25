use std::{future::Future, path::PathBuf};

use sdrmm_wire::tools::{ToolRequest, ToolResponse};
use zgui::{
    canvas::{Brush, zgui_color::Color},
    geom::{Css, CssPx, Point},
    prelude::*,
};

use crate::{store::Store, ui::params::entry};

pub struct Query<T: Send + Sync + 'static> {
    pub data: RwSignal<Option<T>>,
    pub error: RwSignal<Option<String>>,
    pub busy: RwSignal<bool>,
    turn: RwSignal<u64>,
}

impl<T: Send + Sync + 'static> Clone for Query<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for Query<T> {}

impl<T: Send + Sync + 'static> Query<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: RwSignal::new(None),
            error: RwSignal::new(None),
            busy: RwSignal::new(false),
            turn: RwSignal::new(0),
        }
    }

    pub fn run(self, future: impl Future<Output = anyhow::Result<T>> + 'static) {
        let turn = self.turn.get_untracked() + 1;
        self.turn.set(turn);
        self.busy.set(true);
        zgui::task::spawn_local(async move {
            let result = future.await;
            if self.turn.try_get_untracked() != Some(turn) {
                return;
            }
            self.busy.set(false);
            match result {
                Ok(value) => {
                    self.data.set(Some(value));
                    self.error.set(None);
                }
                Err(error) => self.error.set(Some(format!("{error:#}"))),
            }
        });
    }

    pub fn clear(self) {
        self.turn.update(|turn| *turn += 1);
        self.busy.set(false);
        self.data.set(None);
        self.error.set(None);
    }
}

impl<T: Send + Sync + 'static> Default for Query<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn run_tool(store: Store, request: ToolRequest) -> anyhow::Result<ToolResponse> {
    store.api().post("/api/tools/run", &request).await
}

pub fn save_text(name: &str, text: &str) -> anyhow::Result<PathBuf> {
    let folder = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("no folder to save into"))?;
    let path = folder.join(name);
    std::fs::write(&path, text).map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    Ok(path)
}

pub fn save_for(store: Store, name: &str, text: &str) {
    match save_text(name, text) {
        Ok(path) => store.say(format!("Saved {}", path.display())),
        Err(error) => store.say(format!("{error:#}")),
    }
}

const SI_UP: [(f64, &str); 4] = [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, "")];

#[must_use]
pub fn format_hz(hz: f64) -> String {
    if !hz.is_finite() {
        return "? Hz".to_owned();
    }
    let magnitude = hz.abs();
    let (scale, prefix) = SI_UP
        .iter()
        .find(|(step, _)| magnitude >= *step)
        .copied()
        .unwrap_or((1.0, ""));
    format!("{} {prefix}Hz", trim_zeros(&format!("{:.9}", hz / scale)))
}

fn trim_zeros(fixed: &str) -> String {
    if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        fixed.to_owned()
    }
}

pub fn parse_number(text: &str, min: f64, max: f64) -> Result<f64, String> {
    let trimmed = text.trim();
    let value: f64 = trimmed
        .parse()
        .map_err(|_| format!("{trimmed} is not a number"))?;
    if !value.is_finite() {
        return Err(format!("{trimmed} is not a number"));
    }
    Ok(value.clamp(min, max))
}

#[must_use]
pub fn shown_number(value: f64) -> String {
    let rounded = (value * 1e9).round() / 1e9;
    format!("{rounded}")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Bg,
    Line,
    LineStrong,
    Ink,
    InkDim,
    InkFaint,
    Accent,
    Ok,
    PlotBg,
    PlotGrid,
    PlotInkDim,
    Trace,
    Hold,
}

impl Paint {
    #[must_use]
    pub fn rgb(self) -> (f32, f32, f32) {
        match self {
            Self::Bg => (0.063, 0.068, 0.074),
            Self::Line => (0.2, 0.21, 0.223),
            Self::LineStrong => (0.376, 0.39, 0.409),
            Self::Ink => (0.93, 0.935, 0.943),
            Self::InkDim => (0.66, 0.67, 0.684),
            Self::InkFaint => (0.513, 0.527, 0.545),
            Self::Accent => (0.462, 0.674, 0.988),
            Self::Ok => (0.35, 0.828, 0.55),
            Self::PlotBg => (0.0, 0.0, 0.0),
            Self::PlotGrid => (0.196, 0.196, 0.196),
            Self::PlotInkDim => (0.498, 0.498, 0.498),
            Self::Trace => (0.4, 0.898, 1.0),
            Self::Hold => (1.0, 1.0, 0.0),
        }
    }

    #[must_use]
    pub fn css(self) -> &'static str {
        match self {
            Self::Bg => "var(--bg)",
            Self::Line => "var(--line)",
            Self::LineStrong => "var(--line-strong)",
            Self::Ink => "var(--ink)",
            Self::InkDim => "var(--ink-dim)",
            Self::InkFaint => "var(--ink-faint)",
            Self::Accent => "var(--accent)",
            Self::Ok => "var(--ok)",
            Self::PlotBg => "#000000",
            Self::PlotGrid => "#323232",
            Self::PlotInkDim => "#7f7f7f",
            Self::Trace => "#66e5ff",
            Self::Hold => "#ffff00",
        }
    }

    #[must_use]
    pub fn brush(self, alpha: f32) -> Brush {
        let (red, green, blue) = self.rgb();
        Brush::Solid(Color::srgb(red, green, blue, alpha))
    }
}

pub fn local_point(el: NodeRef, at: Point<CssPx, Css>) -> Option<(f64, f64, f64, f64)> {
    let bounds = el.window_bounds()?;
    let scale = if el.scale() > 0.0 { el.scale() } else { 1.0 };
    let left = bounds.left().0 / scale;
    let top = bounds.top().0 / scale;
    Some((
        f64::from(at.x.0 - left),
        f64::from(at.y.0 - top),
        f64::from(bounds.width().0 / scale),
        f64::from(bounds.height().0 / scale),
    ))
}

pub fn labelled(label: &'static str, body: impl IntoView + 'static) -> impl IntoView {
    view! {
        column(class = "tool-field") {
            text(class = "legend") {{label}}
            {body}
        }
    }
}

pub fn number(
    label: &'static str,
    value: RwSignal<f64>,
    min: f64,
    max: f64,
    unit: &'static str,
) -> impl IntoView {
    let shown = Signal::derive(move || shown_number(value.get()));
    let field = entry(shown, label.to_owned(), false, move |text| {
        value.set(parse_number(&text, min, max)?);
        Ok(())
    });
    view! {
        row(class = "tool-num") {
            {field}
            text(class = "tool-num__unit") {{unit}}
        }
    }
}

pub fn button(
    class: &'static str,
    label: impl Fn() -> String + 'static,
    disabled: impl Fn() -> bool + Send + Sync + 'static,
    press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = class,
            a11y:role = Role::Button,
            tabindex = Focus::Sequential,
            state:disabled = disabled,
            on:click:stop = move |_| press()
        ) {
            {label}
        }
    }
}

pub fn chip(key: impl Into<String>, value: impl Into<String>) -> AnyView {
    let key = key.into();
    let value = value.into();
    AnyView::new(view! {
        row(class = "tool-chip") {
            text(class = "tool-chip__key") {{key}}
            text {{value}}
        }
    })
}

pub fn line(label: impl Into<String>, value: impl Into<String>, accent: bool) -> AnyView {
    let label = label.into();
    let value = value.into();
    AnyView::new(view! {
        row(class = "tool-line") {
            text(class = "tool-line__key") {{label}}
            text(class = "tool-line__value", class:accent = accent) {{value}}
        }
    })
}

pub fn group(title: impl Into<String>, lines: Vec<AnyView>) -> AnyView {
    let title = title.into();
    AnyView::new(view! {
        column(class = "tool-group") {
            text(class = "legend") {{title}}
            column(class = "tool-group__lines") {{lines}}
        }
    })
}

pub fn alert(message: impl Fn() -> Option<String> + 'static) -> impl IntoView {
    move || message().map(|text| AnyView::new(view! { text(class = "tool-alert") {{text}} }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequencies_read_in_their_own_unit_without_trailing_zeros() {
        assert_eq!(format_hz(145_500_000.0), "145.5 MHz");
        assert_eq!(format_hz(1_000.0), "1 kHz");
        assert_eq!(format_hz(12.5), "12.5 Hz");
        assert_eq!(format_hz(2_400_000_000.0), "2.4 GHz");
        assert_eq!(format_hz(f64::NAN), "? Hz");
    }

    #[test]
    fn a_typed_number_is_clamped_and_nonsense_is_refused() {
        assert_eq!(parse_number(" 12.5 ", 0.0, 100.0), Ok(12.5));
        assert_eq!(parse_number("500", 0.0, 100.0), Ok(100.0));
        assert!(parse_number("abc", 0.0, 1.0).is_err());
        assert!(parse_number("inf", 0.0, 1.0).is_err());
    }

    #[test]
    fn a_number_shows_without_float_noise() {
        assert_eq!(shown_number(145.5), "145.5");
        assert_eq!(shown_number(0.1 + 0.2), "0.3");
        assert_eq!(shown_number(101.0), "101");
    }
}
