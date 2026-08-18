use std::sync::Arc;

use sdrmm_wire::{
    MAX_SCAN_DEVICE_SETS, Range, ScanMember, ScanSession, ScanSessionStatus, ScanSettings,
    ScannerStatus, ServerEvent, StateScope,
};

use super::{ScanPlan, ScannerState, partition, spawn};
use crate::{DeviceSetState, DeviceSetStatus, Engine, EngineError};

pub(crate) struct SessionState {
    pub(crate) device_sets: Vec<u32>,
    pub(crate) settings: ScanSettings,
}

impl SessionState {
    pub(crate) fn project(&self) -> ScanSession {
        ScanSession {
            device_sets: self.device_sets.clone(),
            settings: self.settings.clone(),
        }
    }
}

fn members(sets: &[u32]) -> Result<Vec<u32>, EngineError> {
    let mut unique: Vec<u32> = Vec::with_capacity(sets.len());
    for &ds in sets {
        if unique.contains(&ds) {
            return Err(EngineError::Scan(format!(
                "device set {ds} is listed twice in the same scan"
            )));
        }
        unique.push(ds);
    }
    if unique.is_empty() {
        return Err(EngineError::Scan(
            "a scan needs at least one device set".to_string(),
        ));
    }
    if unique.len() > MAX_SCAN_DEVICE_SETS {
        return Err(EngineError::Scan(format!(
            "a scan spans at most {MAX_SCAN_DEVICE_SETS} device sets"
        )));
    }
    Ok(unique)
}

fn admits_a_scan(state: &DeviceSetState, ds: u32) -> Result<(), EngineError> {
    if state.scanner.is_some() {
        return Err(EngineError::Scan(format!(
            "device set {ds} is already scanning"
        )));
    }
    if state.hunt.is_some() {
        return Err(EngineError::Scan(format!(
            "device set {ds} is hunting; a scan needs the dial back"
        )));
    }
    if state.status != DeviceSetStatus::Running {
        return Err(EngineError::Scan(format!("device set {ds} is not running")));
    }
    if state.capabilities.per_stream.tuning {
        return Err(EngineError::Scan(format!(
            "device set {ds} tunes each receive stream independently, so a sweep of the shared \
             dial would retune every lane at once; scanning one stream is not supported yet"
        )));
    }
    Ok(())
}

struct Share {
    device_set: u32,
    settings: ScanSettings,
    plan: ScanPlan,
}

fn shares(
    engine: &Engine,
    sets: &[u32],
    settings: &ScanSettings,
    targets: &[f64],
) -> Result<Vec<Share>, EngineError> {
    let inner = engine.lock();
    if inner.scan_session.is_some() {
        return Err(EngineError::Scan("a scan is already running".to_string()));
    }
    let mut reach: Vec<Vec<Range>> = Vec::with_capacity(sets.len());
    let mut holds: Vec<bool> = Vec::with_capacity(sets.len());
    for &ds in sets {
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        admits_a_scan(state, ds)?;
        reach.push(state.capabilities.freq_ranges.clone());
        holds.push(
            settings
                .hold_channel
                .is_some_and(|ch| state.channels.iter().any(|c| c.id == ch)),
        );
    }
    if let Some(ch) = settings.hold_channel
        && !holds.iter().any(|has| *has)
    {
        return Err(EngineError::ChannelNotFound(ch, sets[0]));
    }
    if targets.len() < sets.len() {
        return Err(EngineError::Scan(format!(
            "{} targets cannot be spread over {} device sets; widen the ranges or scan with \
             fewer radios",
            targets.len(),
            sets.len()
        )));
    }
    let split = partition(targets, &reach)?;
    Ok(sets
        .iter()
        .zip(split)
        .zip(holds)
        .map(|((&device_set, share), holds)| Share {
            device_set,
            settings: ScanSettings {
                hold_channel: holds.then_some(settings.hold_channel).flatten(),
                ..settings.clone()
            },
            plan: ScanPlan { targets: share },
        })
        .collect())
}

