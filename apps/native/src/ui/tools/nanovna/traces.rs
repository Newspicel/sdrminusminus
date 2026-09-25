use super::{analysis::PointReadout, rf::format_si};
use crate::ui::tools::kit::Paint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartId {
    Magnitude,
    Vswr,
    Phase,
    Impedance,
    Delay,
    Smith,
}

pub struct Series {
    pub label: &'static str,
    pub ink: Paint,
    pub value: fn(&PointReadout) -> f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Domain {
    pub low: f64,
    pub high: f64,
}

pub struct ChartView {
    pub id: ChartId,
    pub label: &'static str,
    pub unit: &'static str,
    pub series: &'static [Series],
    pub domain: fn(&[f64]) -> Domain,
    pub format: fn(f64) -> String,
}

const MAGNITUDE_FLOOR_DB: f64 = 60.0;

pub const CHART_VIEWS: [ChartView; 6] = [
    ChartView {
        id: ChartId::Magnitude,
        label: "Magnitude",
        unit: "dB",
        series: &[
            Series {
                label: "S11",
                ink: Paint::Trace,
                value: |row| row.s11_db,
            },
            Series {
                label: "S21",
                ink: Paint::Hold,
                value: |row| row.s21_db,
            },
        ],
        domain: |values| floored(snapped(values, 10.0, 20.0), MAGNITUDE_FLOOR_DB),
        format: |value| format!("{value:.0}"),
    },
    ChartView {
        id: ChartId::Vswr,
        label: "VSWR",
        unit: ":1",
        series: &[Series {
            label: "VSWR",
            ink: Paint::Trace,
            value: |row| row.vswr,
        }],
        domain: |values| Domain {
            low: 1.0,
            high: finite_max(values, 3.0).ceil().clamp(2.0, 20.0),
        },
        format: |value| format!("{value:.1}"),
    },
    ChartView {
        id: ChartId::Phase,
        label: "Phase",
        unit: "\u{b0}",
        series: &[
            Series {
                label: "S11",
                ink: Paint::Trace,
                value: |row| row.s11_phase_deg,
            },
            Series {
                label: "S21",
                ink: Paint::Hold,
                value: |row| row.s21_phase_deg,
            },
        ],
        domain: |_| Domain {
            low: -180.0,
            high: 180.0,
        },
        format: |value| format!("{value:.0}"),
    },
    ChartView {
        id: ChartId::Impedance,
        label: "Impedance",
        unit: "\u{3a9}",
        series: &[
            Series {
                label: "R",
                ink: Paint::Trace,
                value: |row| row.impedance.map_or(f64::NAN, |z| z.re),
            },
            Series {
                label: "X",
                ink: Paint::Hold,
                value: |row| row.impedance.map_or(f64::NAN, |z| z.im),
            },
        ],
        domain: |values| {
            let mut with_zero = values.to_vec();
            with_zero.push(0.0);
            snapped(&with_zero, 25.0, 50.0)
        },
        format: |value| format!("{value:.0}"),
    },
    ChartView {
        id: ChartId::Delay,
        label: "Group delay",
        unit: "s",
        series: &[Series {
            label: "S21 delay",
            ink: Paint::Trace,
            value: |row| row.group_delay_s,
        }],
        domain: padded,
        format: |value| format_si(value, "s", 1),
    },
    ChartView {
        id: ChartId::Smith,
        label: "Smith",
        unit: "",
        series: &[Series {
            label: "S11",
            ink: Paint::Trace,
            value: |row| row.s11_linear,
        }],
        domain: |_| Domain {
            low: -1.0,
            high: 1.0,
        },
        format: |value| format!("{value:.2}"),
    },
];

#[must_use]
pub fn chart_view(id: ChartId) -> &'static ChartView {
    CHART_VIEWS
        .iter()
        .find(|view| view.id == id)
        .unwrap_or(&CHART_VIEWS[0])
}

#[must_use]
pub fn series_values(view: &ChartView, rows: &[PointReadout]) -> Vec<f64> {
    view.series
        .iter()
        .flat_map(|series| rows.iter().map(series.value))
        .collect()
}

