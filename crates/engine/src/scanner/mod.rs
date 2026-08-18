use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_device::SweepPlan;
use sdrmm_wire::{ScanMode, ScanSettings, ScanState, ScannerStatus, ServerEvent, StateScope};
use tokio::sync::broadcast::error::TryRecvError;

use crate::{Engine, EngineError, runtime::SpectrumSnapshot};

mod close_call;
mod plan;
pub(crate) mod session;
mod sweep;

use close_call::CloseCall;
use plan::Tuning;
pub(crate) use plan::{ScanPlan, partition};

const USABLE_SPAN_FRACTION: f64 = 0.8;
const RETUNE_SETTLE: Duration = Duration::from_millis(30);
const MIN_DWELL: Duration = Duration::from_millis(40);
const SPECTRUM_TIMEOUT: Duration = Duration::from_secs(2);
const HOLD_POLL: Duration = Duration::from_millis(120);
const POLL: Duration = Duration::from_millis(4);
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);

pub(crate) struct ScannerState {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<ScannerStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl ScannerState {
    pub(crate) fn status(&self) -> ScannerStatus {
        lock_status(&self.status).clone()
    }

    pub(crate) fn stop_and_join(mut self) -> ScannerStatus {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("scanner thread panicked");
        }
        self.status()
    }
}

