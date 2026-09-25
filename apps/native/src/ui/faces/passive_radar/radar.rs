use sdrmm_wire::coherent::{Illuminator, PassiveRadarParams, RadarDetection};

use crate::ui::{
    kit_raster::{Pen, Raster, Scene, WHITE},
    plot::Palette,
};

const LIGHT_SPEED_KM_S: f64 = 299_792.458;
const MARK_RADIUS_PX: f32 = 5.0;
const PALETTE_STEPS: usize = 256;

pub const DEFAULT_ILLUMINATOR: Illuminator = Illuminator {
    lat: 0.0,
    lon: 0.0,
    freq_hz: 100e6,
};

#[must_use]
pub fn range_axis_km(settings: &PassiveRadarParams, range_step_us: f64) -> f64 {
    f64::from(settings.max_range_bins) * range_step_us * LIGHT_SPEED_KM_S / 1e6
}

#[must_use]
pub fn doppler_axis_hz(settings: &PassiveRadarParams) -> f64 {
    settings.doppler_span_hz / 2.0
}

#[must_use]
pub fn doppler_row(doppler_hz: f32, dopplers: u16, step_hz: f32) -> i64 {
    if step_hz == 0.0 {
        return 0;
    }
    ((f32::from(dopplers) - 1.0) / 2.0 + doppler_hz / step_hz).round() as i64
}

#[must_use]
pub fn detection_label(hit: &RadarDetection) -> (String, String) {
    let name = hit.track_id.map_or_else(
        || format!("Bin {}", hit.range_bin),
        |id| format!("Target {id}"),
    );
    let sign = if hit.doppler_hz >= 0.0 { "+" } else { "" };
    (
        name,
        format!(
            "{:.2} km \u{b7} {sign}{:.1} Hz \u{b7} {:.1} dB",
            hit.range_km, hit.doppler_hz, hit.snr_db
        ),
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surface {
    pub ranges: u16,
    pub dopplers: u16,
    pub doppler_step_hz: f32,
    pub cells: Vec<u8>,
}

#[derive(Debug)]
pub struct RadarScene {
    pub surface: Option<Surface>,
    pub detections: Vec<RadarDetection>,
    stamp: u64,
    table: Vec<[u8; 4]>,
}

impl RadarScene {
    #[must_use]
    pub fn new(palette: Palette) -> Self {
        let table = (0..PALETTE_STEPS)
            .map(|step| {
                let [r, g, b] = palette.rgb(step as f32 / (PALETTE_STEPS - 1) as f32);
                [
                    (r * 255.0).round() as u8,
                    (g * 255.0).round() as u8,
                    (b * 255.0).round() as u8,
                    255,
                ]
            })
            .collect();
        Self {
            surface: None,
            detections: Vec::new(),
            stamp: 0,
            table,
        }
    }

    pub fn show(&mut self, surface: Surface) {
        self.surface = Some(surface);
        self.stamp += 1;
    }

    pub fn mark(&mut self, detections: Vec<RadarDetection>) {
        self.detections = detections;
        self.stamp += 1;
    }
}

impl Scene for RadarScene {
    fn stamp(&self) -> u64 {
        self.stamp
    }

    fn paint(&mut self, raster: &mut Raster) {
        raster.fill([0, 0, 0, 255]);
        let Some(surface) = &self.surface else {
            return;
        };
        let (ranges, dopplers) = (usize::from(surface.ranges), usize::from(surface.dopplers));
        if ranges == 0 || dopplers == 0 || surface.cells.len() < ranges * dopplers {
            return;
        }
        let (width, height) = (raster.width as usize, raster.height as usize);
        for y in 0..height {
            let row = dopplers - 1 - (y * dopplers / height).min(dopplers - 1);
            for x in 0..width {
                let column = (x * ranges / width).min(ranges - 1);
                let level = surface.cells[row * ranges + column];
                raster.put(x as i64, y as i64, self.table[usize::from(level)]);
            }
        }
        let pen = Pen::new(WHITE, 1.0, raster.scale.max(1.0));
        for hit in &self.detections {
            let doppler = doppler_row(hit.doppler_hz, surface.dopplers, surface.doppler_step_hz);
            let x = (hit.range_bin as f32 + 0.5) / f32::from(surface.ranges) * width as f32;
            let y = (f32::from(surface.dopplers) - 0.5 - doppler as f32)
                / f32::from(surface.dopplers)
                * height as f32;
            raster.circle((x, y), MARK_RADIUS_PX * raster.scale, pen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(range_bin: u32, doppler_hz: f32) -> RadarDetection {
        RadarDetection {
            range_bin,
            range_km: 18.0,
            doppler_hz,
            snr_db: 19.0,
            track_id: None,
        }
    }

    #[test]
    fn the_middle_row_is_zero_doppler() {
        assert_eq!(doppler_row(0.0, 5, 2.0), 2);
        assert_eq!(doppler_row(4.0, 5, 2.0), 4);
        assert_eq!(doppler_row(-4.0, 5, 2.0), 0);
        assert_eq!(doppler_row(10.0, 5, 0.0), 0);
    }

    #[test]
    fn the_range_axis_is_bins_times_the_light_path_of_a_sample() {
        let settings = PassiveRadarParams::default();
        let km = range_axis_km(&settings, 1.0);
        assert!((km - 256.0 * 0.299_792_458).abs() < 1e-9);
        assert!((doppler_axis_hz(&settings) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_detection_is_named_by_its_track_or_its_bin() {
        let (name, value) = detection_label(&hit(60, 120.0));
        assert_eq!(name, "Bin 60");
        assert_eq!(value, "18.00 km \u{b7} +120.0 Hz \u{b7} 19.0 dB");
        let tracked = RadarDetection {
            track_id: Some(3),
            ..hit(60, -1.5)
        };
        let (name, value) = detection_label(&tracked);
        assert_eq!(name, "Target 3");
        assert!(value.contains("-1.5 Hz"));
    }

    #[test]
    fn the_most_negative_doppler_is_painted_at_the_bottom() {
        let mut scene = RadarScene::new(Palette::Classic);
        scene.show(Surface {
            ranges: 1,
            dopplers: 2,
            doppler_step_hz: 1.0,
            cells: vec![0, 255],
        });
        let mut raster = Raster::sized(1, 2);
        scene.paint(&mut raster);
        assert_eq!(raster.at(0, 0), Some(scene.table[255]));
        assert_eq!(raster.at(0, 1), Some(scene.table[0]));
    }

    #[test]
    fn a_short_surface_paints_nothing_but_black() {
        let mut scene = RadarScene::new(Palette::Classic);
        scene.show(Surface {
            ranges: 4,
            dopplers: 4,
            doppler_step_hz: 1.0,
            cells: vec![255; 3],
        });
        let mut raster = Raster::sized(2, 2);
        scene.paint(&mut raster);
        assert!(
            raster
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0, 0, 0, 255])
        );
    }

    #[test]
    fn a_new_surface_or_new_marks_ask_for_a_repaint() {
        let mut scene = RadarScene::new(Palette::Classic);
        let before = scene.stamp();
        scene.mark(vec![hit(1, 0.0)]);
        assert!(scene.stamp() > before);
    }
}
