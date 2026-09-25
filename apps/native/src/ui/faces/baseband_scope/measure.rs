use std::f32::consts::PI;

use super::frames::{Block, Burst};
use crate::format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Spectrum,
    Constellation,
    Eye,
    Levels,
    States,
    Quality,
    Drift,
}

pub const SIGNAL_VIEWS: [View; 4] = [View::Spectrum, View::Constellation, View::Eye, View::Levels];
pub const SYMBOL_VIEWS: [View; 3] = [View::States, View::Quality, View::Drift];

impl View {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "spectrum",
            Self::Constellation => "constellation",
            Self::Eye => "eye",
            Self::Levels => "levels",
            Self::States => "states",
            Self::Quality => "quality",
            Self::Drift => "drift",
        }
    }

    #[must_use]
    pub fn needs_symbols(self) -> bool {
        SYMBOL_VIEWS.contains(&self)
    }

    #[must_use]
    pub fn shown(self, symbols: bool) -> Self {
        if !symbols && self.needs_symbols() {
            Self::Spectrum
        } else {
            self
        }
    }

    #[must_use]
    pub fn needs_rate(self, symbols: bool, decimate: bool) -> bool {
        self == Self::Eye
            || (!symbols && (self == Self::Levels || (self == Self::Constellation && decimate)))
    }

    #[must_use]
    pub fn decimates(self, symbols: bool) -> bool {
        self == Self::Constellation && !symbols
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub label: &'static str,
    pub value: String,
}

fn row(label: &'static str, value: String) -> Measurement {
    Measurement { label, value }
}

#[must_use]
pub fn baud(symbols_per_second: f32) -> String {
    if symbols_per_second >= 1_000.0 {
        format!("{:.1} kBd", symbols_per_second / 1_000.0)
    } else {
        format!("{symbols_per_second:.0} Bd")
    }
}

#[must_use]
pub fn measurements(
    view: View,
    burst: Option<&Burst>,
    block: Option<&Block>,
    period: f32,
) -> Vec<Measurement> {
    let mut rows = Vec::new();
    if let Some(burst) = burst {
        rows.push(row("Centre", format::frequency(burst.center_hz)));
        rows.push(row("Rate", format::rate(f64::from(burst.sample_rate))));
        let folded = view == View::Eye || (block.is_none() && view != View::Spectrum);
        if folded && period > 0.0 {
            rows.push(row("Sam/sym", format!("{period:.2}")));
        }
    }
    if let Some(block) = block.filter(|_| !matches!(view, View::Spectrum | View::Eye)) {
        let offset = block.freq_error_hz.round();
        rows.extend([
            row("Symbols", baud(block.symbol_rate)),
            row("EVM", format!("{:.1} %", block.evm * 100.0)),
            row(
                "MER",
                if block.mer_db >= 99.0 {
                    "clean".to_owned()
                } else {
                    format!("{:.1} dB", block.mer_db)
                },
            ),
            row("Margin", format!("\u{d7}{:.2}", block.margin)),
            row(
                "Offset",
                format!("{}{offset:.0} Hz", if offset > 0.0 { "+" } else { "" }),
            ),
        ]);
    }
    rows
}

#[must_use]
pub fn waiting(view: View, burst: bool, block: bool) -> Option<&'static str> {
    if view.needs_symbols() {
        return (!block).then_some("This decoder reports no symbols");
    }
    (!burst && !block).then_some("No burst yet")
}

#[must_use]
pub fn discriminator(samples: &[f32], period: f32, offset: i64) -> Vec<f32> {
    let count = samples.len() / 2;
    let step = period.round().max(1.0) as usize;
    let first = offset.max(1) as usize;
    (first..count)
        .step_by(step)
        .map(|i| {
            let (re, im) = (samples[i * 2], samples[i * 2 + 1]);
            let (pr, pi) = (samples[i * 2 - 2], samples[i * 2 - 1]);
            (im * pr - re * pi).atan2(re * pr + im * pi) / PI
        })
        .collect()
}

