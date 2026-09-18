use std::sync::Arc;

use zgui::{
    canvas::{Brush, ShapeBuilder, zgui_color::Color},
    elements::kurbo,
    prelude::*,
};

use crate::{format, socket::Spectrum};

const GRID_LINES: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Palette {
    Viridis,
    Classic,
}

impl Palette {
    pub fn label(self) -> &'static str {
        match self {
            Self::Viridis => "VIRIDIS",
            Self::Classic => "CLASSIC",
        }
    }

    pub fn rgb(self, level: f32) -> [f32; 3] {
        let level = level.clamp(0.0, 1.0);
        match self {
            Self::Viridis => ramp(&VIRIDIS, level),
            Self::Classic => ramp(&CLASSIC, level),
        }
    }
}

const VIRIDIS: [[f32; 3]; 9] = [
    [0.267, 0.005, 0.329],
    [0.283, 0.141, 0.458],
    [0.254, 0.265, 0.530],
    [0.208, 0.372, 0.553],
    [0.164, 0.471, 0.558],
    [0.128, 0.567, 0.551],
    [0.135, 0.659, 0.518],
    [0.267, 0.749, 0.441],
    [0.993, 0.906, 0.144],
];

const CLASSIC: [[f32; 3]; 9] = [
    [0.000, 0.000, 0.000],
    [0.031, 0.047, 0.361],
    [0.090, 0.110, 0.678],
    [0.263, 0.141, 0.686],
    [0.545, 0.157, 0.549],
    [0.788, 0.212, 0.333],
    [0.933, 0.365, 0.145],
    [0.988, 0.612, 0.098],
    [1.000, 0.937, 0.635],
];