fn finite(values: &[f64]) -> impl Iterator<Item = f64> + '_ {
    values.iter().copied().filter(|value| value.is_finite())
}

fn snapped(values: &[f64], step: f64, minimum_span: f64) -> Domain {
    let low = finite(values).fold(f64::INFINITY, f64::min);
    let high = finite(values).fold(f64::NEG_INFINITY, f64::max);
    if low > high {
        return Domain {
            low: -step,
            high: step,
        };
    }
    let low = (low / step).floor() * step;
    let high = (high / step).ceil() * step;
    if high - low >= minimum_span {
        return Domain { low, high };
    }
    let middle = f64::midpoint(low, high);
    Domain {
        low: middle - minimum_span / 2.0,
        high: middle + minimum_span / 2.0,
    }
}

fn floored(domain: Domain, depth: f64) -> Domain {
    Domain {
        low: domain.low.max(domain.high - depth),
        high: domain.high,
    }
}

fn padded(values: &[f64]) -> Domain {
    let low = finite(values).fold(f64::INFINITY, f64::min);
    let high = finite(values).fold(f64::NEG_INFINITY, f64::max);
    if low > high {
        return Domain {
            low: -1.0,
            high: 1.0,
        };
    }
    let spread = (high - low) * 0.1;
    let margin = if spread != 0.0 {
        spread
    } else if high != 0.0 {
        high.abs() * 0.1
    } else {
        1.0
    };
    Domain {
        low: low - margin,
        high: high + margin,
    }
}

fn finite_max(values: &[f64], fallback: f64) -> f64 {
    let high = finite(values).fold(f64::NEG_INFINITY, f64::max);
    if high.is_finite() { high } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnitude_snaps_to_ten_db_and_keeps_sixty_below_the_top() {
        let view = chart_view(ChartId::Magnitude);
        assert_eq!(
            (view.domain)(&[-3.0, -14.0]),
            Domain {
                low: -20.0,
                high: 0.0
            }
        );
        assert_eq!(
            (view.domain)(&[-2.0, -95.0, f64::NEG_INFINITY]),
            Domain {
                low: -60.0,
                high: 0.0
            }
        );
        assert_eq!(
            (view.domain)(&[]),
            Domain {
                low: -10.0,
                high: 10.0
            }
        );
    }

    #[test]
    fn vswr_starts_at_one_and_stops_between_two_and_twenty() {
        let view = chart_view(ChartId::Vswr);
        assert_eq!(
            (view.domain)(&[1.1, 1.4]),
            Domain {
                low: 1.0,
                high: 2.0
            }
        );
        assert_eq!(
            (view.domain)(&[1.1, 4.2]),
            Domain {
                low: 1.0,
                high: 5.0
            }
        );
        assert_eq!(
            (view.domain)(&[f64::INFINITY, 80.0]),
            Domain {
                low: 1.0,
                high: 20.0
            }
        );
        assert_eq!(
            (view.domain)(&[]),
            Domain {
                low: 1.0,
                high: 3.0
            }
        );
    }

    #[test]
    fn impedance_always_shows_zero() {
        let view = chart_view(ChartId::Impedance);
        assert_eq!(
            (view.domain)(&[40.0, 60.0]),
            Domain {
                low: 0.0,
                high: 75.0
            }
        );
    }

    #[test]
    fn delay_pads_its_range_by_a_tenth() {
        let view = chart_view(ChartId::Delay);
        let domain = (view.domain)(&[1e-9, 3e-9]);
        assert!((domain.low - 0.8e-9).abs() < 1e-18 && (domain.high - 3.2e-9).abs() < 1e-18);
        assert_eq!(
            (view.domain)(&[0.0]),
            Domain {
                low: -1.0,
                high: 1.0
            }
        );
        assert_eq!((view.format)(5e-9), "5.0 ns");
    }

    #[test]
    fn every_series_is_read_for_every_row() {
        assert_eq!(series_values(chart_view(ChartId::Phase), &[]).len(), 0);
        assert_eq!(chart_view(ChartId::Smith).label, "Smith");
    }
}
