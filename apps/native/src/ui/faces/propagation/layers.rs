use sdrmm_wire::propagation::IonosondeStation;

use super::model::{Cell, Path};
use crate::ui::map::{
    ACCENT, Geo,
    geo::{great_circle_line, unwrap_trail},
    heat::{Ramp, mix},
    overlay::{Dot, EDGE, Heat, Label, Line, Overlay, WHITE},
};

pub const MUF_MIN_MHZ: f64 = 4.0;
pub const MUF_MAX_MHZ: f64 = 40.0;

const MUF_STOPS: [(f64, u32); 5] = [
    (MUF_MIN_MHZ, 0x3b_4a_7a),
    (10.0, 0x2f_6f_8f),
    (18.0, 0x3f_ae_7a),
    (28.0, 0xe0_a4_58),
    (MUF_MAX_MHZ, 0xef_62_62),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Activity,
    Muf,
}

#[must_use]
pub fn muf_colour(mhz: f64) -> u32 {
    let (first, last) = (MUF_STOPS[0], MUF_STOPS[MUF_STOPS.len() - 1]);
    if mhz <= first.0 {
        return first.1;
    }
    for pair in MUF_STOPS.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        if mhz <= high.0 {
            return mix(low.1, high.1, (mhz - low.0) / (high.0 - low.0));
        }
    }
    last.1
}

#[must_use]
pub fn path_line(path: &Path) -> Vec<Geo> {
    unwrap_trail(&great_circle_line(path.from, path.to))
}

fn activity(cells: &[Cell]) -> Heat {
    Heat {
        points: cells
            .iter()
            .map(|cell| (cell.centre, (cell.weight / 12.0).clamp(0.05, 1.0)))
            .collect(),
        radius: vec![(0.0, 12.0), (8.0, 48.0)],
        intensity: vec![(0.0, 0.6), (8.0, 1.6)],
        opacity: vec![(0.0, 0.75)],
        ramp: Ramp {
            stops: vec![
                (0.0, 0x14_18_30, 0.0),
                (0.15, 0x1b_2a_5e, 1.0),
                (0.35, 0x2f_6f_8f, 1.0),
                (0.55, 0x3f_ae_7a, 1.0),
                (0.75, 0xe0_a4_58, 1.0),
                (1.0, 0xef_62_62, 1.0),
            ],
        },
    }
}

fn muf_cells(cells: &[Cell], overlay: &mut Overlay) {
    for cell in cells {
        let Some(muf) = cell.measured_muf3000_mhz else {
            continue;
        };
        overlay.dots.push(Dot {
            radius: vec![(0.0, 4.0), (6.0, 14.0)],
            alpha: 0.85,
            stroke: EDGE,
            ..Dot::plain(cell.centre, 4.0, muf_colour(muf))
        });
        overlay.labels.push(Label {
            key: format!("muf:{}", cell.key),
            at: cell.centre,
            text: format!("{muf:.0}"),
            colour: WHITE,
            below_px: -6.0,
            min_zoom: 3.0,
        });
    }
}

#[must_use]
pub fn overlay(
    cells: &[Cell],
    paths: &[Path],
    sondes: &[IonosondeStation],
    layer: Layer,
) -> Overlay {
    let mut out = Overlay::default();
    for path in paths {
        out.lines.push(Line {
            points: path_line(path),
            colour: ACCENT,
            alpha: (0.05 + 0.4 * path.weight.clamp(0.0, 1.0)) as f32,
            width: 0.8,
            dash: None,
        });
    }
    match layer {
        Layer::Activity => out.heat.push(activity(cells)),
        Layer::Muf => muf_cells(cells, &mut out),
    }
    for sonde in sondes {
        let at = Geo::new(sonde.latitude, sonde.longitude);
        out.dots.push(Dot {
            stroke: WHITE,
            stroke_width: 1.5,
            ..Dot::plain(at, 4.0, muf_colour(sonde.muf3000_mhz))
        });
        out.labels.push(Label {
            key: format!("sonde:{}", sonde.code),
            at,
            text: format!("{:.1}", sonde.muf3000_mhz),
            colour: WHITE,
            below_px: 6.0,
            min_zoom: 3.0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(muf: Option<f64>) -> Cell {
        Cell {
            key: "IO91".to_owned(),
            centre: Geo::new(51.5, -1.0),
            weight: 2.5,
            decodes: 4,
            callsigns: 3,
            best_freq_hz: 14_074_000.0,
            best_snr_db: -8.0,
            measured_muf3000_mhz: muf,
            median_distance_km: 3_000.0,
            last_seen: 0,
        }
    }

    fn path(from: Geo, to: Geo) -> Path {
        Path {
            key: "p".to_owned(),
            from,
            to,
            weight: 0.8,
            freq_hz: 14_074_000.0,
        }
    }

    #[test]
    fn a_path_is_a_curve_between_its_ends() {
        let line = path_line(&path(Geo::new(52.5, 13.0), Geo::new(42.5, -71.0)));
        assert!(line.len() > 2);
        assert!((line[0].lon - 13.0).abs() < 1e-6 && (line[0].lat - 52.5).abs() < 1e-6);
        assert!((line[line.len() - 1].lat - 42.5).abs() < 1e-6);
    }

    #[test]
    fn a_path_across_the_antimeridian_stays_continuous() {
        let line = path_line(&path(Geo::new(0.0, 170.0), Geo::new(0.0, -170.0)));
        assert!(
            line.windows(2)
                .all(|pair| (pair[1].lon - pair[0].lon).abs() <= 180.0)
        );
    }

    #[test]
    fn the_muf_layer_skips_unmeasured_cells() {
        let drawn = overlay(&[cell(Some(18.0)), cell(None)], &[], &[], Layer::Muf);
        assert_eq!(drawn.dots.len(), 1);
        assert_eq!(drawn.labels[0].text, "18");
        assert!(drawn.heat.is_empty());
        let heat = overlay(&[cell(Some(18.0)), cell(None)], &[], &[], Layer::Activity);
        assert_eq!(heat.heat[0].points.len(), 2);
    }

    #[test]
    fn an_ionosonde_is_labelled_with_its_muf() {
        let sonde = IonosondeStation {
            code: "AU930".to_owned(),
            name: "Austin".to_owned(),
            latitude: 30.4,
            longitude: -97.7,
            muf3000_mhz: 28.8,
            fof2_mhz: None,
            m3000: None,
            confidence: None,
            measured_at: "2026-08-16T18:10:05Z".to_owned(),
        };
        let drawn = overlay(&[], &[], &[sonde], Layer::Activity);
        assert_eq!(drawn.dots[0].at, Geo::new(30.4, -97.7));
        assert_eq!(drawn.labels[0].text, "28.8");
    }

    #[test]
    fn the_muf_ramp_pins_its_ends_and_moves_through_the_middle() {
        assert_eq!(muf_colour(MUF_MIN_MHZ), 0x3b_4a_7a);
        assert_eq!(muf_colour(MUF_MIN_MHZ - 10.0), 0x3b_4a_7a);
        assert_eq!(muf_colour(MUF_MAX_MHZ), 0xef_62_62);
        assert_eq!(muf_colour(MUF_MAX_MHZ + 10.0), 0xef_62_62);
        assert_eq!(muf_colour(18.0), 0x3f_ae_7a);
        assert_ne!(muf_colour(14.0), muf_colour(18.0));
    }
}
