use std::sync::{Arc, Mutex};

use sdrmm_device::{SdrDevice, SweepBand, SweepPlan};
use tokio::sync::broadcast;

use crate::{
    CaptureRuntime, DeviceSetStatus, Engine, EngineError, RebuildEntry, lock_runtime,
    plan_front_end, runtime::SpectrumSnapshot, sample_rate_of,
};

/// How far apart two targets have to sit before sweeping the gap costs more than tuning across it.
const BAND_GAP_SPANS: f64 = 4.0;

/// Groups the targets into the stretches worth sweeping, so a plan that covers two bands does not
/// spend its time on the empty spectrum between them.
pub(crate) fn bands(targets: &[f64], span_hz: f64, measure_bw_hz: f64) -> Vec<SweepBand> {
    let mut bands: Vec<SweepBand> = Vec::new();
    let edge = (measure_bw_hz / 2.0).max(span_hz / 8.0);
    let gap = span_hz * BAND_GAP_SPANS;
    for &target in targets {
        match bands.last_mut() {
            Some(band) if target - band.stop_hz <= gap => band.stop_hz = target + edge,
            _ => bands.push(SweepBand {
                start_hz: target - edge,
                stop_hz: target + edge,
            }),
        }
    }
    bands
}

fn fault_handler(
    engine: &Engine,
    ds: u32,
) -> impl FnOnce(sdrmm_device::DeviceError) + Send + 'static {
    let fault_tx = engine.fault_tx.clone();
    move |err| {
        let _ = fault_tx.send((ds, err));
    }
}

/// Hands the sweep to the radio's firmware, taking the receive stream down for the duration. The
/// spectrum tap survives the switch; the channels do not, and are rebuilt on the way back.
pub(crate) fn enter(engine: &Engine, ds: u32, plan: &SweepPlan) -> Result<(), EngineError> {
    let runtime = {
        let inner = engine.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        if !state.capabilities.hardware_sweep {
            return Err(EngineError::Scan(
                "this radio has no firmware sweep".to_string(),
            ));
        }
        if state.status != DeviceSetStatus::Running {
            return Err(EngineError::Scan(
                "the device set is not running".to_string(),
            ));
        }
        state.runtime.clone()
    };
    let (device, taps) = {
        let mut current = lock_runtime(&runtime);
        if current.is_sweeping() {
            return Ok(());
        }
        let taps = current.taps();
        (current.release_device(), taps)
    };
    let device =
        device.ok_or_else(|| EngineError::Scan("the radio is already down".to_string()))?;
    match CaptureRuntime::start_sweep(device, plan, taps.clone(), fault_handler(engine, ds)) {
        Ok(sweeping) => swap_runtime(engine, ds, sweeping),
        Err((device, refused)) => {
            restore_receiving(engine, ds, device, taps)?;
            Err(EngineError::Device(refused))
        }
    }
}

/// Puts a radio that refused to sweep straight back on the air, so asking never costs the stream.
fn restore_receiving(
    engine: &Engine,
    ds: u32,
    mut device: Box<dyn SdrDevice>,
    taps: Vec<broadcast::Sender<SpectrumSnapshot>>,
) -> Result<(), EngineError> {
    let (settings, channels) = {
        let inner = engine.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        (state.settings.clone(), state.channels.clone())
    };
    let front_end = plan_front_end(device.capabilities(), &settings, &channels);
    device
        .apply(&settings.to_hardware(front_end.lo_offset_hz))
        .map_err(EngineError::Device)?;
    let receiving = CaptureRuntime::start_with_taps(
        device,
        &settings,
        front_end,
        taps,
        fault_handler(engine, ds),
    )
    .map_err(EngineError::Device)?;
    swap_runtime(engine, ds, receiving)?;
    rebuild_channels(engine, ds);
    Ok(())
}

