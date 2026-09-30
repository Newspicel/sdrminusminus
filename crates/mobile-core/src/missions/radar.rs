use sdrmm_wire::{
    frame::{FrameError, RangeDopplerFrame},
    radar::{RadarUpdate, TrackState},
};

use super::views::{RadarTrack, RadarView, RgbaImage};
use crate::colormap::{self, LUT_SIZE};

pub(crate) const MAX_RADAR_WIDTH: usize = 1024;
pub(crate) const MAX_RADAR_HEIGHT: usize = 512;

pub(crate) struct RadarPainter {
    lut: [[u8; 4]; LUT_SIZE],
}

impl RadarPainter {
    pub(crate) fn new() -> Self {
        Self {
            lut: colormap::lut(),
        }
    }

    pub(crate) fn paint(&self, bytes: &[u8]) -> Result<RgbaImage, FrameError> {
        let frame = RangeDopplerFrame::decode(bytes)?;
        let (ranges, dopplers) = (usize::from(frame.ranges), usize::from(frame.dopplers));
        if ranges == 0 || dopplers == 0 {
            return Err(FrameError::Shape);
        }
        let (fx, fy) = (
            ranges.div_ceil(MAX_RADAR_WIDTH),
            dopplers.div_ceil(MAX_RADAR_HEIGHT),
        );
        let (width, height) = (ranges.div_ceil(fx), dopplers.div_ceil(fy));
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let block = height - 1 - y;
            let rows = block * fy..((block + 1) * fy).min(dopplers);
            for x in 0..width {
                let cols = x * fx..((x + 1) * fx).min(ranges);
                let peak = rows
                    .clone()
                    .flat_map(|row| {
                        frame.cells[row * ranges + cols.start..row * ranges + cols.end].iter()
                    })
                    .copied()
                    .max()
                    .unwrap_or(0);
                rgba.extend_from_slice(&self.lut[usize::from(peak)]);
            }
        }
        let range_max_m = f32::from(frame.ranges).mul_add(frame.range_step_m, frame.range_first_m);
        Ok(RgbaImage {
            width: u32::try_from(width).unwrap_or(u32::MAX),
            height: u32::try_from(height).unwrap_or(u32::MAX),
            rgba,
            range_max_km: range_max_m / 1_000.0,
            doppler_span_hz: f32::from(frame.dopplers) * frame.doppler_step_hz,
        })
    }
}

pub(crate) fn view(mission: &str, update: &RadarUpdate, stale: bool) -> RadarView {
    let mut tracks: Vec<RadarTrack> = update
        .tracks
        .iter()
        .map(|track| RadarTrack {
            id: track.id,
            range_km: track.range_km,
            doppler_hz: track.doppler_hz,
            speed_mps: track.range_rate_mps,
            snr_db: track.snr_db,
            closing: track.doppler_hz > 0.0,
            coasting: track.state == TrackState::Coasting,
            bearing_deg: track.aoa.and_then(|aoa| aoa.bearing_deg),
        })
        .collect();
    tracks.sort_by(|a, b| b.snr_db.total_cmp(&a.snr_db));
    RadarView {
        mission: mission.to_owned(),
        echoes: u32::try_from(update.detections.len()).unwrap_or(u32::MAX),
        tracks,
        stale,
        problems: update
            .problems
            .iter()
            .map(|problem| problem.label().to_owned())
            .collect(),
    }
}

