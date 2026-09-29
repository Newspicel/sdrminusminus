use sdrmm_device::{DeviceError, LaneMark, lock};
use sdrmm_wire::{
    ArrayGain, ArrayTune, ArrayTuneRequest, DeviceSettings, GainValue, ServerEvent, StateScope,
    StreamScope, StreamSettings,
};

use super::{check_tune, find, find_mut, members};
use crate::{
    Engine, EngineError,
    array::{ControlConfig, LiveFrame, tuner::TunePlan},
    device_ops::lane_setup,
    lock_runtime,
};

fn kept_gains(changed: &[GainValue], before: &[GainValue]) -> Vec<GainValue> {
    changed
        .iter()
        .filter_map(|gain| before.iter().find(|held| held.stage == gain.stage).cloned())
        .collect()
}

const EVERY_LANE_SETTING: StreamScope = StreamScope {
    tuning: true,
    gain: true,
    antenna: false,
    agc: true,
};

fn restore_stream(previous: &DeviceSettings, entry: &StreamSettings) -> StreamSettings {
    let before = previous.for_stream(entry.stream, &EVERY_LANE_SETTING);
    StreamSettings {
        stream: entry.stream,
        center_hz: entry.center_hz.and(before.center_hz),
        tuning: entry.tuning.and(before.tuning),
        gains: kept_gains(&entry.gains, &before.gains),
        antenna: None,
        agc: entry.agc.as_ref().and(before.agc),
    }
}

pub(super) fn restore(previous: &DeviceSettings, delta: &DeviceSettings) -> DeviceSettings {
    DeviceSettings {
        center_hz: delta.center_hz.and(previous.center_hz),
        tuning: delta.tuning.and(previous.tuning),
        gains: kept_gains(&delta.gains, &previous.gains),
        agc: delta.agc.as_ref().and(previous.agc.clone()),
        streams: delta
            .streams
            .iter()
            .map(|entry| restore_stream(previous, entry))
            .collect(),
        ..DeviceSettings::default()
    }
}

impl Engine {
    pub fn tune_array(&self, node: &str, request: ArrayTuneRequest) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        let (current, cal) = {
            let inner = self.lock();
            let state = find(&inner, node)?;
            (state.tune, state.spec.settings.cal)
        };
        let tune = ArrayTune {
            center_hz: request.center_hz.unwrap_or(current.center_hz),
            gain: request.gain.unwrap_or(current.gain),
        };
        check_tune(&tune, &cal)?;
        self.retune(node, tune, tune.gain)
    }

    pub(crate) fn tune_array_internal(
        &self,
        node: &str,
        tune: ArrayTune,
    ) -> Result<(), EngineError> {
        let mode = find(&self.lock(), node)?.tune.gain;
        let kept = if mode == ArrayGain::Auto {
            ArrayGain::Auto
        } else {
            tune.gain
        };
        self.retune(node, tune, kept)
    }

    pub(super) fn retune(
        &self,
        node: &str,
        tune: ArrayTune,
        kept: ArrayGain,
    ) -> Result<(), EngineError> {
        let (mode, survey) = {
            let inner = self.lock();
            let state = find(&inner, node)?;
            (
                state.spec.settings.tuning,
                members::survey(&inner, &state.spec)?,
            )
        };
        let plan = survey.plan(mode, &tune)?;
        let capabilities_changed = self.apply_plan(&plan, Some(node))?;
        {
            let mut inner = self.lock();
            let state = find_mut(&mut inner, node)?;
            state.tune = ArrayTune {
                center_hz: tune.center_hz,
                gain: kept,
            };
            if let Some(db) = plan.gain_db {
                state.board.control().gain_db = Some(db);
            }
            let frame = LiveFrame {
                center_hz: tune.center_hz,
                lane_centers_hz: plan.lane_centers_hz.clone(),
                ..state.frame.clone()
            };
            let config = ControlConfig {
                gain: kept,
                ..state.config.clone()
            };
            state.configure(config);
            state.reframe(frame)?;
            inner.revision += 1;
        }
        if capabilities_changed {
            self.retier(node)?;
        }
        self.refresh_lanes(node);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        Ok(())
    }

    fn device_settings(&self, ds: u32) -> Result<DeviceSettings, EngineError> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(|state| state.settings.clone())
            .ok_or(EngineError::DeviceSetNotFound(ds))
    }

    pub(super) fn apply_plan(
        &self,
        plan: &TunePlan,
        marks: Option<&str>,
    ) -> Result<bool, EngineError> {
        let mut done: Vec<(u32, DeviceSettings)> = Vec::with_capacity(plan.deltas.len());
        let mut capabilities_changed = false;
        for (ds, delta) in &plan.deltas {
            let previous = self.device_settings(*ds)?;
            match self.patch_device_quietly(*ds, delta.clone()) {
                Ok(patched) => capabilities_changed |= patched.capabilities_changed,
                Err(error) => {
                    self.roll_back(&done, plan);
                    return Err(error);
                }
            }
            if let Some(node) = marks {
                self.mark_retune(node, *ds, &previous);
            }
            done.push((*ds, previous));
        }
        Ok(capabilities_changed)
    }

    fn roll_back(&self, done: &[(u32, DeviceSettings)], plan: &TunePlan) {
        for (ds, previous) in done.iter().rev() {
            let Some((_, delta)) = plan.deltas.iter().find(|(planned, _)| planned == ds) else {
                continue;
            };
            if let Err(error) = self.patch_device_quietly(*ds, restore(previous, delta)) {
                self.mark_device_fault(
                    *ds,
                    DeviceError::Io(format!("an array retune could not be undone: {error}")),
                );
            }
        }
    }

    fn mark_retune(&self, node: &str, ds: u32, previous: &DeviceSettings) {
        let (runtime, marks, board) = {
            let inner = self.lock();
            let Some(state) = inner.device_sets.get(&ds) else {
                return;
            };
            let scope = state.capabilities.per_stream;
            let marks: Vec<(u32, bool, bool)> = state
                .held
                .iter()
                .filter(|(_, array)| *array == node)
                .map(|(stream, _)| {
                    let before = lane_setup(previous, *stream, &scope);
                    let after = lane_setup(&state.settings, *stream, &scope);
                    (
                        *stream,
                        before.tuned != after.tuned,
                        before.front_end != after.front_end,
                    )
                })
                .filter(|(_, retuned, gain)| *retuned || *gain)
                .collect();
            (
                state.runtime.clone(),
                marks,
                inner.arrays.get(node).map(|array| array.board.clone()),
            )
        };
        if marks.is_empty() {
            return;
        }
        let (posters, in_flight) = {
            let runtime = lock_runtime(&runtime);
            (runtime.mark_posters(), runtime.in_flight_samples())
        };
        for (stream, retuned, gain) in marks {
            let wanted = [
                retuned.then_some(LaneMark::Retuned { in_flight }),
                gain.then_some(LaneMark::GainChanged { in_flight }),
            ];
            for mark in wanted.into_iter().flatten() {
                let posted = posters
                    .get(stream as usize)
                    .ok_or_else(|| DeviceError::Io("lane has no mark queue".to_owned()))
                    .and_then(|poster| poster.post(mark));
                if let Err(error) = posted {
                    tracing::warn!(array = %node, ds, stream, %error, "a retune mark was lost");
                    if let Some(board) = &board {
                        board.add_events_lost(1);
                    }
                }
            }
        }
    }
}
