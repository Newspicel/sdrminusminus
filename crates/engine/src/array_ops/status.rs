use sdrmm_wire::{ArrayLaneStatus, ArrayOrientation, ArrayStatus, ProcessorStatus};

use super::{ArrayState, processors::ProcessorRecord};
use crate::{
    Engine,
    array::{ArrayRuntime, tuner::GainMenu},
};

const STOPPED: &str = "Stopped";

fn processor_status(record: &ProcessorRecord) -> ProcessorStatus {
    let mut status = record
        .stats
        .status(&record.spec.node, record.kind, record.error.clone());
    status.running &= record.installed;
    status
}

impl ArrayState {
    fn lane_statuses(&self) -> Vec<ArrayLaneStatus> {
        self.spec
            .lanes
            .iter()
            .enumerate()
            .map(|(slot, lane)| ArrayLaneStatus {
                lane: slot as u32,
                device_set: lane.map(|lane| lane.device_set),
                stream: lane.map_or(0, |lane| lane.stream),
                ..ArrayLaneStatus::default()
            })
            .collect()
    }

    fn orientation(&self, status: &mut ArrayStatus) {
        let pose = self.pose.last();
        status.position = pose.map(|pose| pose.position);
        match self.spec.settings.orientation {
            ArrayOrientation::Fixed { azimuth_deg } => {
                status.azimuth_deg = Some(azimuth_deg.rem_euclid(360.0));
            }
            ArrayOrientation::Heading { mount_offset_deg } => {
                status.azimuth_deg = pose
                    .and_then(|pose| pose.heading_deg)
                    .map(|heading| (heading + mount_offset_deg).rem_euclid(360.0));
                status.heading_source = pose.and_then(|pose| pose.heading_source);
            }
        }
    }

    pub(super) fn status(&self) -> ArrayStatus {
        if self.runtime.as_ref().is_none_or(ArrayRuntime::is_finished) && self.board.alive() {
            self.board.stopped(STOPPED.to_owned());
        }
        let mut status = ArrayStatus {
            node: self.spec.node.clone(),
            lanes: self.lane_statuses(),
            anchor: self.anchor,
            tier: self.tier.tier,
            declared: self.spec.settings.declared,
            tier_capped: self.tier.tier < self.spec.settings.declared,
            center_hz: self.tune.center_hz,
            sample_rate: self.frame.sample_rate,
            tuning: self.spec.settings.tuning,
            gain: self.tune.gain,
            gain_range_db: self.gain_menu.as_ref().and_then(GainMenu::range),
            unambiguous_hz: self
                .spec
                .settings
                .geometry
                .unambiguous_hz(self.spec.lanes.len()),
            processors: self.processors.values().map(processor_status).collect(),
            ..ArrayStatus::default()
        };
        self.board.fill(&mut status);
        if self.failure.is_some() && self.board.alive() {
            status.failure.clone_from(&self.failure);
        }
        if !self.board.alive() {
            for processor in &mut status.processors {
                processor.running = false;
            }
        }
        status.recording = self.recording.as_ref().map(super::ArrayRecording::status);
        self.orientation(&mut status);
        status
    }
}

impl Engine {
    #[must_use]
    pub fn array_statuses(&self) -> Vec<ArrayStatus> {
        self.lock()
            .arrays
            .values()
            .map(ArrayState::status)
            .collect()
    }
}
