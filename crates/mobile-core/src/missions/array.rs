use sdrmm_wire::array::{ArrayStatus, CalPhase, ProcessorGate, SyncState};

use super::views::DfState;

pub(crate) fn gate(status: &ArrayStatus, node: &str) -> Option<DfState> {
    let gated = status
        .processors
        .iter()
        .find(|processor| processor.node == node)
        .and_then(|processor| processor.gated);
    match gated {
        Some(ProcessorGate::Calibrating) => return Some(DfState::Calibrating),
        Some(ProcessorGate::Phase) => return Some(DfState::PhaseUnknown),
        Some(
            ProcessorGate::Sync
            | ProcessorGate::Gain
            | ProcessorGate::Retuning
            | ProcessorGate::Tier
            | ProcessorGate::TuningMode,
        ) => return Some(DfState::Waiting),
        None => {}
    }
    match (status.cal, status.phase_ready, status.sync) {
        (CalPhase::Measuring, _, _) => Some(DfState::Calibrating),
        (_, false, _) => Some(DfState::PhaseUnknown),
        (_, true, SyncState::Idle | SyncState::Searching | SyncState::Lost) => {
            Some(DfState::Waiting)
        }
        (
            CalPhase::None
            | CalPhase::Waiting
            | CalPhase::Solved
            | CalPhase::Warm
            | CalPhase::Stale
            | CalPhase::Failed,
            true,
            SyncState::Locked | SyncState::Drifting,
        ) => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use sdrmm_wire::array::{ArrayGain, ArrayTuningMode, ProcessorStatus};
    use sdrmm_wire::device::Coherence;

    use super::*;

    pub(crate) fn status(node: &str) -> ArrayStatus {
        ArrayStatus {
            node: node.to_owned(),
            lanes: Vec::new(),
            anchor: None,
            tier: Coherence::default(),
            declared: Coherence::default(),
            tier_capped: false,
            sync: SyncState::Locked,
            cal: CalPhase::Solved,
            phase_ready: true,
            center_hz: 433.92e6,
            sample_rate: 2.4e6,
            tuning: ArrayTuningMode::default(),
            gain: ArrayGain::default(),
            gain_db: None,
            gain_range_db: None,
            generation: 1,
            realigns: 0,
            dropped_samples: 0,
            events_lost: 0,
            drift_ppm: None,
            last_solve_at: None,
            next_check_in_s: None,
            azimuth_deg: None,
            heading_source: None,
            position: None,
            unambiguous_hz: None,
            failure: None,
            processors: Vec::new(),
            recording: None,
        }
    }

    pub(crate) fn processor(node: &str, gated: Option<ProcessorGate>) -> ProcessorStatus {
        ProcessorStatus {
            node: node.to_owned(),
            kind: "df".to_owned(),
            running: true,
            gated,
            gated_samples: 0,
            dropped_samples: 0,
            dropped_reports: 0,
            lane_overflows: 0,
            lane_mismatch: 0,
            solver_failures: 0,
            resets: 0,
            truncated: 0,
            error: None,
        }
    }

    #[test]
    fn every_wire_state_maps() {
        let ready = status("arr");
        assert_eq!(gate(&ready, "df1"), None);
        let cases = [
            (Some(ProcessorGate::Calibrating), Some(DfState::Calibrating)),
            (Some(ProcessorGate::Phase), Some(DfState::PhaseUnknown)),
            (Some(ProcessorGate::Sync), Some(DfState::Waiting)),
            (Some(ProcessorGate::Retuning), Some(DfState::Waiting)),
            (None, None),
        ];
        for (gated, expected) in cases {
            let mut gated_status = status("arr");
            gated_status.processors = vec![
                processor("df1", gated),
                processor("other", Some(ProcessorGate::Phase)),
            ];
            assert_eq!(gate(&gated_status, "df1"), expected, "{gated:?}");
        }
        let mut measuring = status("arr");
        measuring.cal = CalPhase::Measuring;
        assert_eq!(gate(&measuring, "df1"), Some(DfState::Calibrating));
        let mut unknown = status("arr");
        unknown.phase_ready = false;
        assert_eq!(gate(&unknown, "df1"), Some(DfState::PhaseUnknown));
        let mut lost = status("arr");
        lost.sync = SyncState::Lost;
        assert_eq!(gate(&lost, "df1"), Some(DfState::Waiting));
        let mut drifting = status("arr");
        drifting.sync = SyncState::Drifting;
        assert_eq!(gate(&drifting, "df1"), None);
    }
}
