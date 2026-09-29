use std::{
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering},
    },
    time::Instant,
};

use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_device::lock;
use sdrmm_wire::{
    ArrayFailure, ArrayLaneStatus, ArrayRecordingStatus, ArrayStatus, CalPhase, ProcessorGate,
    SyncState,
};

const MILLI: f64 = 1_000.0;
const CENTI: f64 = 100.0;

#[derive(Default)]
pub(crate) struct LaneBoard {
    pub(crate) delay_milli: AtomicI64,
    pub(crate) phase_mdeg: AtomicI32,
    pub(crate) gain_mdb: AtomicI32,
    pub(crate) coherence_milli: AtomicU32,
    pub(crate) residual_delay_milli: AtomicI64,
    pub(crate) residual_phase_mdeg: AtomicI32,
    pub(crate) has_residual: AtomicBool,
    pub(crate) sync: AtomicU8,
    pub(crate) level_cdb: AtomicI32,
    pub(crate) clipping: AtomicBool,
    pub(crate) gaps: AtomicU64,
    pub(crate) gap_samples: AtomicU64,
    pub(crate) uncertain: AtomicU64,
}

impl LaneBoard {
    pub(crate) fn set_solution(&self, delay: f64, phase_deg: f64, gain_db: f64, coherence: f32) {
        self.delay_milli
            .store(scaled_i64(delay, MILLI), Ordering::Relaxed);
        self.phase_mdeg
            .store(scaled_i32(phase_deg, MILLI), Ordering::Relaxed);
        self.gain_mdb
            .store(scaled_i32(gain_db, MILLI), Ordering::Relaxed);
        self.coherence_milli.store(
            scaled_i64(f64::from(coherence.max(0.0)), MILLI).clamp(0, i64::from(u32::MAX)) as u32,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn set_residual(&self, delay: f64, phase_deg: f64) {
        self.residual_delay_milli
            .store(scaled_i64(delay, MILLI), Ordering::Relaxed);
        self.residual_phase_mdeg
            .store(scaled_i32(phase_deg, MILLI), Ordering::Relaxed);
        self.has_residual.store(true, Ordering::Relaxed);
    }

    pub(crate) fn set_level(&self, level_dbfs: f32, clipping: bool) {
        self.level_cdb
            .store(scaled_i32(f64::from(level_dbfs), CENTI), Ordering::Relaxed);
        self.clipping.store(clipping, Ordering::Relaxed);
    }

    pub(crate) fn add_gap(&self, missing: u64) {
        self.gaps.fetch_add(1, Ordering::Relaxed);
        self.gap_samples.fetch_add(missing, Ordering::Relaxed);
    }

    pub(crate) fn add_uncertain(&self) {
        self.uncertain.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn sync(&self) -> SyncState {
        sync_of(self.sync.load(Ordering::Relaxed))
    }

    pub(crate) fn set_sync(&self, state: SyncState) {
        self.sync.store(sync_code(state), Ordering::Relaxed);
    }

    fn fill(&self, status: &mut ArrayLaneStatus) {
        status.sync = self.sync();
        status.delay_samples = unscaled(self.delay_milli.load(Ordering::Relaxed), MILLI);
        status.phase_deg =
            unscaled(i64::from(self.phase_mdeg.load(Ordering::Relaxed)), MILLI) as f32;
        status.gain_db = unscaled(i64::from(self.gain_mdb.load(Ordering::Relaxed)), MILLI) as f32;
        status.coherence = unscaled(
            i64::from(self.coherence_milli.load(Ordering::Relaxed)),
            MILLI,
        ) as f32;
        let residual = self.has_residual.load(Ordering::Relaxed);
        status.residual_delay = residual
            .then(|| unscaled(self.residual_delay_milli.load(Ordering::Relaxed), MILLI) as f32);
        status.residual_phase_deg = residual.then(|| {
            unscaled(
                i64::from(self.residual_phase_mdeg.load(Ordering::Relaxed)),
                MILLI,
            ) as f32
        });
        status.level_dbfs =
            unscaled(i64::from(self.level_cdb.load(Ordering::Relaxed)), CENTI) as f32;
        status.clipping = self.clipping.load(Ordering::Relaxed);
        status.gaps = self.gaps.load(Ordering::Relaxed);
        status.gap_samples = self.gap_samples.load(Ordering::Relaxed);
        status.uncertain = self.uncertain.load(Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct ControlStatus {
    pub(crate) failure: Option<ArrayFailure>,
    pub(crate) drift_ppm: Option<f64>,
    pub(crate) last_solve_at: Option<String>,
    pub(crate) next_check_at: Option<Instant>,
    pub(crate) recording: Option<ArrayRecordingStatus>,
    pub(crate) gain_db: Option<f64>,
}

pub(crate) struct StatusBoard {
    pub(crate) lanes: [LaneBoard; MAX_LANES],
    pub(crate) lane_count: AtomicU8,
    pub(crate) sync: AtomicU8,
    pub(crate) cal: AtomicU8,
    pub(crate) phase_ready: AtomicBool,
    pub(crate) generation: AtomicU32,
    pub(crate) realigns: AtomicU64,
    pub(crate) dropped_samples: AtomicU64,
    pub(crate) events_lost: AtomicU64,
    pub(crate) aligned: AtomicU64,
    pub(crate) alive: AtomicBool,
    pub(crate) busy: AtomicBool,
    pub(crate) control: Mutex<ControlStatus>,
}

impl StatusBoard {
    pub(crate) fn new(lanes: usize) -> Self {
        Self {
            lanes: std::array::from_fn(|_| LaneBoard::default()),
            lane_count: AtomicU8::new(u8::try_from(lanes.min(MAX_LANES)).unwrap_or(u8::MAX)),
            sync: AtomicU8::new(sync_code(SyncState::Idle)),
            cal: AtomicU8::new(cal_code(CalPhase::None)),
            phase_ready: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            realigns: AtomicU64::new(0),
            dropped_samples: AtomicU64::new(0),
            events_lost: AtomicU64::new(0),
            aligned: AtomicU64::new(0),
            alive: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            control: Mutex::new(ControlStatus::default()),
        }
    }

    pub(crate) fn lane_count(&self) -> usize {
        usize::from(self.lane_count.load(Ordering::Relaxed)).min(MAX_LANES)
    }

    pub(crate) fn lane(&self, lane: usize) -> Option<&LaneBoard> {
        self.lanes.get(lane)
    }

    pub(crate) fn sync(&self) -> SyncState {
        sync_of(self.sync.load(Ordering::Relaxed))
    }

    pub(crate) fn set_sync(&self, state: SyncState) {
        self.sync.store(sync_code(state), Ordering::Relaxed);
    }

    pub(crate) fn set_all_lanes(&self, state: SyncState) {
        for lane in &self.lanes[..self.lane_count()] {
            lane.set_sync(state);
        }
        self.set_sync(state);
    }

    pub(crate) fn cal(&self) -> CalPhase {
        cal_of(self.cal.load(Ordering::Relaxed))
    }

    pub(crate) fn set_cal(&self, phase: CalPhase) {
        self.cal.store(cal_code(phase), Ordering::Relaxed);
    }

    pub(crate) fn phase_ready(&self) -> bool {
        self.phase_ready.load(Ordering::Relaxed)
    }

    pub(crate) fn generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) fn add_events_lost(&self, count: u64) {
        if count > 0 {
            self.events_lost.fetch_add(count, Ordering::Relaxed);
        }
    }

    pub(crate) fn add_aligned(&self, samples: u64) {
        self.aligned.fetch_add(samples, Ordering::Relaxed);
    }

    pub(crate) fn aligned(&self) -> u64 {
        self.aligned.load(Ordering::Relaxed)
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub(crate) fn control(&self) -> MutexGuard<'_, ControlStatus> {
        lock(&self.control)
    }

    pub(crate) fn set_failure(&self, failure: Option<ArrayFailure>) {
        self.control().failure = failure;
    }

    pub(crate) fn stopped(&self, message: String) {
        self.alive.store(false, Ordering::Relaxed);
        self.set_failure(Some(ArrayFailure::Stopped { message }));
    }

    pub(crate) fn fill(&self, status: &mut ArrayStatus) {
        let lanes = self.lane_count();
        if status.lanes.len() < lanes {
            for lane in status.lanes.len()..lanes {
                status.lanes.push(ArrayLaneStatus {
                    lane: lane as u32,
                    ..ArrayLaneStatus::default()
                });
            }
        }
        for entry in &mut status.lanes {
            if let Some(lane) = self.lanes.get(entry.lane as usize) {
                lane.fill(entry);
            }
        }
        status.sync = self.sync();
        status.cal = self.cal();
        status.phase_ready = self.phase_ready();
        status.generation = self.generation();
        status.realigns = self.realigns.load(Ordering::Relaxed);
        status.dropped_samples = self.dropped_samples.load(Ordering::Relaxed);
        status.events_lost = self.events_lost.load(Ordering::Relaxed);
        let control = self.control();
        status.failure = control.failure.clone().or_else(|| {
            if !self.alive() {
                Some(ArrayFailure::Stopped {
                    message: String::new(),
                })
            } else if self.busy.load(Ordering::Relaxed) {
                Some(ArrayFailure::Busy)
            } else {
                None
            }
        });
        status.drift_ppm = control.drift_ppm;
        status.last_solve_at.clone_from(&control.last_solve_at);
        status.next_check_in_s = control
            .next_check_at
            .map(|at| at.saturating_duration_since(Instant::now()).as_secs_f64());
        status.recording.clone_from(&control.recording);
        status.gain_db = control.gain_db;
    }
}

fn scaled_i64(value: f64, scale: f64) -> i64 {
    let scaled = (value * scale).round();
    if scaled.is_finite() {
        scaled.clamp(i64::MIN as f64, i64::MAX as f64) as i64
    } else {
        0
    }
}

fn scaled_i32(value: f64, scale: f64) -> i32 {
    scaled_i64(value, scale).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn unscaled(value: i64, scale: f64) -> f64 {
    value as f64 / scale
}

pub(crate) fn sync_code(state: SyncState) -> u8 {
    SyncState::ALL
        .iter()
        .position(|known| *known == state)
        .unwrap_or(0) as u8
}

pub(crate) fn sync_of(code: u8) -> SyncState {
    SyncState::ALL
        .get(usize::from(code))
        .copied()
        .unwrap_or_default()
}

pub(crate) fn cal_code(phase: CalPhase) -> u8 {
    CalPhase::ALL
        .iter()
        .position(|known| *known == phase)
        .unwrap_or(0) as u8
}

pub(crate) fn cal_of(code: u8) -> CalPhase {
    CalPhase::ALL
        .get(usize::from(code))
        .copied()
        .unwrap_or_default()
}

pub(crate) fn gate_code(gate: Option<ProcessorGate>) -> u8 {
    gate.and_then(|gate| ProcessorGate::ALL.iter().position(|known| *known == gate))
        .map_or(0, |index| index as u8 + 1)
}

pub(crate) fn gate_of(code: u8) -> Option<ProcessorGate> {
    usize::from(code)
        .checked_sub(1)
        .and_then(|index| ProcessorGate::ALL.get(index).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_every_state() {
        for state in SyncState::ALL {
            assert_eq!(sync_of(sync_code(state)), state);
        }
        for phase in CalPhase::ALL {
            assert_eq!(cal_of(cal_code(phase)), phase);
        }
        assert_eq!(gate_of(gate_code(None)), None);
        for gate in ProcessorGate::ALL {
            assert_eq!(gate_of(gate_code(Some(gate))), Some(gate));
        }
    }

    #[test]
    fn the_board_fills_lanes_counters_and_failures() {
        let board = StatusBoard::new(3);
        board.lanes[1].set_solution(12.25, -40.5, -1.5, 0.95);
        board.lanes[1].set_level(-20.0, true);
        board.lanes[2].add_gap(100);
        board.lanes[2].set_sync(SyncState::Lost);
        board.add_events_lost(3);
        board.busy.store(true, Ordering::Relaxed);
        let mut status = ArrayStatus::default();
        board.fill(&mut status);
        assert_eq!(status.lanes.len(), 3);
        assert!((status.lanes[1].delay_samples - 12.25).abs() < 1e-9);
        assert!((status.lanes[1].phase_deg + 40.5).abs() < 1e-3);
        assert!(status.lanes[1].clipping);
        assert_eq!(status.lanes[2].gaps, 1);
        assert_eq!(status.lanes[2].gap_samples, 100);
        assert_eq!(status.lanes[2].sync, SyncState::Lost);
        assert_eq!(status.events_lost, 3);
        assert_eq!(status.failure, Some(ArrayFailure::Busy));
        board.stopped("boom".to_owned());
        board.fill(&mut status);
        assert_eq!(
            status.failure,
            Some(ArrayFailure::Stopped {
                message: "boom".to_owned()
            })
        );
    }
}
