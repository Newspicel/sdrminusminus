use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_wire::{HuntSettings, HuntStatus, ServerEvent, StateScope};
use tokio::sync::broadcast::error::TryRecvError;

use crate::{DeviceSetStatus, Engine, EngineError};

/// How much of the previous reading survives into the next one. A hunt is walked with, and a
/// meter that jumps on every fade tells the operator about the multipath, not about the distance.
const SMOOTHING: f32 = 0.25;
const SETTLE: Duration = Duration::from_millis(60);
const SPECTRUM_TIMEOUT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(2);
const MIN_INTERVAL: Duration = Duration::from_millis(20);
const MAX_INTERVAL: Duration = Duration::from_millis(1_000);
/// How far the reading has to climb before the hunt calls it closing rather than noise.
const CLOSING_DB: f32 = 0.5;
/// A meter needs a range to sit in before the first reading has shown it one.
const MIN_RANGE_DB: f32 = 6.0;

pub(crate) struct HuntState {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<HuntStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl HuntState {
    pub(crate) fn status(&self) -> HuntStatus {
        lock_status(&self.status).clone()
    }

    pub(crate) fn stop_and_join(mut self) -> HuntStatus {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("hunt thread panicked");
        }
        self.status()
    }
}

fn lock_status(status: &Mutex<HuntStatus>) -> std::sync::MutexGuard<'_, HuntStatus> {
    status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn check(settings: &HuntSettings) -> Result<(), EngineError> {
    let bad = |msg: String| EngineError::Scan(msg);
    if !settings.freq_hz.is_finite() || settings.freq_hz <= 0.0 {
        return Err(bad(format!(
            "hunt frequency {} is not a usable Hz value",
            settings.freq_hz
        )));
    }
    if !settings.bw_hz.is_finite() || settings.bw_hz <= 0.0 {
        return Err(bad(format!(
            "hunt bandwidth must be positive, got {}",
            settings.bw_hz
        )));
    }
    Ok(())
}

pub(crate) fn start(
    engine: &Arc<Engine>,
    ds: u32,
    settings: HuntSettings,
) -> Result<HuntStatus, EngineError> {
    check(&settings)?;
    admits_a_hunt(engine, ds, &settings)?;
    let hunt = spawn(engine, ds, settings)?;
    let status = hunt.status();
    {
        let mut inner = engine.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            drop(inner);
            hunt.stop_and_join();
            return Err(EngineError::DeviceSetNotFound(ds));
        };
        if state.hunt.is_some() {
            drop(inner);
            hunt.stop_and_join();
            return Err(EngineError::Scan("a hunt is already running".to_string()));
        }
        state.hunt = Some(hunt);
        inner.revision += 1;
    }
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

fn admits_a_hunt(engine: &Engine, ds: u32, settings: &HuntSettings) -> Result<(), EngineError> {
    let inner = engine.lock();
    let state = inner
        .device_sets
        .get(&ds)
        .ok_or(EngineError::DeviceSetNotFound(ds))?;
    if state.hunt.is_some() {
        return Err(EngineError::Scan("a hunt is already running".to_string()));
    }
    if state.scanner.is_some() {
        return Err(EngineError::Scan(
            "this radio is scanning; a hunt needs the dial parked".to_string(),
        ));
    }
    if state.status != DeviceSetStatus::Running {
        return Err(EngineError::Scan(
            "the device set is not running".to_string(),
        ));
    }
    let ranges = &state.capabilities.freq_ranges;
    if !ranges.is_empty()
        && !ranges
            .iter()
            .any(|r| settings.freq_hz >= r.min && settings.freq_hz <= r.max)
    {
        return Err(EngineError::Scan(format!(
            "{} Hz is outside this device's tuning range",
            settings.freq_hz
        )));
    }
    Ok(())
}