#[must_use]
pub fn tick_label(value: f32) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let unit = 10f64.powi(magnitude - 1);
    let rounded = (f64::from(value) / unit).round() * unit;
    let decimals = (1 - magnitude).max(0) as usize;
    let text = format!("{rounded:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::super::frames::tests::block;
    use super::*;

    fn burst() -> Burst {
        Burst {
            center_hz: 145.8e6,
            sample_rate: 48_000.0,
            samples: vec![1.0, 0.0],
        }
    }

    fn labels(rows: &[Measurement]) -> Vec<&'static str> {
        rows.iter().map(|row| row.label).collect()
    }

    fn value<'a>(rows: &'a [Measurement], label: &str) -> Option<&'a str> {
        rows.iter()
            .find(|row| row.label == label)
            .map(|row| row.value.as_str())
    }

    #[test]
    fn keeps_the_spectrum_to_the_burst_it_was_drawn_from() {
        assert_eq!(
            labels(&measurements(
                View::Spectrum,
                Some(&burst()),
                Some(&block()),
                10.0
            )),
            ["Centre", "Rate"]
        );
    }

    #[test]
    fn adds_the_fold_for_the_eye_but_not_the_decoder_figures() {
        let rows = measurements(View::Eye, Some(&burst()), Some(&block()), 10.0);
        assert_eq!(labels(&rows), ["Centre", "Rate", "Sam/sym"]);
        assert_eq!(value(&rows, "Sam/sym"), Some("10.00"));
    }

    #[test]
    fn reads_the_decoder_figures_on_the_views_the_symbols_feed() {
        for view in [
            View::Constellation,
            View::Levels,
            View::States,
            View::Quality,
            View::Drift,
        ] {
            let rows = measurements(view, Some(&burst()), Some(&block()), 10.0);
            assert_eq!(value(&rows, "EVM"), Some("12.5 %"));
            assert_eq!(value(&rows, "MER"), Some("18.1 dB"));
            assert_eq!(value(&rows, "Offset"), Some("-12 Hz"));
        }
    }

    #[test]
    fn calls_a_perfect_burst_clean_and_signs_a_positive_offset() {
        let clean = Block {
            mer_db: 99.0,
            freq_error_hz: 7.4,
            ..block()
        };
        let rows = measurements(View::Levels, Some(&burst()), Some(&clean), 10.0);
        assert_eq!(value(&rows, "MER"), Some("clean"));
        assert_eq!(value(&rows, "Offset"), Some("+7 Hz"));
    }

    #[test]
    fn folds_at_the_set_rate_when_no_decoder_reports_symbols() {
        assert!(
            labels(&measurements(
                View::Constellation,
                Some(&burst()),
                None,
                10.0
            ))
            .contains(&"Sam/sym")
        );
        assert!(measurements(View::Spectrum, None, None, 0.0).is_empty());
    }

    #[test]
    fn says_a_trend_needs_a_decoder_that_reports_symbols() {
        for view in [View::Quality, View::Drift, View::States] {
            assert!(
                waiting(view, false, false).is_some_and(|hint| hint.contains("reports no symbols"))
            );
            assert_eq!(waiting(view, false, true), None);
        }
        assert_eq!(waiting(View::Spectrum, false, false), Some("No burst yet"));
        assert_eq!(
            waiting(View::Constellation, false, false),
            Some("No burst yet")
        );
        assert_eq!(waiting(View::Levels, false, true), None);
    }

    #[test]
    fn a_symbol_view_falls_back_to_the_spectrum_without_symbols() {
        assert_eq!(View::Quality.shown(false), View::Spectrum);
        assert_eq!(View::Quality.shown(true), View::Quality);
        assert_eq!(View::Eye.shown(false), View::Eye);
        assert!(View::Eye.needs_rate(true, false));
        assert!(View::Constellation.needs_rate(false, true));
        assert!(!View::Constellation.needs_rate(true, true));
        assert!(View::Constellation.decimates(false));
    }

    #[test]
    fn reads_a_steady_rotation_as_a_steady_level() {
        let wave: Vec<f32> = (0..64)
            .flat_map(|i| {
                let phase = PI / 4.0 * i as f32;
                [phase.cos(), phase.sin()]
            })
            .collect();
        let rail = discriminator(&wave, 4.0, 1);
        assert!(rail.len() > 4);
        assert!(rail.iter().all(|value| (value - 0.25).abs() < 1e-5));
    }

    #[test]
    fn takes_one_reading_per_symbol_period() {
        let wave = vec![0.0; 128];
        assert_eq!(discriminator(&wave, 8.0, 0).len(), 8);
        assert_eq!(discriminator(&wave, 4.0, 0).len(), 16);
    }

    #[test]
    fn rounds_a_tick_to_two_significant_digits() {
        assert_eq!(tick_label(0.3536), "0.35");
        assert_eq!(tick_label(-1234.0), "-1200");
        assert_eq!(tick_label(0.5), "0.5");
    }

    #[test]
    fn a_rate_reads_in_baud() {
        assert_eq!(baud(4800.0), "4.8 kBd");
        assert_eq!(baud(1200.0), "1.2 kBd");
        assert_eq!(baud(300.0), "300 Bd");
    }
}