pub(crate) fn start(
    engine: &Arc<Engine>,
    sets: &[u32],
    settings: ScanSettings,
) -> Result<ScanSessionStatus, EngineError> {
    let sets = members(sets)?;
    let plan = ScanPlan::build(&settings)?;
    let shares = shares(engine, &sets, &settings, &plan.targets)?;

    let mut started: Vec<(u32, ScannerState)> = Vec::with_capacity(shares.len());
    for share in shares {
        match spawn(engine, share.device_set, share.plan, share.settings) {
            Ok(worker) => started.push((share.device_set, worker)),
            Err(e) => {
                for (_, worker) in started {
                    worker.stop_and_join();
                }
                return Err(e);
            }
        }
    }

    let mut inner = engine.lock();
    if inner.scan_session.is_some() {
        drop(inner);
        for (_, worker) in started {
            worker.stop_and_join();
        }
        return Err(EngineError::Scan("a scan is already running".to_string()));
    }
    let mut members = Vec::with_capacity(started.len());
    let mut planted: Vec<u32> = Vec::with_capacity(started.len());
    for (ds, worker) in started {
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            let stragglers: Vec<ScannerState> = planted
                .iter()
                .filter_map(|id| inner.device_sets.get_mut(id).and_then(|s| s.scanner.take()))
                .collect();
            drop(inner);
            worker.stop_and_join();
            for straggler in stragglers {
                straggler.stop_and_join();
            }
            return Err(EngineError::DeviceSetNotFound(ds));
        };
        members.push(ScanMember {
            device_set: ds,
            status: worker.status(),
        });
        state.scanner = Some(worker);
        planted.push(ds);
    }
    inner.scan_session = Some(SessionState {
        device_sets: sets,
        settings: settings.clone(),
    });
    inner.revision += 1;
    drop(inner);

    for member in &members {
        engine.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(member.device_set),
        });
    }
    Ok(ScanSessionStatus { settings, members })
}

/// Takes a device set out of the running scan, ending the scan when it was the last one left.
pub(crate) fn detach(engine: &Engine, ds: u32) -> Option<ScannerState> {
    let mut inner = engine.lock();
    let worker = inner.device_sets.get_mut(&ds)?.scanner.take()?;
    if let Some(session) = inner.scan_session.as_mut() {
        session.device_sets.retain(|&id| id != ds);
        if session.device_sets.is_empty() {
            inner.scan_session = None;
        }
    }
    inner.revision += 1;
    Some(worker)
}

pub(crate) fn stop_one(engine: &Engine, ds: u32) -> Result<ScannerStatus, EngineError> {
    {
        let inner = engine.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        if state.scanner.is_none() {
            return Err(EngineError::Scan("no scan is running".to_string()));
        }
    }
    let worker =
        detach(engine, ds).ok_or_else(|| EngineError::Scan("no scan is running".to_string()))?;
    let status = worker.stop_and_join();
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

pub(crate) fn stop_all(engine: &Engine) -> Result<ScanSessionStatus, EngineError> {
    let (settings, sets) = {
        let mut inner = engine.lock();
        let session = inner
            .scan_session
            .take()
            .ok_or_else(|| EngineError::Scan("no scan is running".to_string()))?;
        inner.revision += 1;
        (session.settings, session.device_sets)
    };
    let mut members = Vec::with_capacity(sets.len());
    for ds in sets {
        let Some(worker) = ({
            let mut inner = engine.lock();
            inner
                .device_sets
                .get_mut(&ds)
                .and_then(|state| state.scanner.take())
        }) else {
            continue;
        };
        members.push(ScanMember {
            device_set: ds,
            status: worker.stop_and_join(),
        });
        engine.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }
    Ok(ScanSessionStatus { settings, members })
}
