use num_complex::Complex;
use sdrmm_dsp::spectrum::SpectrumAnalyzer;
use sdrmm_wire::frame::SymbolPlane;

use super::{
    frames::{Block, Burst},
    grid::{
        BASEBAND_DECAY, Eye, Grid, Trend, add_constellation, add_eye, eye_scale, iq_scale,
        samples_per_symbol, symbol_histogram, symbol_phase,
    },
    measure::{View, discriminator, tick_label},
};
use crate::ui::{
    kit_raster::{PLOT_BG, PLOT_GRID, PLOT_HOLD, PLOT_INK, PLOT_TRACE, Pen, Raster, Rgba, Scene},
    faces::scope::colormap::Colormap,
};

pub const GRID: usize = 320;
pub const FFT_SIZE: usize = 2048;
pub const SPECTRUM_RANGE_DB: f32 = 90.0;
pub const TREND_POINTS: usize = 240;
const SPECTRUM_STEP_DB: f32 = 20.0;
const MINOR_TICKS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub view: View,
    pub eye: Eye,
    pub symbol_rate: f32,
    pub decimate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            view: View::Spectrum,
            eye: Eye::Frequency,
            symbol_rate: 4800.0,
            decimate: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frac {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Frac {
    fn px(self, raster: &Raster) -> (f32, f32, f32, f32) {
        let (w, h) = (raster.width as f32, raster.height as f32);
        let (x0, y0) = ((self.left * w).round(), (self.top * h).round());
        (
            x0,
            y0,
            (self.right * w).round() - x0,
            (self.bottom * h).round() - y0,
        )
    }
}

pub const SPECTRUM_BOX: Frac = Frac {
    left: 0.0,
    top: 0.02,
    right: 1.0,
    bottom: 0.88,
};
pub const SCATTER_BOX: Frac = Frac {
    left: 0.04,
    top: 0.04,
    right: 0.96,
    bottom: 0.96,
};
pub const HISTOGRAM_BOX: Frac = Frac {
    left: 0.02,
    top: 0.08,
    right: 0.98,
    bottom: 0.86,
};
pub const TREND_BOX: Frac = Frac {
    left: 0.14,
    top: 0.08,
    right: 0.98,
    bottom: 0.86,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Left,
    Centre,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub anchor: Anchor,
}

fn label(x: f32, y: f32, text: impl Into<String>, anchor: Anchor) -> Label {
    Label {
        x,
        y,
        text: text.into(),
        anchor,
    }
}

pub struct Trends {
    pub mer: Trend,
    pub margin: Trend,
    pub drift: Trend,
}

pub struct Scope {
    pub settings: Settings,
    pub burst: Option<Burst>,
    pub block: Option<Block>,
    pub trends: Trends,
    grid: Grid,
    scale: f32,
    analyzer: SpectrumAnalyzer,
    input: Vec<Complex<f32>>,
    db: Vec<f32>,
    top: f32,
    lut: Vec<Rgba>,
    stamp: u64,
}

impl Scope {
    #[must_use]
    pub fn new(palette: Colormap) -> Self {
        Self {
            settings: Settings::default(),
            burst: None,
            block: None,
            trends: Trends {
                mer: Trend::new(TREND_POINTS),
                margin: Trend::new(TREND_POINTS),
                drift: Trend::new(TREND_POINTS),
            },
            grid: Grid::new(GRID, GRID),
            scale: 1.0,
            analyzer: SpectrumAnalyzer::new(FFT_SIZE),
            input: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            db: vec![0.0; FFT_SIZE],
            top: 0.0,
            lut: lut(palette),
            stamp: 0,
        }
    }

    #[must_use]
    pub fn shown(&self) -> View {
        self.settings.view.shown(self.block.is_some())
    }

    pub fn configure(&mut self, settings: Settings) {
        self.settings = settings;
        self.grid.clear();
        if self.shown() == View::Spectrum {
            self.analyse();
        }
        self.stamp += 1;
    }

    pub fn take_burst(&mut self, burst: Burst) {
        let settings = self.settings;
        let period = samples_per_symbol(burst.sample_rate, settings.symbol_rate);
        match settings.view {
            View::Constellation => {
                self.grid.decay(BASEBAND_DECAY);
                if self.block.is_none() {
                    let step = if settings.decimate {
                        period.round()
                    } else {
                        1.0
                    };
                    let offset = if settings.decimate {
                        symbol_phase(&burst.samples, period)
                    } else {
                        0
                    };
                    self.scale = iq_scale(&burst.samples);
                    add_constellation(&mut self.grid, &burst.samples, self.scale, step, offset);
                }
            }
            View::Eye => {
                self.grid.decay(BASEBAND_DECAY);
                let scale = eye_scale(&burst.samples, settings.eye);
                add_eye(&mut self.grid, &burst.samples, period, settings.eye, scale);
            }
            _ => {}
        }
        self.burst = Some(burst);
        if self.shown() == View::Spectrum {
            self.analyse();
        }
        self.stamp += 1;
    }

    pub fn take_block(&mut self, block: Block) {
        self.trends.mer.push(block.mer_db);
        self.trends.margin.push(block.margin);
        self.trends.drift.push(block.freq_error_hz);
        if self.settings.view == View::Constellation {
            self.grid.decay(BASEBAND_DECAY);
            self.scale = block.reference_scale();
            add_constellation(&mut self.grid, &block.paired(), self.scale, 1.0, 0);
        }
        self.block = Some(block);
        self.stamp += 1;
    }

    #[must_use]
    pub fn square(&self) -> bool {
        self.shown() == View::Constellation
    }

    #[must_use]
    pub fn labels(&self) -> Vec<Label> {
        match self.shown() {
            View::Spectrum => self.burst.as_ref().map_or_else(Vec::new, |burst| {
                let mut labels = spectrum_labels(burst);
                labels.extend(spectrum_db_labels(self.top));
                labels
            }),
            View::Constellation => constellation_labels(self.scale),
            View::Eye => vec![
                label(
                    SCATTER_BOX.left + 0.01,
                    SCATTER_BOX.top + 0.03,
                    self.settings.eye.label(),
                    Anchor::Left,
                ),
                label(
                    SCATTER_BOX.right - 0.01,
                    SCATTER_BOX.bottom - 0.04,
                    "2 symbols",
                    Anchor::Right,
                ),
            ],
            View::Levels => self.levels_labels(),
            View::Quality => trend_labels(
                &[&self.trends.mer, &self.trends.margin],
                &["MER dB", "margin"],
                "per block",
                false,
            ),
            View::Drift => trend_labels(&[&self.trends.drift], &["carrier"], "Hz", true),
            View::States => Vec::new(),
        }
    }

    fn levels_labels(&self) -> Vec<Label> {
        let mut labels = vec![
            label(
                HISTOGRAM_BOX.left + 0.01,
                HISTOGRAM_BOX.top + 0.03,
                "share",
                Anchor::Left,
            ),
            label(
                HISTOGRAM_BOX.right - 0.01,
                HISTOGRAM_BOX.top + 0.03,
                "level",
                Anchor::Right,
            ),
        ];
        if let Some(block) = &self.block {
            let scale = block.reference_scale();
            for level in reference_levels(block) {
                let x = histogram_x(level, scale);
                if (0.0..=1.0).contains(&x) {
                    let at = HISTOGRAM_BOX.left + x * (HISTOGRAM_BOX.right - HISTOGRAM_BOX.left);
                    labels.push(label(
                        at,
                        HISTOGRAM_BOX.bottom + 0.06,
                        level_text(level),
                        Anchor::Centre,
                    ));
                }
            }
        }
        labels
    }

    fn analyse(&mut self) {
        let Some(burst) = &self.burst else {
            return;
        };
        let pairs = burst
            .samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[re, im]| Complex::new(*re, *im))
            .chain(std::iter::repeat(Complex::new(0.0, 0.0)));
        for (slot, pair) in self.input.iter_mut().zip(pairs) {
            *slot = pair;
        }
        self.analyzer.power_db(&self.input, &mut self.db);
        self.top = spectrum_top(&self.db);
    }

    fn paint_spectrum(&self, raster: &mut Raster) {
        if self.burst.is_none() {
            return;
        }
        let top = self.top;
        let (x0, y0, w, h) = SPECTRUM_BOX.px(raster);
        let grid = Pen::new(PLOT_GRID, 1.0, 1.0);
        for step in 1..=4 {
            let y = y0 + h * (step as f32 * SPECTRUM_STEP_DB / SPECTRUM_RANGE_DB);
            raster.line((x0, y), (x0 + w, y), grid);
        }
        for column in 1..8 {
            let x = x0 + w * column as f32 / 8.0;
            raster.line((x, y0), (x, y0 + h), grid);
        }
        let points = trace_points(&self.db, w as usize, top);
        let foot = y0 + h;
        let mut last = None;
        for (column, level) in points.iter().enumerate() {
            let x = x0 + column as f32;
            let y = y0 + h * (1.0 - level);
            raster.rect(x, y, x + 1.0, foot, PLOT_TRACE, 0.22);
            if let Some(previous) = last {
                raster.line(previous, (x, y), Pen::new(PLOT_TRACE, 1.0, raster.scale));
            }
            last = Some((x, y));
        }
    }

    fn paint_scatter(&self, raster: &mut Raster) {
        let constellation = self.shown() == View::Constellation;
        if constellation {
            let side = raster.width.min(raster.height);
            raster.resize(side, side);
            raster.fill(PLOT_BG);
            raster.aspect = Some(1.0);
        }
        let (x0, y0, w, h) = SCATTER_BOX.px(raster);
        graticule(raster, (x0, y0, w, h), 8, if constellation { 8 } else { 6 });
        if constellation {
            let pen = Pen::new(PLOT_INK, 0.35, 1.0).dashed(2.0 * raster.scale, 4.0 * raster.scale);
            raster.circle((x0 + w / 2.0, y0 + h / 2.0), w / 2.0, pen);
        }
        self.recolour(raster, (x0, y0, w, h));
    }

    fn recolour(&self, raster: &mut Raster, (x0, y0, w, h): (f32, f32, f32, f32)) {
        if w < 1.0 || h < 1.0 {
            return;
        }
        let (grid_w, grid_h) = (self.grid.width, self.grid.height);
        for py in 0..h as usize {
            let gy = (py * grid_h / h as usize).min(grid_h - 1);
            for px in 0..w as usize {
                let gx = (px * grid_w / w as usize).min(grid_w - 1);
                let value = self.grid.cells[gy * grid_w + gx];
                if value <= 0.0 {
                    continue;
                }
                let colour = self.lut[((value * 255.0).round() as usize).min(255)];
                let alpha = (40.0 + value * 215.0).min(255.0) / 255.0;
                raster.blend(x0 as i64 + px as i64, y0 as i64 + py as i64, colour, alpha);
            }
        }
    }

    fn paint_levels(&self, raster: &mut Raster) {
        let (bins, reference, scale) = match (&self.block, &self.burst) {
            (Some(block), _) => {
                let scale = block.reference_scale();
                let stride = if block.plane == SymbolPlane::Complex {
                    2
                } else {
                    1
                };
                (
                    symbol_histogram(&block.symbols, stride, scale),
                    reference_levels(block),
                    scale,
                )
            }
            (None, Some(burst)) => {
                let period = samples_per_symbol(burst.sample_rate, self.settings.symbol_rate);
                let rail =
                    discriminator(&burst.samples, period, symbol_phase(&burst.samples, period));
                (symbol_histogram(&rail, 1, 1.0), Vec::new(), 1.0)
            }
            (None, None) => return,
        };
        let (x0, y0, w, h) = HISTOGRAM_BOX.px(raster);
        graticule(raster, (x0, y0, w, h), 8, 4);
        let step = w / bins.len() as f32;
        let foot = y0 + h;
        for (index, value) in bins.iter().enumerate() {
            if *value > 0.0 {
                let left = x0 + index as f32 * step;
                raster.rect(
                    left,
                    foot - value * h * 0.92,
                    left + (step - 0.5).max(1.0),
                    foot,
                    PLOT_TRACE,
                    0.85,
                );
            }
        }
        let dash = Pen::new(PLOT_HOLD, 0.7, 1.0).dashed(3.0 * raster.scale, 3.0 * raster.scale);
        for level in reference {
            let x = x0 + histogram_x(level, scale) * w;
            if (x0..=x0 + w).contains(&x) {
                raster.line((x, y0), (x, foot), dash);
            }
        }
    }

    fn paint_trend(&self, raster: &mut Raster, series: &[(&Trend, Rgba)], zero: bool) {
        let (x0, y0, w, h) = TREND_BOX.px(raster);
        graticule(raster, (x0, y0, w, h), 10, 4);
        let trends: Vec<&Trend> = series.iter().map(|(trend, _)| *trend).collect();
        let Some((min, max, longest)) = trend_span(&trends, zero) else {
            return;
        };
        let pen_width = 1.5 * raster.scale;
        for (trend, colour) in series {
            if trend.len() < 2 {
                continue;
            }
            let mut last = None;
            for index in 0..trend.len() {
                let x = x0 + index as f32 / (longest - 1).max(1) as f32 * w;
                let y = y0 + (1.0 - (trend.sample(index) - min) / (max - min)) * h;
                if let Some(previous) = last {
                    raster.line(previous, (x, y), Pen::new(*colour, 1.0, pen_width));
                }
                last = Some((x, y));
            }
        }
        let mut at = x0 + 6.0 * raster.scale;
        for (_, colour) in series {
            let y = y0 + h * 0.04;
            raster.rect(
                at,
                y,
                at + 8.0 * raster.scale,
                y + 2.0 * raster.scale,
                *colour,
                1.0,
            );
            at += w * 0.22;
        }
    }
}

