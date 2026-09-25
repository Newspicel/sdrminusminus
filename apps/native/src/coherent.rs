use std::{collections::HashMap, time::SystemTime};

use sdrmm_wire::{
    coherent::{CalState, DfFusionState, DfReading, RadarDetection},
    ws::ServerEvent,
};

pub const BEARING_HISTORY: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct BearingSample {
    pub bearing_deg: f32,
    pub confidence: f32,
    pub at: SystemTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finder {
    pub device_set: u32,
    pub reading: DfReading,
    pub cal: CalState,
    pub heard: bool,
    pub history: Vec<BearingSample>,
    pub fusion: Option<DfFusionState>,
    pub detections: Vec<RadarDetection>,
}

impl Finder {
    fn quiet() -> Self {
        Self {
            device_set: 0,
            reading: DfReading {
                pseudospectrum: Vec::new(),
                ..DfReading::default()
            },
            cal: CalState {
                phase_unknown: true,
                ..CalState::default()
            },
            heard: false,
            history: Vec::new(),
            fusion: None,
            detections: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub by_node: HashMap<String, Finder>,
}

impl Book {
    pub fn observe(&mut self, event: &ServerEvent, now: SystemTime) -> bool {
        match event {
            ServerEvent::DfUpdate {
                device_set,
                node,
                reading,
                cal,
            } => {
                let finder = self.entry(node);
                if reading.confidence > 0.0 {
                    finder.history.push(BearingSample {
                        bearing_deg: reading.bearing_deg,
                        confidence: reading.confidence,
                        at: now,
                    });
                    let overflow = finder.history.len().saturating_sub(BEARING_HISTORY);
                    finder.history.drain(..overflow);
                }
                finder.device_set = *device_set;
                finder.reading = (**reading).clone();
                finder.cal = (**cal).clone();
                finder.heard = true;
                true
            }
            ServerEvent::DfFusionUpdate { node, state } => {
                self.entry(node).fusion = Some((**state).clone());
                true
            }
            ServerEvent::RadarDetections {
                device_set,
                node,
                detections,
            } => {
                let finder = self.entry(node);
                finder.device_set = *device_set;
                finder.detections = detections.clone();
                true
            }
            _ => false,
        }
    }

    fn entry(&mut self, node: &str) -> &mut Finder {
        self.by_node
            .entry(node.to_owned())
            .or_insert_with(Finder::quiet)
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{coherent::DfEstimate, device::Coherence};

    use super::*;

    fn reading(bearing_deg: f32, confidence: f32) -> DfReading {
        DfReading {
            bearing_deg,
            confidence,
            peak_to_floor_db: 20.0,
            pseudospectrum: vec![1, 2, 3],
        }
    }

    fn update(bearing_deg: f32, confidence: f32) -> ServerEvent {
        ServerEvent::DfUpdate {
            device_set: 1,
            node: "df".to_owned(),
            reading: Box::new(reading(bearing_deg, confidence)),
            cal: Box::new(CalState {
                tier: Coherence::PhaseCoherent,
                lanes: Vec::new(),
                phase_unknown: false,
                solved: true,
                reference_on: false,
            }),
        }
    }

    #[test]
    fn keeps_the_newest_reading_and_the_trail_behind_it() {
        let mut book = Book::default();
        book.observe(&update(10.0, 0.8), SystemTime::UNIX_EPOCH);
        book.observe(&update(20.0, 0.8), SystemTime::UNIX_EPOCH);
        let finder = &book.by_node["df"];
        assert_eq!(finder.reading.bearing_deg, 20.0);
        let trail: Vec<f32> = finder
            .history
            .iter()
            .map(|sample| sample.bearing_deg)
            .collect();
        assert_eq!(trail, vec![10.0, 20.0]);
        assert_eq!(finder.device_set, 1);
        assert!(finder.heard);
    }

    #[test]
    fn leaves_the_trail_alone_for_a_reading_with_no_confidence_in_it() {
        let mut book = Book::default();
        book.observe(&update(10.0, 0.8), SystemTime::UNIX_EPOCH);
        book.observe(&update(0.0, 0.0), SystemTime::UNIX_EPOCH);
        assert_eq!(book.by_node["df"].history.len(), 1);
    }

    #[test]
    fn caps_the_trail_so_a_long_drive_cannot_grow_without_bound() {
        let mut book = Book::default();
        for index in 0..BEARING_HISTORY + 20 {
            book.observe(&update((index % 360) as f32, 0.8), SystemTime::UNIX_EPOCH);
        }
        assert_eq!(book.by_node["df"].history.len(), BEARING_HISTORY);
    }

    #[test]
    fn keeps_the_last_bearing_when_only_the_fusion_moves() {
        let mut book = Book::default();
        book.observe(&update(30.0, 0.8), SystemTime::UNIX_EPOCH);
        book.observe(
            &ServerEvent::DfFusionUpdate {
                node: "df".to_owned(),
                state: Box::new(DfFusionState {
                    samples: 4,
                    estimate: Some(DfEstimate {
                        lat: 51.5,
                        lon: 7.0,
                        ellipse_major_m: 300.0,
                        ellipse_minor_m: 200.0,
                        ellipse_bearing_deg: 10.0,
                        converged: true,
                        samples: 4,
                    }),
                    ..DfFusionState::default()
                }),
            },
            SystemTime::UNIX_EPOCH,
        );
        let finder = &book.by_node["df"];
        assert_eq!(finder.reading.bearing_deg, 30.0);
        assert_eq!(
            finder
                .fusion
                .as_ref()
                .and_then(|fusion| fusion.estimate)
                .map(|estimate| estimate.converged),
            Some(true)
        );
    }

    #[test]
    fn a_fusion_alone_leaves_the_phase_unknown() {
        let mut book = Book::default();
        book.observe(
            &ServerEvent::DfFusionUpdate {
                node: "grid".to_owned(),
                state: Box::default(),
            },
            SystemTime::UNIX_EPOCH,
        );
        let finder = &book.by_node["grid"];
        assert!(!finder.heard);
        assert!(finder.cal.phase_unknown);
    }

    #[test]
    fn keeps_radar_detections_under_the_node_that_found_them() {
        let mut book = Book::default();
        book.observe(
            &ServerEvent::RadarDetections {
                device_set: 2,
                node: "radar".to_owned(),
                detections: vec![RadarDetection {
                    range_bin: 60,
                    range_km: 18.0,
                    doppler_hz: 120.0,
                    snr_db: 19.0,
                    track_id: None,
                }],
            },
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(book.by_node["radar"].detections.len(), 1);
    }

    #[test]
    fn ignores_every_other_event() {
        let mut book = Book::default();
        assert!(!book.observe(&ServerEvent::Hello { revision: 1 }, SystemTime::UNIX_EPOCH));
        assert!(book.by_node.is_empty());
    }
}
