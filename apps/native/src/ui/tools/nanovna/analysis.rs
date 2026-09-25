use sdrmm_wire::tools::{NanoVnaComplex, NanoVnaPoint};

use super::rf::{
    Component, admittance, equivalent_component, gain_db, group_delays, impedance, magnitude,
    mismatch_loss_db, phase_deg, q_factor, return_loss_db, vswr,
};

#[derive(Clone, Debug, PartialEq)]
pub struct PointReadout {
    pub index: usize,
    pub frequency_hz: f64,
    pub s11: NanoVnaComplex,
    pub s21: NanoVnaComplex,
    pub s11_db: f64,
    pub s11_linear: f64,
    pub return_loss_db: f64,
    pub s11_phase_deg: f64,
    pub vswr: f64,
    pub mismatch_loss_db: f64,
    pub impedance: Option<NanoVnaComplex>,
    pub impedance_magnitude: f64,
    pub q: f64,
    pub component: Option<Component>,
    pub admittance: Option<NanoVnaComplex>,
    pub s21_db: f64,
    pub s21_linear: f64,
    pub s21_phase_deg: f64,
    pub insertion_loss_db: f64,
    pub group_delay_s: f64,
}

#[must_use]
pub fn readouts(points: &[NanoVnaPoint]) -> Vec<PointReadout> {
    let delays = group_delays(points);
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let z = impedance(point.s11);
            let frequency_hz = point.frequency_hz as f64;
            PointReadout {
                index,
                frequency_hz,
                s11: point.s11,
                s21: point.s21,
                s11_db: gain_db(point.s11),
                s11_linear: magnitude(point.s11),
                return_loss_db: return_loss_db(point.s11),
                s11_phase_deg: phase_deg(point.s11),
                vswr: vswr(point.s11),
                mismatch_loss_db: mismatch_loss_db(point.s11),
                impedance: z,
                impedance_magnitude: z.map_or(f64::NAN, |z| z.re.hypot(z.im)),
                q: q_factor(z),
                component: z.and_then(|z| equivalent_component(z.im, frequency_hz)),
                admittance: admittance(point.s11),
                s21_db: gain_db(point.s21),
                s21_linear: magnitude(point.s21),
                s21_phase_deg: phase_deg(point.s21),
                insertion_loss_db: -gain_db(point.s21),
                group_delay_s: delays.get(index).copied().unwrap_or(f64::NAN),
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub span_hz: f64,
    pub center_hz: f64,
    pub q: f64,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SweepAnalysis {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub span_hz: f64,
    pub count: usize,
    pub resonance: Option<PointReadout>,
    pub vswr_bands: Vec<(f64, Option<Band>)>,
    pub peak: Option<PointReadout>,
    pub transmission_band: Option<Band>,
    pub transmitting: bool,
}

const HALF_POWER_DB: f64 = 3.0;
const S21_NOISE_FLOOR_DB: f64 = -70.0;
const VSWR_LIMITS: [f64; 3] = [1.5, 2.0, 3.0];

#[must_use]
pub fn analyse(points: &[NanoVnaPoint]) -> SweepAnalysis {
    let rows = readouts(points);
    let frequencies: Vec<f64> = rows.iter().map(|row| row.frequency_hz).collect();
    let start_hz = frequencies.first().copied().unwrap_or(0.0);
    let stop_hz = frequencies.last().copied().unwrap_or(0.0);
    let resonance = best_by(&rows, |row| row.vswr);
    let peak = best_by(&rows, |row| -row.s21_db);
    let transmitting = peak
        .as_ref()
        .is_some_and(|peak| peak.s21_db > S21_NOISE_FLOOR_DB);
    let vswrs: Vec<f64> = rows.iter().map(|row| row.vswr).collect();
    let vswr_bands = VSWR_LIMITS
        .iter()
        .map(|limit| {
            let band = resonance.as_ref().and_then(|resonance| {
                band_span(&frequencies, &vswrs, resonance.index, *limit, true)
            });
            (*limit, band)
        })
        .collect();
    let transmission_band = peak.as_ref().filter(|_| transmitting).and_then(|peak| {
        let gains: Vec<f64> = rows.iter().map(|row| row.s21_db).collect();
        band_span(
            &frequencies,
            &gains,
            peak.index,
            peak.s21_db - HALF_POWER_DB,
            false,
        )
    });
    SweepAnalysis {
        start_hz,
        stop_hz,
        span_hz: stop_hz - start_hz,
        count: rows.len(),
        resonance,
        vswr_bands,
        peak: if transmitting { peak } else { None },
        transmission_band,
        transmitting,
    }
}

fn best_by(rows: &[PointReadout], score: impl Fn(&PointReadout) -> f64) -> Option<PointReadout> {
    let mut best: Option<&PointReadout> = None;
    let mut best_score = f64::INFINITY;
    for row in rows {
        let candidate = score(row);
        if candidate.is_finite() && candidate < best_score {
            best = Some(row);
            best_score = candidate;
        }
    }
    best.cloned()
}

#[must_use]
pub fn band_span(
    frequencies: &[f64],
    values: &[f64],
    center: usize,
    threshold: f64,
    below: bool,
) -> Option<Band> {
    let inside = |index: usize| {
        values.get(index).is_some_and(|value| {
            value.is_finite()
                && if below {
                    *value <= threshold
                } else {
                    *value >= threshold
                }
        })
    };
    let last = frequencies.len().checked_sub(1)?;
    if last < 1 || !inside(center) {
        return None;
    }
    let mut low = center;
    while low > 0 && inside(low - 1) {
        low -= 1;
    }
    let mut high = center;
    while high < last && inside(high + 1) {
        high += 1;
    }
    let start_hz = if low == 0 {
        frequencies[0]
    } else {
        crossing(frequencies, values, low - 1, low, threshold)?
    };
    let stop_hz = if high == last {
        frequencies[last]
    } else {
        crossing(frequencies, values, high, high + 1, threshold)?
    };
    let span_hz = stop_hz - start_hz;
    let center_hz = f64::midpoint(start_hz, stop_hz);
    Some(Band {
        start_hz,
        stop_hz,
        span_hz,
        center_hz,
        q: if span_hz > 0.0 {
            center_hz / span_hz
        } else {
            f64::INFINITY
        },
        truncated: low == 0 || high == last,
    })
}

fn crossing(
    frequencies: &[f64],
    values: &[f64],
    low: usize,
    high: usize,
    threshold: f64,
) -> Option<f64> {
    let (low_hz, high_hz) = (*frequencies.get(low)?, *frequencies.get(high)?);
    let (low_value, high_value) = (*values.get(low)?, *values.get(high)?);
    if high_value == low_value {
        return Some(high_hz);
    }
    Some(low_hz + (threshold - low_value) / (high_value - low_value) * (high_hz - low_hz))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tools::nanovna::{
        rf::{ComponentKind, complex},
        testdata::{point, resonant_sweep, sweep_of},
    };

    fn close(left: f64, right: f64, digits: i32) -> bool {
        (left - right).abs() < 10f64.powi(-digits) / 2.0
    }

    #[test]
    fn one_frequency_carries_every_derived_quantity() {
        let rows = readouts(&[point(1_000_000, complex(0.0, 0.5), complex(0.5, 0.0))]);
        let row = &rows[0];
        assert!(close(row.vswr, 3.0, 12));
        let z = row.impedance.unwrap();
        assert!(close(z.re, 30.0, 9) && close(z.im, 40.0, 9));
        assert!(close(row.impedance_magnitude, 50.0, 9));
        assert!(close(row.s21_db, -6.0206, 4));
        assert!(close(row.insertion_loss_db, 6.0206, 4));
        assert_eq!(
            row.component.map(|component| component.kind),
            Some(ComponentKind::Inductance)
        );
    }

    #[test]
    fn the_resonance_the_load_was_built_around_is_found() {
        let analysis = analyse(&resonant_sweep(201).points);
        let resonance = analysis.resonance.unwrap();
        assert!(resonance.frequency_hz > 14_080_000.0 && resonance.frequency_hz < 14_120_000.0);
        assert!(resonance.vswr < 1.01);
    }

    #[test]
    fn the_vswr_bands_nest_and_carry_a_loaded_q() {
        let analysis = analyse(&resonant_sweep(201).points);
        let spans: Vec<f64> = analysis
            .vswr_bands
            .iter()
            .map(|(_, band)| band.map_or(0.0, |band| band.span_hz))
            .collect();
        assert!(spans[0] > 0.0 && spans[0] < spans[1] && spans[1] < spans[2]);
        let two = analysis
            .vswr_bands
            .iter()
            .find(|(limit, _)| *limit == 2.0)
            .and_then(|(_, band)| *band)
            .unwrap();
        assert!(!two.truncated);
        assert!(two.q > 1.0);
    }

    #[test]
    fn the_noise_floor_is_not_a_passband() {
        let points: Vec<_> = (0..21)
            .map(|index| {
                point(
                    1_000_000 + index * 100_000,
                    complex(0.99, 0.0),
                    complex(1e-5, 1e-5),
                )
            })
            .collect();
        let quiet = analyse(&points);
        assert!(!quiet.transmitting);
        assert!(quiet.peak.is_none());
        assert!(quiet.transmission_band.is_none());
    }

    #[test]
    fn an_empty_sweep_invents_nothing() {
        let empty = analyse(&[]);
        assert_eq!(empty.count, 0);
        assert!(empty.resonance.is_none());
        assert_eq!(empty.span_hz, 0.0);
    }

    const FREQUENCIES: [f64; 5] = [1.0, 2.0, 3.0, 4.0, 5.0];

    #[test]
    fn band_edges_interpolate_between_samples() {
        let band = band_span(&FREQUENCIES, &[4.0, 2.0, 0.0, 2.0, 4.0], 2, 1.0, true).unwrap();
        assert!(close(band.start_hz, 2.5, 9) && close(band.stop_hz, 3.5, 9));
        assert!(close(band.span_hz, 1.0, 9));
        assert!(!band.truncated);
    }

    #[test]
    fn a_band_reaching_the_edge_is_marked() {
        let band = band_span(&FREQUENCIES, &[0.0; 5], 2, 1.0, true).unwrap();
        assert!(band.truncated);
        assert_eq!((band.start_hz, band.stop_hz), (1.0, 5.0));
    }

    #[test]
    fn a_centre_outside_the_limit_has_no_band() {
        assert!(band_span(&FREQUENCIES, &[4.0; 5], 2, 1.0, true).is_none());
    }

    #[test]
    fn a_trace_can_be_required_to_stay_above_a_limit() {
        let band = band_span(&FREQUENCIES, &[0.0, 0.0, 10.0, 0.0, 0.0], 2, 5.0, false).unwrap();
        assert!(close(band.start_hz, 2.5, 9) && close(band.stop_hz, 3.5, 9));
    }

    #[test]
    fn group_delay_is_defined_at_both_ends() {
        let sweep = sweep_of(vec![
            point(1, complex(0.0, 0.0), complex(0.0, 0.0)),
            point(2, complex(0.0, 0.0), complex(0.0, 0.0)),
        ]);
        assert!(
            readouts(&sweep.points)
                .iter()
                .all(|row| row.group_delay_s.is_finite())
        );
    }
}