pub(crate) fn stop(engine: &Engine, ds: u32) -> Result<HuntStatus, EngineError> {
    let hunt = {
        let mut inner = engine.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let hunt = state
            .hunt
            .take()
            .ok_or_else(|| EngineError::Scan("no hunt is running".to_string()))?;
        inner.revision += 1;
        hunt
    };
    let status = hunt.stop_and_join();
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

pub(crate) fn spawn(
    engine: &Arc<Engine>,
    ds: u32,
    settings: HuntSettings,
) -> Result<HuntState, EngineError> {
    check(&settings)?;
    let status = Arc::new(Mutex::new(HuntStatus {
        settings,
        level_db: None,
        smooth_db: None,
        floor_db: None,
        best_db: None,
        strength: 0.0,
        closing: false,
        readings: 0,
        error: None,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let weak = Arc::downgrade(engine);
        let stop = stop.clone();
        let status = status.clone();
        std::thread::Builder::new()
            .name(format!("sdrmm-hunt-{ds}"))
            .spawn(move || {
                Hunt {
                    engine: weak,
                    ds,
                    settings,
                    stop,
                    status,
                    smooth: None,
                }
                .run();
            })
            .map_err(|e| EngineError::Scan(format!("spawn hunt thread: {e}")))?
    };
    Ok(HuntState {
        stop,
        status,
        thread: Some(thread),
    })
}

struct Hunt {
    engine: Weak<Engine>,
    ds: u32,
    settings: HuntSettings,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<HuntStatus>>,
    smooth: Option<f32>,
}

enum Halt {
    Stopped,
    Failed(String),
}

impl Hunt {
    fn run(mut self) {
        match self.listen() {
            Ok(()) | Err(Halt::Stopped) => {}
            Err(Halt::Failed(error)) => {
                tracing::warn!(ds = self.ds, %error, "hunt stopped");
                lock_status(&self.status).error = Some(error);
                if let Some(engine) = self.engine.upgrade() {
                    self.publish(&engine);
                    engine.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(self.ds),
                    });
                }
            }
        }
    }

    fn listen(&mut self) -> Result<(), Halt> {
        let engine = self.engine.upgrade().ok_or(Halt::Stopped)?;
        let rate = engine.scan_sample_rate(self.ds).ok_or(Halt::Stopped)?;
        let mut rx = engine
            .scan_retune(self.ds, park_at(self.settings.freq_hz, rate))
            .map_err(|e| match e {
                EngineError::DeviceSetNotFound(_) => Halt::Stopped,
                other => Halt::Failed(other.to_string()),
            })?;
        std::thread::sleep(SETTLE);

        let interval = Duration::from_millis(u64::from(self.settings.interval_ms))
            .clamp(MIN_INTERVAL, MAX_INTERVAL);
        let mut heard = Instant::now();
        let mut window_end = Instant::now() + interval;
        let mut peak = f32::NEG_INFINITY;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(Halt::Stopped);
            }
            match rx.try_recv() {
                Ok(snapshot) => {
                    heard = Instant::now();
                    if let Some(db) = crate::scanner::measure(
                        &snapshot,
                        self.settings.freq_hz,
                        self.settings.bw_hz,
                    ) {
                        peak = peak.max(db);
                    }
                }
                Err(TryRecvError::Empty) => {
                    if heard.elapsed() >= SPECTRUM_TIMEOUT {
                        return Err(Halt::Failed(format!(
                            "the device produced no spectrum within {SPECTRUM_TIMEOUT:?}"
                        )));
                    }
                    std::thread::sleep(POLL);
                }
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Closed) => return Err(Halt::Stopped),
            }
            if Instant::now() < window_end {
                continue;
            }
            window_end = Instant::now() + interval;
            if peak.is_finite() {
                self.record(peak);
                self.publish(&engine);
            }
            peak = f32::NEG_INFINITY;
        }
    }

    fn record(&mut self, level_db: f32) {
        let previous = self.smooth;
        let smooth = previous.map_or(level_db, |had| had + (level_db - had) * SMOOTHING);
        self.smooth = Some(smooth);

        let mut status = lock_status(&self.status);
        status.level_db = Some(level_db);
        status.smooth_db = Some(smooth);
        status.floor_db = Some(status.floor_db.map_or(smooth, |had| had.min(smooth)));
        status.best_db = Some(status.best_db.map_or(smooth, |had| had.max(smooth)));
        status.closing = previous.is_some_and(|had| smooth - had >= CLOSING_DB);
        status.strength = strength(smooth, status.floor_db, status.best_db);
        status.readings += 1;
    }

    fn publish(&self, engine: &Engine) {
        engine.emit(ServerEvent::HuntUpdate {
            device_set: self.ds,
            status: Box::new(lock_status(&self.status).clone()),
        });
    }
}

/// Where to park the dial so the hunted frequency is not sitting under the front end's own spike.
fn park_at(freq_hz: f64, sample_rate: f64) -> f64 {
    (freq_hz - sample_rate / 4.0).max(1.0)
}

fn strength(smooth: f32, floor: Option<f32>, best: Option<f32>) -> f32 {
    let (Some(floor), Some(best)) = (floor, best) else {
        return 0.0;
    };
    let range = (best - floor).max(MIN_RANGE_DB);
    ((smooth - floor) / range).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dial_parks_clear_of_the_frequency_being_hunted() {
        let parked = park_at(433_920_000.0, 2_048_000.0);
        assert!(
            (433_920_000.0 - parked) > 100_000.0,
            "the hunted carrier would sit on the receiver's own spike"
        );
        assert!(
            (433_920_000.0 - parked) < 2_048_000.0 / 2.0,
            "the hunted carrier fell outside the passband"
        );
        assert_eq!(park_at(1_000.0, 2_048_000.0), 1.0, "a dial below zero");
    }

    #[test]
    fn a_meter_has_a_range_before_the_ground_has_been_walked() {
        assert_eq!(strength(-70.0, None, None), 0.0);
        assert_eq!(
            strength(-70.0, Some(-70.0), Some(-70.0)),
            0.0,
            "one reading is not a range"
        );
        assert!(strength(-68.0, Some(-70.0), Some(-70.0)) < 0.5);
    }

    #[test]
    fn strength_spans_the_ground_actually_covered() {
        assert_eq!(strength(-90.0, Some(-90.0), Some(-30.0)), 0.0);
        assert_eq!(strength(-30.0, Some(-90.0), Some(-30.0)), 1.0);
        assert!((strength(-60.0, Some(-90.0), Some(-30.0)) - 0.5).abs() < 0.01);
        assert_eq!(
            strength(-100.0, Some(-90.0), Some(-30.0)),
            0.0,
            "a reading below the floor must not run the meter backwards"
        );
    }

    #[test]
    fn hunt_settings_that_cannot_be_measured_are_refused() {
        for bad in [
            HuntSettings {
                freq_hz: 0.0,
                ..HuntSettings::default()
            },
            HuntSettings {
                freq_hz: f64::NAN,
                ..HuntSettings::default()
            },
            HuntSettings {
                freq_hz: 433e6,
                bw_hz: 0.0,
                ..HuntSettings::default()
            },
        ] {
            assert!(check(&bad).is_err(), "accepted {bad:?}");
        }
        assert!(
            check(&HuntSettings {
                freq_hz: 433e6,
                ..HuntSettings::default()
            })
            .is_ok()
        );
    }
}