/// Puts the receive stream back and rebuilds every channel the sweep displaced.
pub(crate) fn leave(engine: &Engine, ds: u32) -> Result<(), EngineError> {
    let runtime = {
        let inner = engine.lock();
        inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?
            .runtime
            .clone()
    };
    let device = {
        let mut current = lock_runtime(&runtime);
        if !current.is_sweeping() {
            return Ok(());
        }
        current.release_device()
    };
    let device =
        device.ok_or_else(|| EngineError::Scan("the radio is already down".to_string()))?;
    let taps = lock_runtime(&runtime).taps();
    restore_receiving(engine, ds, device, taps)
}

fn swap_runtime(engine: &Engine, ds: u32, runtime: CaptureRuntime) -> Result<(), EngineError> {
    let cmd_txs = runtime.command_senders();
    let overruns = runtime.overruns_counters();
    let stalls = runtime.stall_counters();
    let runtime = Arc::new(Mutex::new(runtime));
    let replaced = {
        let mut inner = engine.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            drop(inner);
            lock_runtime(&runtime).stop();
            return Err(EngineError::DeviceSetNotFound(ds));
        };
        state.cmd_txs = cmd_txs;
        state.overruns = overruns;
        state.stalls = stalls;
        let replaced = std::mem::replace(&mut state.runtime, runtime);
        inner.revision += 1;
        replaced
    };
    lock_runtime(&replaced).stop();
    Ok(())
}

fn rebuild_channels(engine: &Engine, ds: u32) {
    let (rebuilds, rate) = {
        let inner = engine.lock();
        let Some(state) = inner.device_sets.get(&ds) else {
            return;
        };
        let rebuilds: Vec<RebuildEntry> = state
            .channels
            .iter()
            .filter_map(|c| {
                state.media.get(&c.id).map(|m| RebuildEntry {
                    id: c.id,
                    stream: c.stream,
                    settings: c.settings.clone(),
                    sinks: m.sinks.clone(),
                })
            })
            .collect();
        (rebuilds, sample_rate_of(&state.settings))
    };
    let mut dead = Vec::new();
    for rebuild in rebuilds {
        engine.rebuild_channel(ds, rebuild, rate, &mut dead);
    }
    for handle in dead {
        handle.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_targets_share_one_band() {
        let targets: Vec<f64> = (0..9).map(|i| 144e6 + f64::from(i) * 25e3).collect();
        let bands = bands(&targets, 2e6, 12.5e3);
        assert_eq!(bands.len(), 1, "one band covers a run of close targets");
        assert!(bands[0].start_hz < targets[0]);
        assert!(bands[0].stop_hz > targets[8]);
    }

    #[test]
    fn distant_targets_get_a_band_each_instead_of_the_empty_gap() {
        let bands = bands(&[144e6, 144.025e6, 433e6, 433.05e6], 2e6, 12.5e3);
        assert_eq!(
            bands.len(),
            2,
            "sweeping 289 MHz of nothing wastes the pass"
        );
        assert!(bands[0].stop_hz < 145e6);
        assert!(bands[1].start_hz > 432e6);
    }

    #[test]
    fn every_target_lands_inside_a_band() {
        let targets = vec![88e6, 90e6, 108e6, 433.5e6];
        for band in bands(&targets, 2e6, 12.5e3) {
            assert!(band.stop_hz > band.start_hz);
        }
        let covered = |hz: f64| {
            bands(&targets, 2e6, 12.5e3)
                .iter()
                .any(|b| hz >= b.start_hz && hz <= b.stop_hz)
        };
        assert!(targets.iter().copied().all(covered));
    }

    #[test]
    fn a_lone_target_still_makes_a_band_wide_enough_to_sweep() {
        let bands = bands(&[433.92e6], 2e6, 12.5e3);
        assert_eq!(bands.len(), 1);
        assert!(
            bands[0].stop_hz - bands[0].start_hz >= 2e6 / 4.0,
            "a hairline band leaves the firmware nothing to step through"
        );
    }
}