pub(crate) fn empty(mission: &str) -> RadarView {
    RadarView {
        mission: mission.to_owned(),
        echoes: 0,
        tracks: Vec::new(),
        stale: true,
        problems: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use sdrmm_wire::{
        frame::RangeDopplerOwned,
        radar::{RadarAoa, RadarDetection, RadarProblem, RadarTrack as WireTrack},
    };

    use super::*;

    pub(crate) fn frame(ranges: u16, dopplers: u16, hot: &[(usize, usize, u8)]) -> Vec<u8> {
        let mut cells = vec![0u8; usize::from(ranges) * usize::from(dopplers)];
        for (row, col, value) in hot {
            cells[row * usize::from(ranges) + col] = *value;
        }
        RangeDopplerOwned {
            stream_id: 4,
            seq: 1,
            timestamp: 0,
            ranges,
            dopplers,
            range_first_m: 0.0,
            range_step_m: 300.0,
            doppler_first_hz: -(f32::from(dopplers) / 2.0) * 2.0,
            doppler_step_hz: 2.0,
            carrier_hz: 100e6,
            db_min: 0.0,
            db_max: 30.0,
            cells,
        }
        .frame()
        .encode()
    }

    fn pixel(image: &RgbaImage, x: usize, y: usize) -> [u8; 4] {
        let at = (y * image.width as usize + x) * 4;
        [
            image.rgba[at],
            image.rgba[at + 1],
            image.rgba[at + 2],
            image.rgba[at + 3],
        ]
    }

    #[test]
    fn radar_image_rows_run_top_down_positive_doppler() {
        let painter = RadarPainter::new();
        let bytes = frame(8, 4, &[(3, 2, 255), (0, 5, 128)]);
        let image = painter.paint(&bytes).expect("image");
        assert_eq!((image.width, image.height), (8, 4));
        assert_eq!(image.rgba.len(), 8 * 4 * 4);
        let lut = colormap::lut();
        assert_eq!(pixel(&image, 2, 0), lut[255]);
        assert_eq!(pixel(&image, 5, 3), lut[128]);
        assert_eq!(pixel(&image, 0, 0), lut[0]);
        assert!(image.rgba.chunks(4).all(|px| px[3] == 255));
    }

    #[test]
    fn axes_follow_the_steps() {
        let painter = RadarPainter::new();
        let image = painter.paint(&frame(256, 101, &[])).expect("image");
        assert!((image.range_max_km - 76.8).abs() < 1e-3);
        assert!((image.doppler_span_hz - 202.0).abs() < 1e-3);
    }

    #[test]
    fn large_surfaces_are_max_pooled() {
        let painter = RadarPainter::new();
        let image = painter
            .paint(&frame(2048, 1024, &[(1023, 2047, 200)]))
            .expect("image");
        assert_eq!((image.width, image.height), (1024, 512));
        let lut = colormap::lut();
        assert_eq!(pixel(&image, 1023, 0), lut[200]);
        let hot = image.rgba.chunks(4).filter(|px| *px == lut[200]).count();
        assert_eq!(hot, 1);
    }

    #[test]
    fn a_bad_frame_is_refused() {
        let painter = RadarPainter::new();
        let bytes = frame(8, 4, &[]);
        assert!(painter.paint(&bytes[..bytes.len() - 1]).is_err());
        assert!(painter.paint(&[1, 6, 0]).is_err());
    }

    fn track(id: u32, snr_db: f32, doppler_hz: f32) -> WireTrack {
        WireTrack {
            id,
            state: if id == 2 {
                TrackState::Coasting
            } else {
                TrackState::Confirmed
            },
            range_km: 20.0,
            range_rate_mps: -doppler_hz * 3.0,
            doppler_hz,
            snr_db,
            aoa: (id == 1).then_some(RadarAoa {
                azimuth_deg: 10.0,
                bearing_deg: Some(55.0),
                sigma_deg: 3.0,
                quality: 0.9,
                mirror_deg: None,
            }),
            ..WireTrack::default()
        }
    }

    #[test]
    fn tracks_keep_the_server_range_rate_and_sort_by_snr() {
        let update = RadarUpdate {
            tracks: vec![track(1, 12.0, 10.0), track(2, 20.0, -5.0)],
            detections: vec![RadarDetection::default(); 3],
            problems: vec![
                RadarProblem::NoHeading,
                RadarProblem::Refused("Tuned by arr".to_owned()),
            ],
            ..RadarUpdate::default()
        };
        let view = view("pr1", &update, false);
        assert_eq!(view.echoes, 3);
        assert_eq!(
            view.tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            [2, 1]
        );
        assert!(view.tracks[0].coasting && !view.tracks[0].closing);
        assert_eq!(view.tracks[0].speed_mps, 15.0);
        assert!(view.tracks[1].closing);
        assert_eq!(view.tracks[1].bearing_deg, Some(55.0));
        assert_eq!(view.problems, ["No heading", "Tuned by arr"]);
        assert!(empty("pr1").stale);
    }

    #[test]
    fn a_carrier_outside_the_array_table_is_told() {
        let update = RadarUpdate {
            problems: vec![RadarProblem::TableOutOfRange],
            ..RadarUpdate::default()
        };
        let view = view("pr1", &update, false);
        assert_eq!(view.problems, [RadarProblem::TableOutOfRange.label()]);
        assert_eq!(view.problems, ["Outside cal table"]);
    }
}