fn lock_status(status: &Mutex<ScannerStatus>) -> std::sync::MutexGuard<'_, ScannerStatus> {
    status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn spawn(
    engine: &Arc<Engine>,
    ds: u32,
    plan: ScanPlan,
    settings: ScanSettings,
) -> Result<ScannerState, EngineError> {
    let hardware = settings.hardware_sweep && engine.sweeps_in_firmware(ds);
    let status = Arc::new(Mutex::new(ScannerStatus {
        state: ScanState::Scanning,
        settings: settings.clone(),
        targets: plan.targets.len() as u32,
        first_hz: plan.targets[0],
        last_hz: *plan.targets.last().unwrap_or(&plan.targets[0]),
        current_hz: plan.targets[0],
        current_db: None,
        sweeps: 0,
        hits: 0,
        hardware_sweep: hardware,
        error: None,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let weak = Arc::downgrade(engine);
        let stop = stop.clone();
        let status = status.clone();
        std::thread::Builder::new()
            .name(format!("sdrmm-scan-{ds}"))
            .spawn(move || {
                let scan = Scan {
                    engine: weak,
                    ds,
                    plan,
                    settings,
                    stop,
                    status,
                    last_update: None,
                    hardware,
                    in_sweep: false,
                    close_call: CloseCall::default(),
                };
                scan.run();
            })
            .map_err(|e| EngineError::Scan(format!("spawn scanner thread: {e}")))?
    };
    Ok(ScannerState {
        stop,
        status,
        thread: Some(thread),
    })
}

struct Scan {
    engine: Weak<Engine>,
    ds: u32,
    plan: ScanPlan,
    settings: ScanSettings,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<ScannerStatus>>,
    last_update: Option<Instant>,
    hardware: bool,
    in_sweep: bool,
    close_call: CloseCall,
}

enum Halt {
    Stopped,
    Failed(String),
}

/// A frequency the scan decided to stop on, and the level it has to keep to stay there.
#[derive(Clone, Copy, Debug)]
struct Call {
    hz: f64,
    db: f32,
    keep_db: f32,
}

impl Scan {
    fn run(mut self) {
        let outcome = self.sweep_forever();
        if self.in_sweep
            && let Some(engine) = self.engine.upgrade()
            && let Err(e) = sweep::leave(&engine, self.ds)
            && !matches!(e, EngineError::DeviceSetNotFound(_))
        {
            tracing::error!(ds = self.ds, error = %e, "could not put the receive stream back");
        }
        match outcome {
            Ok(()) | Err(Halt::Stopped) => {}
            Err(Halt::Failed(error)) => {
                tracing::warn!(ds = self.ds, %error, "scan stopped");
                lock_status(&self.status).error = Some(error);
                if let Some(engine) = self.engine.upgrade() {
                    self.push_update(&engine, true);
                    engine.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(self.ds),
                    });
                }
            }
        }
    }

    fn sweep_forever(&mut self) -> Result<(), Halt> {
        loop {
            let engine = self.engine.upgrade().ok_or(Halt::Stopped)?;
            let rate = engine.scan_sample_rate(self.ds).ok_or(Halt::Stopped)?;
            if self.hardware {
                self.firmware_pass(&engine, rate)?;
                continue;
            }
            let usable = rate * USABLE_SPAN_FRACTION - self.settings.measure_bw_hz;
            if usable <= 0.0 {
                return Err(Halt::Failed(format!(
                    "a {} Hz measurement bandwidth does not fit in a {rate} Hz device passband",
                    self.settings.measure_bw_hz
                )));
            }
            let tunings = self.plan.tunings(usable);
            for tuning in &tunings {
                self.check_stop()?;
                self.visit(&engine, tuning)?;
            }
            lock_status(&self.status).sweeps += 1;
        }
    }

    /// One pass of the radio's own sweep: the blocks arrive stamped with the frequency each was
    /// taken at, so nothing here retunes and every target is read as its block goes past.
    fn firmware_pass(&mut self, engine: &Arc<Engine>, rate: f64) -> Result<(), Halt> {
        if !self.in_sweep {
            let plan = SweepPlan::new(
                sweep::bands(&self.plan.targets, rate, self.settings.measure_bw_hz),
                rate,
            );
            if let Err(error) = sweep::enter(engine, self.ds, &plan) {
                tracing::warn!(ds = self.ds, %error, "no firmware sweep; retuning instead");
                self.hardware = false;
                lock_status(&self.status).hardware_sweep = false;
                self.push_update(engine, true);
                return Ok(());
            }
            self.in_sweep = true;
        }
        let mut rx = engine
            .subscribe_spectrum(self.ds, 0)
            .map_err(|e| Halt::Failed(e.to_string()))?;
        let mut heard = Instant::now();
        let mut first_center = None;
        loop {
            self.check_stop()?;
            match rx.try_recv() {
                Ok(snapshot) => {
                    heard = Instant::now();
                    match first_center {
                        None => first_center = Some(snapshot.center_hz),
                        Some(start) if (snapshot.center_hz - start).abs() < 1.0 => {
                            lock_status(&self.status).sweeps += 1;
                        }
                        Some(_) => {}
                    }
                    if let Some(call) = self.read_block(engine, &snapshot) {
                        self.hit(engine, call, snapshot.center_hz)?;
                        return Ok(());
                    }
                }
                Err(TryRecvError::Empty) => {
                    if heard.elapsed() >= SPECTRUM_TIMEOUT {
                        return Err(Halt::Failed(format!(
                            "the firmware sweep went quiet for {SPECTRUM_TIMEOUT:?}"
                        )));
                    }
                    std::thread::sleep(POLL);
                }
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Closed) => return Err(Halt::Stopped),
            }
        }
    }

    /// Reads every target this block covers, answering with the first one over the threshold.
    ///
    /// A block spans one tuning out of a plan that may hold thousands of targets, so the sorted
    /// list is cut down to that span before anything is measured.
    fn read_block(&mut self, engine: &Arc<Engine>, snapshot: &SpectrumSnapshot) -> Option<Call> {
        if self.settings.mode == ScanMode::CloseCall {
            let call = self.call_in(snapshot)?;
            self.note_hit(call);
            return Some(call);
        }
        let mut seen = None;
        for &target in covered(&self.plan.targets, snapshot, self.settings.measure_bw_hz) {
            let Some(db) = measure(snapshot, target, self.settings.measure_bw_hz) else {
                continue;
            };
            seen = Some((target, db));
            if db >= self.settings.threshold_db {
                let call = Call {
                    hz: target,
                    db,
                    keep_db: self.settings.threshold_db,
                };
                self.note_hit(call);
                return Some(call);
            }
        }
        if let Some((target, db)) = seen {
            let mut status = lock_status(&self.status);
            status.current_hz = target;
            status.current_db = Some(db);
            drop(status);
            self.push_update(engine, false);
        }
        None
    }

    /// The loudest carrier in a block, and the level it has to keep to stay held.
    fn call_in(&mut self, snapshot: &SpectrumSnapshot) -> Option<Call> {
        let margin = self.settings.margin_db;
        let peak = self.close_call.strongest(snapshot, margin)?;
        Some(Call {
            hz: peak.hz,
            db: peak.db,
            keep_db: peak.floor_db + margin,
        })
    }

    fn note_hit(&self, call: Call) {
        let mut status = lock_status(&self.status);
        status.current_hz = call.hz;
        status.current_db = Some(call.db);
        status.hits += 1;
    }

    /// Puts the receive stream back, holds on what the sweep found, and returns to sweeping.
    fn hit(&mut self, engine: &Arc<Engine>, call: Call, center_hz: f64) -> Result<(), Halt> {
        sweep::leave(engine, self.ds).map_err(|e| Halt::Failed(e.to_string()))?;
        self.in_sweep = false;
        let mut rx = engine
            .scan_retune(self.ds, center_hz)
            .map_err(|e| match e {
                EngineError::DeviceSetNotFound(_) => Halt::Stopped,
                other => Halt::Failed(other.to_string()),
            })?;
        std::thread::sleep(RETUNE_SETTLE);
        drain(&mut rx);
        self.hold(engine, &mut rx, call, center_hz)
    }

    fn visit(&mut self, engine: &Arc<Engine>, tuning: &Tuning) -> Result<(), Halt> {
        let mut rx = engine
            .scan_retune(self.ds, tuning.center_hz)
            .map_err(|e| match e {
                EngineError::DeviceSetNotFound(_) => Halt::Stopped,
                other => Halt::Failed(other.to_string()),
            })?;
        std::thread::sleep(RETUNE_SETTLE);
        drain(&mut rx);

        let dwell = Duration::from_millis(u64::from(self.settings.dwell_ms)).max(MIN_DWELL);
        if self.settings.mode == ScanMode::CloseCall {
            return self.watch(engine, &mut rx, tuning.center_hz, dwell);
        }
        let targets: Vec<f64> = self.plan.targets[tuning.first..=tuning.last].to_vec();
        let mut peaks = vec![f32::NEG_INFINITY; targets.len()];
        self.listen(&mut rx, &targets, &mut peaks, dwell)?;

        for (&target, &level) in targets.iter().zip(&peaks) {
            self.check_stop()?;
            {
                let mut status = lock_status(&self.status);
                status.current_hz = target;
                status.current_db = level.is_finite().then_some(level);
            }
            self.push_update(engine, false);
            if level >= self.settings.threshold_db {
                let call = Call {
                    hz: target,
                    db: level,
                    keep_db: self.settings.threshold_db,
                };
                lock_status(&self.status).hits += 1;
                self.hold(engine, &mut rx, call, tuning.center_hz)?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Watches one tuning for the loudest thing standing clear of the noise, rather than reading
    /// frequencies someone had to name in advance.
    fn watch(
        &mut self,
        engine: &Arc<Engine>,
        rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>,
        center_hz: f64,
        dwell: Duration,
    ) -> Result<(), Halt> {
        let deadline = Instant::now() + dwell;
        let mut heard = Instant::now();
        loop {
            self.check_stop()?;
            match rx.try_recv() {
                Ok(snapshot) => {
                    heard = Instant::now();
                    if let Some(call) = self.call_in(&snapshot) {
                        self.note_hit(call);
                        return self.hold(engine, rx, call, center_hz);
                    }
                    lock_status(&self.status).current_hz = center_hz;
                    self.push_update(engine, false);
                    if Instant::now() >= deadline {
                        return Ok(());
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
        }
    }

    fn hold(
        &mut self,
        engine: &Arc<Engine>,
        rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>,
        call: Call,
        center_hz: f64,
    ) -> Result<(), Halt> {
        let target = call.hz;
        if let Some(channel) = self.settings.hold_channel
            && let Err(e) = engine.scan_park_channel(self.ds, channel, target - center_hz)
        {
            tracing::warn!(ds = self.ds, channel, error = %e, "scan hold channel unusable");
            self.settings.hold_channel = None;
            lock_status(&self.status).settings.hold_channel = None;
        }
        {
            let mut status = lock_status(&self.status);
            status.state = ScanState::Holding;
            status.current_hz = target;
        }
        self.push_update(engine, true);

        let resume = Duration::from_millis(u64::from(self.settings.resume_ms));
        let mut quiet_since: Option<Instant> = None;
        loop {
            self.check_stop()?;
            let mut peak = f32::NEG_INFINITY;
            self.listen(
                rx,
                std::slice::from_ref(&target),
                std::slice::from_mut(&mut peak),
                HOLD_POLL,
            )?;
            lock_status(&self.status).current_db = peak.is_finite().then_some(peak);
            self.push_update(engine, false);
            if peak >= call.keep_db {
                quiet_since = None;
            } else {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= resume {
                    break;
                }
            }
        }
        lock_status(&self.status).state = ScanState::Scanning;
        self.push_update(engine, true);
        Ok(())
    }

    fn listen(
        &self,
        rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>,
        targets: &[f64],
        peaks: &mut [f32],
        window: Duration,
    ) -> Result<(), Halt> {
        peaks.fill(f32::NEG_INFINITY);
        let start = Instant::now();
        let deadline = start + window;
        let mut frames = 0usize;
        loop {
            self.check_stop()?;
            let now = Instant::now();
            if frames > 0 {
                if now >= deadline {
                    return Ok(());
                }
            } else if now.duration_since(start) >= SPECTRUM_TIMEOUT {
                return Err(Halt::Failed(format!(
                    "the device produced no spectrum within {SPECTRUM_TIMEOUT:?}"
                )));
            }
            match rx.try_recv() {
                Ok(snapshot) => {
                    frames += 1;
                    for (peak, &target) in peaks.iter_mut().zip(targets) {
                        if let Some(db) = measure(&snapshot, target, self.settings.measure_bw_hz) {
                            *peak = peak.max(db);
                        }
                    }
                }
                Err(TryRecvError::Empty) => std::thread::sleep(POLL),
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Closed) => return Err(Halt::Stopped),
            }
        }
    }

    fn check_stop(&self) -> Result<(), Halt> {
        if self.stop.load(Ordering::Acquire) {
            return Err(Halt::Stopped);
        }
        Ok(())
    }

    fn push_update(&mut self, engine: &Engine, force: bool) {
        let now = Instant::now();
        if !force && self.last_update.is_some_and(|t| now - t < UPDATE_INTERVAL) {
            return;
        }
        self.last_update = Some(now);
        engine.emit(ServerEvent::ScannerUpdate {
            device_set: self.ds,
            status: Box::new(lock_status(&self.status).clone()),
        });
    }
}

/// The stretch of a sorted target list that a block could hold a reading for.
fn covered<'a>(targets: &'a [f64], snapshot: &SpectrumSnapshot, bw_hz: f64) -> &'a [f64] {
    let half = f64::from(snapshot.span_hz) / 2.0 + bw_hz / 2.0;
    let low = snapshot.center_hz - half;
    let high = snapshot.center_hz + half;
    let first = targets.partition_point(|&hz| hz < low);
    let last = targets.partition_point(|&hz| hz <= high);
    &targets[first..last]
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>) {
    while !matches!(
        rx.try_recv(),
        Err(TryRecvError::Empty | TryRecvError::Closed)
    ) {}
}

pub(crate) fn measure(snapshot: &SpectrumSnapshot, target: f64, bw_hz: f64) -> Option<f32> {
    let n = snapshot.db.len();
    if n == 0 || snapshot.span_hz <= 0.0 {
        return None;
    }
    let span = f64::from(snapshot.span_hz);
    let bin = |hz: f64| (hz - snapshot.center_hz) / span * n as f64 + n as f64 / 2.0;
    let lo = bin(target - bw_hz / 2.0).floor();
    let hi = bin(target + bw_hz / 2.0).ceil();
    if hi < 0.0 || lo > (n - 1) as f64 {
        return None;
    }
    let lo = (lo.max(0.0) as usize).min(n - 1);
    let hi = (hi.max(0.0) as usize).min(n - 1);
    let guard = snapshot.lo_guard();
    let peak = (lo..=hi)
        .filter(|i| !guard.as_ref().is_some_and(|g| g.contains(i)))
        .map(|i| snapshot.db[i])
        .fold(f32::NEG_INFINITY, f32::max);
    peak.is_finite().then_some(peak)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(center_hz: f64, span_hz: f32, db: Vec<f32>) -> SpectrumSnapshot {
        SpectrumSnapshot {
            seq: 1,
            timestamp: 0,
            center_hz,
            span_hz,
            lo_hz: center_hz,
            db: Arc::from(db.as_slice()),
        }
    }

    #[test]
    fn measure_finds_the_peak_and_refuses_out_of_span_targets() {
        let mut db = vec![-90.0f32; 1024];
        db[640] = -20.0;
        let snap = snapshot(100_000_000.0, 1_000_000.0, db);

        assert_eq!(measure(&snap, 100_125_000.0, 10_000.0), Some(-20.0));
        assert_eq!(measure(&snap, 100_300_000.0, 10_000.0), Some(-90.0));
        assert_eq!(measure(&snap, 101_000_000.0, 10_000.0), None);
        assert_eq!(measure(&snap, 99_000_000.0, 10_000.0), None);

        assert_eq!(measure(&snap, 100_100_000.0, 60_000.0), Some(-20.0));
    }

    #[test]
    fn a_spike_at_the_lo_is_not_mistaken_for_a_target() {
        let mut db = vec![-90.0f32; 1024];
        db[512] = -20.0;
        let snap = snapshot(100_000_000.0, 1_000_000.0, db);

        assert_eq!(
            measure(&snap, 100_000_000.0, 10_000.0),
            Some(-90.0),
            "the front end's own spike read as a signal on the tuned frequency"
        );
    }

    #[test]
    fn moving_the_lo_off_centre_frees_the_frequency_under_it() {
        let mut db = vec![-90.0f32; 1024];
        db[512] = -20.0;
        let mut snap = snapshot(100_000_000.0, 1_000_000.0, db);
        snap.lo_hz = 100_000_000.0 - 250_000.0;

        assert_eq!(
            measure(&snap, 100_000_000.0, 10_000.0),
            Some(-20.0),
            "a real carrier was thrown away with the LO guard"
        );
    }

    fn listener() -> Scan {
        let settings = ScanSettings::default();
        Scan {
            engine: Weak::new(),
            ds: 0,
            plan: ScanPlan {
                targets: vec![100_000_000.0],
            },
            status: Arc::new(Mutex::new(ScannerStatus {
                state: ScanState::Scanning,
                settings: settings.clone(),
                targets: 1,
                first_hz: 100_000_000.0,
                last_hz: 100_000_000.0,
                current_hz: 100_000_000.0,
                current_db: None,
                sweeps: 0,
                hits: 0,
                hardware_sweep: false,
                error: None,
            })),
            settings,
            stop: Arc::new(AtomicBool::new(false)),
            last_update: None,
            hardware: false,
            in_sweep: false,
            close_call: CloseCall::default(),
        }
    }

    fn carrier_at_125_khz() -> SpectrumSnapshot {
        let mut db = vec![-90.0f32; 1024];
        db[640] = -20.0;
        snapshot(100_000_000.0, 1_000_000.0, db)
    }

    #[test]
    fn a_window_waits_past_its_deadline_for_the_first_frame() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let feeder = tx.clone();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let _ = feeder.send(carrier_at_125_khz());
        });

        let mut peak = f32::NEG_INFINITY;
        let listened = listener().listen(
            &mut rx,
            &[100_125_000.0],
            std::slice::from_mut(&mut peak),
            Duration::from_millis(20),
        );
        assert!(
            matches!(listened, Ok(())),
            "a late first frame must not fail the scan"
        );
        assert_eq!(peak, -20.0, "the late frame must still be measured");
        sender.join().expect("feeder");
    }

    #[test]
    fn a_silent_tap_fails_the_scan_once_the_timeout_passes() {
        let (_tx, mut rx) = tokio::sync::broadcast::channel::<SpectrumSnapshot>(8);
        let started = Instant::now();
        let mut peak = f32::NEG_INFINITY;
        let listened = listener().listen(
            &mut rx,
            &[100_000_000.0],
            std::slice::from_mut(&mut peak),
            Duration::from_millis(20),
        );
        let Err(Halt::Failed(error)) = listened else {
            panic!("a silent tap must fail the scan");
        };
        assert!(error.contains("no spectrum"), "unhelpful error: {error}");
        assert!(started.elapsed() >= SPECTRUM_TIMEOUT, "gave up early");
    }

    #[test]
    fn a_stop_beats_the_wait_for_a_frame() {
        let (_tx, mut rx) = tokio::sync::broadcast::channel::<SpectrumSnapshot>(8);
        let scan = listener();
        scan.stop.store(true, Ordering::Release);
        let started = Instant::now();
        let mut peak = f32::NEG_INFINITY;
        let listened = scan.listen(
            &mut rx,
            &[100_000_000.0],
            std::slice::from_mut(&mut peak),
            Duration::from_millis(20),
        );
        assert!(matches!(listened, Err(Halt::Stopped)));
        assert!(
            started.elapsed() < SPECTRUM_TIMEOUT,
            "waited out the timeout"
        );
    }
}