fn ramp(stops: &[[f32; 3]; 9], level: f32) -> [f32; 3] {
    let scaled = level * (stops.len() - 1) as f32;
    let low = scaled.floor() as usize;
    let high = (low + 1).min(stops.len() - 1);
    let blend = scaled - low as f32;
    let (a, b) = (stops[low], stops[high]);
    [
        a[0] + (b[0] - a[0]) * blend,
        a[1] + (b[1] - a[1]) * blend,
        a[2] + (b[2] - a[2]) * blend,
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub at: f32,
    pub label: String,
}

#[must_use]
pub fn markers(centre_hz: f64, span_hz: f64, channels: &[(String, f64)]) -> Vec<Marker> {
    if span_hz <= 0.0 {
        return Vec::new();
    }
    channels
        .iter()
        .filter_map(|(name, hz)| {
            let at = ((hz - centre_hz) / span_hz + 0.5) as f32;
            (0.0..=1.0).contains(&at).then(|| Marker {
                at,
                label: format!("{name} {}", offset_label(hz - centre_hz)),
            })
        })
        .collect()
}

fn offset_label(delta_hz: f64) -> String {
    let sign = if delta_hz < 0.0 { "-" } else { "+" };
    let magnitude = delta_hz.abs();
    if magnitude >= 1e6 {
        format!("{sign}{:.2} MHz", magnitude / 1e6)
    } else {
        format!("{sign}{:.0} kHz", magnitude / 1e3)
    }
}

pub fn trace(spectrum: Signal<Option<Arc<Spectrum>>>, marks: Signal<Vec<Marker>>) -> impl IntoView {
    zgui::elements::canvas()
        .class("scope__trace")
        .draw(move |cx| {
            let (width, height) = (f64::from(cx.size.width.0), f64::from(cx.size.height.0));
            if width <= 1.0 || height <= 1.0 {
                return;
            }
            grid(cx.scene, width, height);
            for mark in marks.get() {
                let x = width * f64::from(mark.at);
                let mut path = kurbo::BezPath::new();
                path.move_to((x, 0.0));
                path.line_to((x, height));
                cx.scene.push(
                    ShapeBuilder::new(path)
                        .stroke(Brush::Solid(Color::srgb(0.62, 0.65, 0.72, 0.75)), 1.0)
                        .build(),
                );
            }
            let Some(spectrum) = spectrum.get() else {
                return;
            };
            if spectrum.bins.len() < 2 {
                return;
            }
            let mut path = kurbo::BezPath::new();
            let step = width / (spectrum.bins.len() - 1) as f64;
            for (index, bin) in spectrum.bins.iter().enumerate() {
                let y = height * (1.0 - f64::from(*bin) / 255.0);
                let x = index as f64 * step;
                if index == 0 {
                    path.move_to((x, y));
                } else {
                    path.line_to((x, y));
                }
            }
            let mut under = path.clone();
            under.line_to((width, height));
            under.line_to((0.0, height));
            under.close_path();
            cx.scene.push(
                ShapeBuilder::new(under)
                    .fill(Brush::Solid(Color::srgb(0.20, 0.62, 0.72, 0.22)))
                    .build(),
            );
            cx.scene.push(
                ShapeBuilder::new(path)
                    .stroke(Brush::Solid(Color::srgb(0.40, 0.898, 1.0, 1.0)), 1.0)
                    .build(),
            );
        })
        .into_view()
}

fn grid(scene: &mut zgui::canvas::CanvasScene, width: f64, height: f64) {
    let ink = || Brush::Solid(Color::srgb(0.196, 0.196, 0.196, 1.0));
    for line in 1..GRID_LINES {
        let y = height * line as f64 / GRID_LINES as f64;
        let mut path = kurbo::BezPath::new();
        path.move_to((0.0, y));
        path.line_to((width, y));
        scene.push(ShapeBuilder::new(path).stroke(ink(), 1.0).build());
    }
    for line in 1..8 {
        let x = width * line as f64 / 8.0;
        let mut path = kurbo::BezPath::new();
        path.move_to((x, 0.0));
        path.line_to((x, height));
        scene.push(ShapeBuilder::new(path).stroke(ink(), 1.0).build());
    }
}

#[must_use]
pub fn frequency_ticks(centre_hz: f64, span_hz: f64, count: usize) -> Vec<String> {
    if count < 2 || span_hz <= 0.0 {
        return Vec::new();
    }
    (0..count)
        .map(|at| {
            let fraction = at as f64 / (count - 1) as f64 - 0.5;
            format!("{:.2}", (centre_hz + span_hz * fraction) / 1e6)
        })
        .collect()
}

#[must_use]
pub fn decibel_ticks(db_min: f32, db_max: f32) -> Vec<String> {
    (1..GRID_LINES)
        .map(|line| {
            let fraction = 1.0 - line as f32 / GRID_LINES as f32;
            format!("{:.0}", db_min + (db_max - db_min) * fraction)
        })
        .collect()
}

pub fn readout(spectrum: &Spectrum) -> String {
    format!(
        "{}   {}   {} .. {}",
        format::frequency(spectrum.center_hz),
        format::span(f64::from(spectrum.span_hz)),
        format::decibels(spectrum.db_min),
        format::decibels(spectrum.db_max),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_inside_the_span_is_marked_where_it_sits() {
        let marks = markers(100e6, 2e6, &[("NFM".to_owned(), 100.3e6)]);
        assert_eq!(marks.len(), 1);
        assert!((marks[0].at - 0.65).abs() < 1e-6);
        assert_eq!(marks[0].label, "NFM +300 kHz");
    }

    #[test]
    fn a_channel_outside_the_span_is_not_marked_at_the_edge() {
        assert!(markers(100e6, 2e6, &[("AM".to_owned(), 130e6)]).is_empty());
        assert!(markers(100e6, 0.0, &[("AM".to_owned(), 100e6)]).is_empty());
    }

    #[test]
    fn an_offset_reads_in_the_unit_it_belongs_in() {
        assert_eq!(offset_label(-300_000.0), "-300 kHz");
        assert_eq!(offset_label(1_500_000.0), "+1.50 MHz");
        assert_eq!(offset_label(0.0), "+0 kHz");
    }

    #[test]
    fn the_frequency_axis_runs_from_the_low_edge_to_the_high_one() {
        let ticks = frequency_ticks(100e6, 2e6, 5);
        assert_eq!(ticks, ["99.00", "99.50", "100.00", "100.50", "101.00"]);
        assert!(frequency_ticks(100e6, 0.0, 5).is_empty());
        assert!(frequency_ticks(100e6, 2e6, 1).is_empty());
    }

    #[test]
    fn the_decibel_axis_labels_the_lines_the_grid_draws() {
        assert_eq!(decibel_ticks(-100.0, -20.0).len(), GRID_LINES - 1);
        assert_eq!(decibel_ticks(-100.0, -20.0)[0], "-40");
    }

    #[test]
    fn a_palette_runs_from_its_first_stop_to_its_last() {
        assert_eq!(Palette::Viridis.rgb(0.0), VIRIDIS[0]);
        assert_eq!(Palette::Viridis.rgb(1.0), VIRIDIS[8]);
        assert_eq!(Palette::Classic.rgb(2.0), CLASSIC[8]);
        assert_eq!(Palette::Classic.rgb(-1.0), CLASSIC[0]);
    }
}
