use sdrmm_wire::AvhrrChannel;

use super::geometry::{
    CHANNEL_WEDGE, FRAME_LINES, RAMP_WEDGES, WEDGE_LINES, ZERO_WEDGE, wedge_level,
};

const MIN_RAMP_CORRELATION: f64 = 0.98;
const ID_WEDGES: usize = 6;
const RAMP_SPAN: usize = ZERO_WEDGE * WEDGE_LINES;
const FULL_SCALE: f32 = 255.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Calibration {
    pub(crate) black: f32,
    pub(crate) white: f32,
}

impl Calibration {
    pub(crate) fn level(self, raw: u16) -> u8 {
        let span = (self.white - self.black).max(1.0);
        ((f32::from(raw) - self.black) / span * FULL_SCALE)
            .round()
            .clamp(0.0, FULL_SCALE) as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    pub(crate) calibration: Calibration,
    pub(crate) channel_a: Option<AvhrrChannel>,
    pub(crate) channel_b: Option<AvhrrChannel>,
}

struct Fit {
    offset: f64,
    slope: f64,
    correlation: f64,
}

fn wedge_mean(column: &[f32], first_line: usize) -> f64 {
    let lines = &column[first_line + 1..first_line + WEDGE_LINES - 1];
    lines.iter().map(|&v| f64::from(v)).sum::<f64>() / lines.len() as f64
}

fn ramp(column: &[f32], start: usize) -> [f64; ZERO_WEDGE] {
    std::array::from_fn(|k| wedge_mean(column, start + k * WEDGE_LINES))
}

fn fit(wedges: &[f64; ZERO_WEDGE]) -> Fit {
    let targets: [f64; ZERO_WEDGE] = std::array::from_fn(|k| f64::from(wedge_level(k + 1)));
    let n = ZERO_WEDGE as f64;
    let mean_t = targets.iter().sum::<f64>() / n;
    let mean_w = wedges.iter().sum::<f64>() / n;
    let mut cross = 0.0;
    let mut var_t = 0.0;
    let mut var_w = 0.0;
    for (t, w) in targets.iter().zip(wedges) {
        cross += (t - mean_t) * (w - mean_w);
        var_t += (t - mean_t).powi(2);
        var_w += (w - mean_w).powi(2);
    }
    let slope = cross / var_t;
    let correlation = if var_w > 0.0 {
        cross / (var_t * var_w).sqrt()
    } else {
        0.0
    };
    Fit {
        offset: mean_w - slope * mean_t,
        slope,
        correlation,
    }
}

fn channel(column: &[f32], start: usize, wedges: &[f64; ZERO_WEDGE]) -> Option<AvhrrChannel> {
    let id_line = (start + (CHANNEL_WEDGE - 1) * WEDGE_LINES).checked_sub(FRAME_LINES)?;
    let id = wedge_mean(column, id_line);
    let step = (wedges[RAMP_WEDGES - 1] - wedges[0]) / (RAMP_WEDGES - 1) as f64;
    let (index, distance) = wedges[..ID_WEDGES]
        .iter()
        .map(|w| (w - id).abs())
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    if distance > step / 2.0 {
        return None;
    }
    AvhrrChannel::from_wedge((index + 1) as u8)
}

pub(crate) fn read(column_a: &[f32], column_b: &[f32]) -> Option<Reading> {
    let lines = column_a.len().min(column_b.len());
    let latest = lines.checked_sub(RAMP_SPAN)?;
    let earliest = latest.saturating_sub(FRAME_LINES - 1);
    let (start, fit_a, fit_b) = (earliest..=latest)
        .map(|start| {
            (
                start,
                fit(&ramp(column_a, start)),
                fit(&ramp(column_b, start)),
            )
        })
        .max_by(|a, b| {
            (a.1.correlation + a.2.correlation).total_cmp(&(b.1.correlation + b.2.correlation))
        })?;
    if fit_a.correlation.min(fit_b.correlation) < MIN_RAMP_CORRELATION {
        return None;
    }
    let offset = (fit_a.offset + fit_b.offset) / 2.0;
    let slope = (fit_a.slope + fit_b.slope) / 2.0;
    Some(Reading {
        calibration: Calibration {
            black: offset as f32,
            white: (offset + slope * f64::from(FULL_SCALE)) as f32,
        },
        channel_a: channel(column_a, start, &ramp(column_a, start)),
        channel_b: channel(column_b, start, &ramp(column_b, start)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(phase: usize, lines: usize, id: usize) -> Vec<f32> {
        (0..lines)
            .map(|line| {
                let wedge = (line + phase) % FRAME_LINES / WEDGE_LINES + 1;
                let level = match wedge {
                    CHANNEL_WEDGE => wedge_level(id),
                    10..=15 => 90,
                    _ => wedge_level(wedge),
                };
                1_000.0 + 40.0 * f32::from(level)
            })
            .collect()
    }

    #[test]
    fn the_ramp_calibrates_and_wedge_16_names_the_channel() {
        let a = column(64, 160, 2);
        let b = column(64, 160, 4);
        let reading = read(&a, &b).expect("reads a frame");
        assert!((reading.calibration.black - 1_000.0).abs() < 1.0);
        assert!((reading.calibration.white - 11_200.0).abs() < 1.0);
        assert_eq!(reading.channel_a, Some(AvhrrChannel::Ch2));
        assert_eq!(reading.channel_b, Some(AvhrrChannel::Ch4));
    }

    #[test]
    fn a_flat_column_reads_nothing() {
        let flat = vec![500.0; 200];
        assert!(read(&flat, &flat).is_none());
    }

    #[test]
    fn too_few_lines_read_nothing() {
        let a = column(64, 60, 2);
        assert!(read(&a, &a).is_none());
    }
}