impl Scene for Scope {
    fn stamp(&self) -> u64 {
        self.stamp
    }

    fn paint(&mut self, raster: &mut Raster) {
        raster.fill(PLOT_BG);
        match self.shown() {
            View::Spectrum => self.paint_spectrum(raster),
            View::Constellation | View::Eye => self.paint_scatter(raster),
            View::Levels => self.paint_levels(raster),
            View::Quality => self.paint_trend(
                raster,
                &[
                    (&self.trends.mer, PLOT_TRACE),
                    (&self.trends.margin, PLOT_HOLD),
                ],
                false,
            ),
            View::Drift => self.paint_trend(raster, &[(&self.trends.drift, PLOT_TRACE)], true),
            View::States => {}
        }
    }
}

fn lut(palette: Colormap) -> Vec<Rgba> {
    (0..256)
        .map(|step| {
            let [r, g, b] = palette.sample(f64::from(step as f32 / 255.0));
            [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
        })
        .collect()
}

fn graticule(
    raster: &mut Raster,
    (x0, y0, w, h): (f32, f32, f32, f32),
    columns: usize,
    rows: usize,
) {
    let grid = Pen::new(PLOT_GRID, 0.55, 1.0);
    for column in 1..columns {
        let x = (x0 + w * column as f32 / columns as f32).round();
        raster.line((x, y0), (x, y0 + h), grid);
    }
    for row in 1..rows {
        let y = (y0 + h * row as f32 / rows as f32).round();
        raster.line((x0, y), (x0 + w, y), grid);
    }
    let edge = Pen::new(PLOT_GRID, 1.0, 1.0);
    raster.line((x0, y0), (x0 + w, y0), edge);
    raster.line((x0, y0 + h), (x0 + w, y0 + h), edge);
    raster.line((x0, y0), (x0, y0 + h), edge);
    raster.line((x0 + w, y0), (x0 + w, y0 + h), edge);
    let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
    let tick = Pen::new(PLOT_INK, 0.45, 1.0);
    let scale = raster.scale;
    let reach = |index: usize| if index.is_multiple_of(MINOR_TICKS) { 4.0 } else { 2.0 } * scale;
    for index in 1..columns * MINOR_TICKS {
        let x = (x0 + w * index as f32 / (columns * MINOR_TICKS) as f32).round();
        let size = reach(index);
        raster.line((x, cy - size), (x, cy + size), tick);
    }
    for index in 1..rows * MINOR_TICKS {
        let y = (y0 + h * index as f32 / (rows * MINOR_TICKS) as f32).round();
        let size = reach(index);
        raster.line((cx - size, y), (cx + size, y), tick);
    }
}

#[must_use]
pub fn spectrum_top(db: &[f32]) -> f32 {
    let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if peak.is_finite() {
        (peak / 10.0).ceil() * 10.0
    } else {
        0.0
    }
}

#[must_use]
pub fn trace_points(db: &[f32], columns: usize, top: f32) -> Vec<f32> {
    if db.is_empty() || columns == 0 {
        return Vec::new();
    }
    (0..columns)
        .map(|column| {
            let from = column * db.len() / columns;
            let to = ((column + 1) * db.len() / columns)
                .max(from + 1)
                .min(db.len());
            let peak = db[from..to]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            ((peak - (top - SPECTRUM_RANGE_DB)) / SPECTRUM_RANGE_DB).clamp(0.0, 1.0)
        })
        .collect()
}

fn spectrum_labels(burst: &Burst) -> Vec<Label> {
    let mut labels = vec![
        label(0.01, SPECTRUM_BOX.top + 0.03, "dBFS", Anchor::Left),
        label(0.99, SPECTRUM_BOX.bottom + 0.06, "kHz", Anchor::Right),
    ];
    for column in 1..8 {
        let fraction = column as f32 / 8.0;
        let khz = (fraction - 0.5) * burst.sample_rate / 1_000.0;
        labels.push(label(
            fraction,
            SPECTRUM_BOX.bottom + 0.06,
            format!("{khz:+.1}"),
            Anchor::Centre,
        ));
    }
    labels
}

fn spectrum_db_labels(top: f32) -> Vec<Label> {
    (1..=4)
        .map(|step| {
            let db = top - step as f32 * SPECTRUM_STEP_DB;
            let y = SPECTRUM_BOX.top
                + (SPECTRUM_BOX.bottom - SPECTRUM_BOX.top)
                    * (step as f32 * SPECTRUM_STEP_DB / SPECTRUM_RANGE_DB);
            label(0.01, y - 0.03, format!("{db:.0}"), Anchor::Left)
        })
        .collect()
}

fn constellation_labels(scale: f32) -> Vec<Label> {
    let half = scale / 2.0;
    let span = |fraction: f32| SCATTER_BOX.left + fraction * (SCATTER_BOX.right - SCATTER_BOX.left);
    let rise = |fraction: f32| SCATTER_BOX.top + fraction * (SCATTER_BOX.bottom - SCATTER_BOX.top);
    vec![
        label(span(1.0) - 0.01, rise(0.5) - 0.04, "I", Anchor::Right),
        label(span(0.5) + 0.02, rise(0.0) + 0.03, "Q", Anchor::Left),
        label(
            span(0.25),
            rise(1.0) - 0.04,
            tick_label(-half),
            Anchor::Centre,
        ),
        label(
            span(0.75),
            rise(1.0) - 0.04,
            tick_label(half),
            Anchor::Centre,
        ),
        label(span(0.0) + 0.01, rise(0.25), tick_label(half), Anchor::Left),
        label(
            span(0.0) + 0.01,
            rise(0.75),
            tick_label(-half),
            Anchor::Left,
        ),
    ]
}

#[must_use]
pub fn reference_levels(block: &Block) -> Vec<f32> {
    match block.plane {
        SymbolPlane::Level => block.reference.clone(),
        SymbolPlane::Complex => block.reference.iter().step_by(2).copied().collect(),
    }
}

#[must_use]
pub fn histogram_x(level: f32, scale: f32) -> f32 {
    let span = if scale > 0.0 { scale } else { 1.0 };
    (level / span + 1.0) / 2.0
}

#[must_use]
pub fn level_text(level: f32) -> String {
    if level.fract() == 0.0 {
        format!("{level:.0}")
    } else {
        format!("{level:.2}")
    }
}

#[must_use]
pub fn trend_span(trends: &[&Trend], zero: bool) -> Option<(f32, f32, usize)> {
    let filled: Vec<&&Trend> = trends.iter().filter(|trend| trend.len() > 0).collect();
    let longest = filled.iter().map(|trend| trend.len()).max()?;
    let (mut min, mut max) = filled.iter().map(|trend| trend.range()).fold(
        (f32::INFINITY, f32::NEG_INFINITY),
        |(min, max), (low, high)| (min.min(low), max.max(high)),
    );
    if zero {
        let reach = min.abs().max(max.abs()).max(1.0);
        (min, max) = (-reach, reach);
    }
    let pad = ((max - min) * 0.1).max(0.5);
    Some((min - pad, max + pad, longest))
}

fn trend_labels(trends: &[&Trend], names: &[&str], unit: &str, zero: bool) -> Vec<Label> {
    let mut labels = vec![
        label(
            TREND_BOX.right - 0.01,
            TREND_BOX.top + 0.03,
            unit,
            Anchor::Right,
        ),
        label(
            TREND_BOX.left,
            TREND_BOX.bottom + 0.06,
            "older",
            Anchor::Left,
        ),
        label(
            TREND_BOX.right,
            TREND_BOX.bottom + 0.06,
            "now",
            Anchor::Right,
        ),
    ];
    let width = TREND_BOX.right - TREND_BOX.left;
    for (index, name) in names.iter().enumerate() {
        labels.push(label(
            TREND_BOX.left + 0.06 + width * 0.22 * index as f32,
            TREND_BOX.top + 0.03,
            *name,
            Anchor::Left,
        ));
    }
    let Some((min, max, _)) = trend_span(trends, zero) else {
        return labels;
    };
    let decimals = if (max - min).abs() < 10.0 { 1 } else { 0 };
    for step in 0..=4 {
        let value = min + (max - min) * step as f32 / 4.0;
        let y = TREND_BOX.bottom - (TREND_BOX.bottom - TREND_BOX.top) * step as f32 / 4.0;
        labels.push(label(
            TREND_BOX.left - 0.01,
            y,
            format!("{value:.decimals$}"),
            Anchor::Right,
        ));
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::super::frames::tests::block;
    use super::*;

    fn burst(samples: Vec<f32>) -> Burst {
        Burst {
            center_hz: 100e6,
            sample_rate: 48_000.0,
            samples,
        }
    }

    #[test]
    fn a_burst_in_constellation_view_lights_the_grid_and_asks_for_a_repaint() {
        let mut scope = Scope::new(Colormap::Viridis);
        scope.configure(Settings {
            view: View::Constellation,
            decimate: false,
            ..Settings::default()
        });
        let before = scope.stamp();
        scope.take_burst(burst(vec![1.0, 0.0, -1.0, 0.0]));
        assert!(scope.stamp() > before);
        assert!(scope.grid.cells.iter().any(|cell| *cell > 0.0));
        scope.configure(scope.settings);
        assert!(scope.grid.cells.iter().all(|cell| *cell == 0.0));
    }

    #[test]
    fn a_block_feeds_the_trends_and_unlocks_the_symbol_views() {
        let mut scope = Scope::new(Colormap::Viridis);
        scope.configure(Settings {
            view: View::Quality,
            ..Settings::default()
        });
        assert_eq!(scope.shown(), View::Spectrum);
        scope.take_block(block());
        assert_eq!(scope.shown(), View::Quality);
        assert_eq!(scope.trends.mer.len(), 1);
        assert_eq!(scope.trends.drift.sample(0), -12.0);
    }

    #[test]
    fn the_constellation_paints_square_and_the_spectrum_fills_the_plot() {
        let mut scope = Scope::new(Colormap::Viridis);
        scope.take_burst(burst((0..256).map(|i| (i as f32 * 0.3).sin()).collect()));
        let mut raster = Raster::sized(200, 100);
        scope.paint(&mut raster);
        assert_eq!((raster.width, raster.height), (200, 100));
        assert!(
            raster
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| *pixel != PLOT_BG)
        );
        scope.configure(Settings {
            view: View::Constellation,
            ..Settings::default()
        });
        let mut square = Raster::sized(200, 100);
        scope.paint(&mut square);
        assert_eq!((square.width, square.height), (100, 100));
        assert_eq!(square.aspect, Some(1.0));
        assert!(scope.square());
    }

    #[test]
    fn the_spectrum_window_sits_below_the_rounded_peak() {
        assert_eq!(spectrum_top(&[-43.0, -71.0]), -40.0);
        assert_eq!(spectrum_top(&[]), 0.0);
        let points = trace_points(&[-40.0, -130.0, -85.0, -85.0], 2, -40.0);
        assert_eq!(points, vec![1.0, 0.5]);
    }

    #[test]
    fn a_trend_span_pads_its_range_and_centres_on_zero_when_asked() {
        let mut trend = Trend::new(4);
        trend.push(2.0);
        trend.push(6.0);
        let (min, max, longest) = trend_span(&[&trend], false).expect("a span");
        assert_eq!(longest, 2);
        assert!(min < 2.0 && max > 6.0);
        let (min, max, _) = trend_span(&[&trend], true).expect("a span");
        assert!((min + max).abs() < 1e-5);
        assert_eq!(trend_span(&[&Trend::new(4)], false), None);
    }

    #[test]
    fn reference_levels_are_labelled_where_the_histogram_puts_them() {
        assert_eq!(reference_levels(&block()), vec![1.0, 3.0, -1.0, -3.0]);
        assert!((histogram_x(0.0, 4.2) - 0.5).abs() < 1e-6);
        assert_eq!(level_text(3.0), "3");
        assert_eq!(level_text(0.33), "0.33");
    }

    #[test]
    fn every_view_but_the_states_carries_its_labels() {
        let mut scope = Scope::new(Colormap::Viridis);
        scope.take_burst(burst(vec![1.0, 0.0]));
        scope.take_block(block());
        for view in [
            View::Spectrum,
            View::Constellation,
            View::Eye,
            View::Levels,
            View::Quality,
            View::Drift,
        ] {
            scope.configure(Settings {
                view,
                ..Settings::default()
            });
            assert!(!scope.labels().is_empty(), "{view:?}");
        }
        scope.configure(Settings {
            view: View::States,
            ..Settings::default()
        });
        assert!(scope.labels().is_empty());
    }
}
