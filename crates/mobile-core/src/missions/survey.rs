use sdrmm_wire::survey::{SurveyCell, SurveyGrid, SurveyUpdate};

use super::views::{SurveyPoint, SurveyView};
use crate::records::LatLon;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Survey {
    freq_hz: f64,
    level_db: Option<f32>,
    min_db: Option<f32>,
    max_db: Option<f32>,
    total: u64,
    recording: bool,
}

impl Survey {
    pub(crate) fn new(freq_hz: f64, recording: bool, total: u64) -> Self {
        Self {
            freq_hz,
            recording,
            total,
            ..Self::default()
        }
    }

    pub(crate) fn seed(&mut self, grid: &SurveyGrid) -> Vec<SurveyPoint> {
        if let Some(freq_hz) = grid.frequency_hz {
            self.freq_hz = freq_hz;
        }
        self.recording = grid.recording;
        self.total = grid.cells.len() as u64;
        self.min_db = None;
        self.max_db = None;
        grid.cells.iter().map(|cell| self.cell(cell)).collect()
    }

    pub(crate) fn update(&mut self, update: &SurveyUpdate) -> Option<SurveyPoint> {
        if let Some(target_hz) = update.target_hz {
            self.freq_hz = target_hz;
        }
        self.level_db = update.level_dbfs;
        self.recording = update.recording && update.stopped.is_none();
        self.total = u64::from(update.cells);
        if update.cells == 0 {
            self.min_db = None;
            self.max_db = None;
        }
        update.cell.as_ref().map(|cell| self.cell(cell))
    }

    fn cell(&mut self, cell: &SurveyCell) -> SurveyPoint {
        self.min_db = Some(
            self.min_db
                .map_or(cell.level_dbfs, |min| min.min(cell.level_dbfs)),
        );
        self.max_db = Some(
            self.max_db
                .map_or(cell.level_dbfs, |max| max.max(cell.level_dbfs)),
        );
        SurveyPoint {
            at: LatLon {
                lat: cell.latitude,
                lon: cell.longitude,
            },
            level_db: cell.level_dbfs,
        }
    }

    pub(crate) fn view(&self, mission: &str) -> SurveyView {
        SurveyView {
            mission: mission.to_owned(),
            freq_hz: self.freq_hz,
            level_db: self.level_db,
            min_db: self.min_db.unwrap_or_default(),
            max_db: self.max_db.unwrap_or_default(),
            total: self.total,
            recording: self.recording,
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::survey::SurveyStop;

    use super::*;

    fn cell(lon: f64, level: f32) -> SurveyCell {
        SurveyCell {
            latitude: 52.5,
            longitude: lon,
            frequency_hz: 145.5e6,
            level_dbfs: level,
            measured_at: "2026-09-28T12:00:00Z".to_owned(),
            observations: 1,
            accuracy_m: None,
        }
    }

    #[test]
    fn a_seed_lists_every_cell_and_the_range() {
        let mut survey = Survey::new(0.0, false, 0);
        let grid = SurveyGrid {
            node: "map1".to_owned(),
            frequency_hz: Some(145.5e6),
            offset_hz: 0,
            bandwidth_hz: 12_500,
            recording: true,
            cells: vec![cell(13.0, -60.0), cell(13.1, -40.0)],
            dropped: 0,
        };
        let points = survey.seed(&grid);
        assert_eq!(points.len(), 2);
        let view = survey.view("map1");
        assert_eq!((view.min_db, view.max_db, view.total), (-60.0, -40.0, 2));
        assert!(view.recording);
        assert_eq!(view.freq_hz, 145.5e6);
    }

    #[test]
    fn updates_add_points_and_a_stop_ends_recording() {
        let mut survey = Survey::new(145.5e6, true, 0);
        let update = SurveyUpdate {
            level_dbfs: Some(-51.0),
            target_hz: None,
            recording: true,
            cells: 7,
            dropped: 0,
            cell: Some(cell(13.2, -51.0)),
            stopped: None,
        };
        assert_eq!(
            survey.update(&update).map(|point| point.level_db),
            Some(-51.0)
        );
        let view = survey.view("map1");
        assert_eq!(
            (view.level_db, view.total, view.recording),
            (Some(-51.0), 7, true)
        );
        let stopped = SurveyUpdate {
            cell: None,
            stopped: Some(SurveyStop::Retuned),
            ..update
        };
        assert_eq!(survey.update(&stopped), None);
        assert!(!survey.view("map1").recording);
    }

    #[test]
    fn a_cleared_grid_forgets_its_range() {
        let mut survey = Survey::new(145.5e6, true, 0);
        let update = SurveyUpdate {
            level_dbfs: Some(-51.0),
            target_hz: None,
            recording: true,
            cells: 1,
            dropped: 0,
            cell: Some(cell(13.2, -51.0)),
            stopped: None,
        };
        survey.update(&update);
        assert_eq!(survey.view("map1").min_db, -51.0);
        let cleared = SurveyUpdate {
            cells: 0,
            cell: None,
            ..update.clone()
        };
        survey.update(&cleared);
        let view = survey.view("map1");
        assert_eq!((view.min_db, view.max_db, view.total), (0.0, 0.0, 0));
        survey.update(&SurveyUpdate {
            cells: 1,
            cell: Some(cell(13.3, -70.0)),
            ..update
        });
        let view = survey.view("map1");
        assert_eq!((view.min_db, view.max_db), (-70.0, -70.0));
    }
}
