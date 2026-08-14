//! Frequency scanner ( P2, M5). App-level, not a channel: a control-plane thread per
//! device set walks a target list, measures each target off the existing spectrum tap, and
//! parks a hosted channel on whatever breaks the threshold.
//!
//! The unit of work is a *device tuning*, not a target: one tuning's passband usually covers
//! hundreds of targets, and they are all measured from the same spectrum frames. A 2 MHz-wide
//! receiver therefore sweeps a whole VHF band per dwell instead of per channel, which is what
//! keeps this affordable on the Pi 4 ( performance floor) — the scan costs retunes and
//! a max over FFT bins, and no extra DSP at all.

use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_wire::{
    MAX_SCAN_TARGETS, ScanSettings, ScanState, ScannerStatus, ServerEvent, StateScope,
};
use tokio::sync::broadcast::error::TryRecvError;

use crate::{Engine, EngineError, runtime::SpectrumSnapshot};

/// Fraction of the sample rate a scan will place targets in. The band edges belong to the
/// device's analog roll-off and the capture filter's transition, so measuring there would
/// read low and miss signals; the same 80% the DDC reserves as its guard band ().
const USABLE_SPAN_FRACTION: f64 = 0.8;
/// Time to let the tuner and the capture ring flush after a retune, before the first frame is
/// believed. The FFT's own 4096-sample history is sub-millisecond at any real rate; this
/// covers the ring backlog, which is what would otherwise be measured at the old frequency.
const RETUNE_SETTLE: Duration = Duration::from_millis(30);
/// Floor on how long a tuning is listened to, whatever `dwell_ms` says — below one spectrum
/// frame (~33 ms at the tap's 30 fps) a dwell would peak-hold a single frame and spend the
/// rest of the sweep retuning.
const MIN_DWELL: Duration = Duration::from_millis(40);
/// How long a listening window overruns waiting for its *first* frame. A dwell shorter than
/// the tap's frame interval, a capture thread still starting, or one the OS starved for a
/// moment must cost latency — never the scan. Only a tap this quiet is a device that stopped.
const SPECTRUM_TIMEOUT: Duration = Duration::from_secs(2);
/// Re-measurement cadence while parked on a hit.
const HOLD_POLL: Duration = Duration::from_millis(120);
/// Spectrum poll interval; the tap produces ~30 frames a second, so this only bounds latency.
const POLL: Duration = Duration::from_millis(4);
/// Minimum spacing between [`ServerEvent::ScannerUpdate`] pushes. State transitions bypass it.
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);

/// Control-plane handle on a running scan: the thread, its stop flag, and the status the
/// device-set projection reads.
pub(crate) struct ScannerState {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<ScannerStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl ScannerState {
    pub(crate) fn status(&self) -> ScannerStatus {
        lock_status(&self.status).clone()
    }

    /// Signal the thread and join it. Must never be called while the engine `inner` lock is
    /// held: the scan thread takes that lock on every retune, so joining under it deadlocks.
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

/// The expanded target list a scan walks: sorted, deduplicated, and bounded.
pub(crate) struct ScanPlan {
    pub(crate) targets: Vec<f64>,
}

impl ScanPlan {
    /// Expand ranges + explicit frequencies into targets, rejecting anything that cannot
    /// produce a finite bounded list. Frequencies are rounded to 1 Hz before deduplication:
    /// two ranges overlapping at a step boundary must not scan the same channel twice.
    pub(crate) fn build(settings: &ScanSettings) -> Result<Self, EngineError> {
        let bad = |msg: String| EngineError::Scan(msg);
        if !settings.threshold_db.is_finite() {
            return Err(bad("threshold_db must be finite".to_string()));
        }
        if !settings.measure_bw_hz.is_finite() || settings.measure_bw_hz <= 0.0 {
            return Err(bad(format!(
                "measure_bw_hz must be positive, got {}",
                settings.measure_bw_hz
            )));
        }
        let mut targets: Vec<f64> = Vec::new();
        for range in &settings.ranges {
            if !range.start_hz.is_finite() || !range.stop_hz.is_finite() {
                return Err(bad("scan range bounds must be finite".to_string()));
            }
            if !range.step_hz.is_finite() || range.step_hz <= 0.0 {
                return Err(bad(format!(
                    "scan range step must be positive, got {}",
                    range.step_hz
                )));
            }
            if range.stop_hz < range.start_hz {
                return Err(bad(format!(
                    "scan range {} Hz–{} Hz ends before it starts",
                    range.start_hz, range.stop_hz
                )));
            }
            // Bounded *before* the cast: `f64 as usize` saturates in release and panics in
            // debug, so checking the count after converting would let a 1 Hz step over a GHz
            // through on one profile and abort on the other.
            let steps = ((range.stop_hz - range.start_hz) / range.step_hz).floor();
            let too_many = !steps.is_finite()
                || steps < 0.0
                || steps >= MAX_SCAN_TARGETS as f64
                || targets.len() + (steps as usize) + 1 > MAX_SCAN_TARGETS;
            if too_many {
                return Err(bad(format!(
                    "scan expands to more than {MAX_SCAN_TARGETS} targets; widen the step or \
                     narrow the range"
                )));
            }
            let count = steps as usize + 1;
            for i in 0..count {
                targets.push(range.start_hz + range.step_hz * i as f64);
            }
        }
        for &freq in &settings.frequencies {
            if !freq.is_finite() || freq <= 0.0 {
                return Err(bad(format!(
                    "scan frequency {freq} is not a usable Hz value"
                )));
            }
            targets.push(freq);
        }
        if targets.len() > MAX_SCAN_TARGETS {
            return Err(bad(format!(
                "scan expands to more than {MAX_SCAN_TARGETS} targets"
            )));
        }
        for t in &mut targets {
            *t = t.round();
        }
        targets.sort_by(f64::total_cmp);
        targets.dedup();
        if targets.is_empty() {
            return Err(bad(
                "a scan needs at least one range or frequency".to_string()
            ));
        }
        Ok(Self { targets })
    }

    /// Group targets into device tunings: each covers as many consecutive targets as fit in
    /// `usable_span`, centered on the group so both ends stay inside the trusted band.
    fn tunings(&self, usable_span: f64) -> Vec<Tuning> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.targets.len() {
            let low = self.targets[i];
            let mut j = i + 1;
            while j < self.targets.len() && self.targets[j] - low <= usable_span {
                j += 1;
            }
            out.push(Tuning {
                center_hz: f64::midpoint(low, self.targets[j - 1]),
                first: i,
                last: j - 1,
            });
            i = j;
        }
        out
    }
}

/// One device tuning and the inclusive target index span it covers.
struct Tuning {
    center_hz: f64,
    first: usize,
    last: usize,
}

/// Spawn the scan thread. The caller has already validated `plan` against the device's tuning
/// range and stored the returned state on the device set.
pub(crate) fn spawn(
    engine: &Arc<Engine>,
    ds: u32,
    plan: ScanPlan,
    settings: ScanSettings,
) -> Result<ScannerState, EngineError> {
    let status = Arc::new(Mutex::new(ScannerStatus {
        state: ScanState::Scanning,
        settings: settings.clone(),
        targets: plan.targets.len() as u32,
        current_hz: plan.targets[0],
        current_db: None,
        sweeps: 0,
        hits: 0,
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
}

/// Why a scan loop stopped.
enum Halt {
    /// Asked to stop, or the engine/device set went away.
    Stopped,
    Failed(String),
}

impl Scan {
    fn run(mut self) {
        match self.sweep_forever() {
            Ok(()) | Err(Halt::Stopped) => {}
            Err(Halt::Failed(error)) => {
                tracing::warn!(ds = self.ds, %error, "scan stopped");
                lock_status(&self.status).error = Some(error);
                // The scan is over but the handle stays until it is stopped or the set goes
                // away; a full state change is what tells clients to read the error.
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
            // A single-tuning scan with nothing on the air would otherwise spin the sweep
            // counter as fast as the dwell allows; the dwell itself is the pacing everywhere
            // else, so nothing extra is needed here.
        }
    }

    /// Retune, dwell, and act on whatever the tuning's targets measured.
    fn visit(&mut self, engine: &Arc<Engine>, tuning: &Tuning) -> Result<(), Halt> {
        let mut rx = engine
            .scan_retune(self.ds, tuning.center_hz)
            .map_err(|e| match e {
                EngineError::DeviceSetNotFound(_) => Halt::Stopped,
                other => Halt::Failed(other.to_string()),
            })?;
        std::thread::sleep(RETUNE_SETTLE);
        drain(&mut rx);

        // Copied out of the plan rather than borrowed: `hold` below needs `&mut self`, and one
        // small Vec per dwell is free next to the dwell itself.
        let targets: Vec<f64> = self.plan.targets[tuning.first..=tuning.last].to_vec();
        let mut peaks = vec![f32::NEG_INFINITY; targets.len()];
        let dwell = Duration::from_millis(u64::from(self.settings.dwell_ms)).max(MIN_DWELL);
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
                lock_status(&self.status).hits += 1;
                self.hold(engine, &mut rx, target, tuning.center_hz)?;
                // A hold costs seconds, so the rest of this tuning's peaks are stale by the
                // time it ends; measure the tuning again instead of acting on them.
                return Ok(());
            }
        }
        Ok(())
    }

    /// Park on an active target until it has been quiet for `resume_ms`.
    fn hold(
        &mut self,
        engine: &Arc<Engine>,
        rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>,
        target: f64,
        center_hz: f64,
    ) -> Result<(), Halt> {
        if let Some(channel) = self.settings.hold_channel
            && let Err(e) = engine.scan_park_channel(self.ds, channel, target - center_hz)
        {
            // A hold channel the user deleted (or retuned out of range) must not kill the
            // scan; drop the listening half and keep logging hits.
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
            // A window that measured nothing must not read as a quiet channel: `listen` waits
            // for a frame, so `resume` only ever counts against real measurements.
            let mut peak = f32::NEG_INFINITY;
            self.listen(
                rx,
                std::slice::from_ref(&target),
                std::slice::from_mut(&mut peak),
                HOLD_POLL,
            )?;
            lock_status(&self.status).current_db = peak.is_finite().then_some(peak);
            self.push_update(engine, false);
            if peak >= self.settings.threshold_db {
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

    /// Peak-hold each of `targets` into the matching slot of `peaks` over one listening window,
    /// which the caller sizes to `targets`.
    ///
    /// The window never closes before it has measured a frame: absence of spectrum is not
    /// absence of signal, so a tap slower than the window — a capture thread still starting,
    /// a momentary stall — costs latency rather than a false reading. Silence beyond
    /// [`SPECTRUM_TIMEOUT`] is a device that stopped, and surfaces as a failed scan.
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
                // The tap drops the oldest frames under a slow consumer; peak-hold over a
                // window tolerates that, so a lag only costs sensitivity, never correctness.
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

    /// Push live progress. Scanner updates are their own event precisely so they can be
    /// frequent; `force` is for state transitions, which must never be throttled away.
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

/// Discard everything already queued — after a retune those frames are the old frequency.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<SpectrumSnapshot>) {
    while !matches!(
        rx.try_recv(),
        Err(TryRecvError::Empty | TryRecvError::Closed)
    ) {}
}

/// Peak power in `bw_hz` around `target`, or `None` when the target is outside this
/// snapshot's span. Peak, not integrated power: the waterfall the operator picks a threshold
/// off is max-decimated (), so a threshold set by eye means the same thing here.
fn measure(snapshot: &SpectrumSnapshot, target: f64, bw_hz: f64) -> Option<f32> {
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
    snapshot.db[lo..=hi]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max)
        .into()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::ScanRange;

    use super::*;

    fn settings(ranges: Vec<ScanRange>, frequencies: Vec<f64>) -> ScanSettings {
        ScanSettings {
            ranges,
            frequencies,
            ..ScanSettings::default()
        }
    }

    #[test]
    fn plan_expands_ranges_inclusively_and_dedups() {
        let plan = ScanPlan::build(&settings(
            vec![
                ScanRange {
                    start_hz: 144_000_000.0,
                    stop_hz: 144_100_000.0,
                    step_hz: 25_000.0,
                },
                // Overlaps the first range's last step: the shared target appears once.
                ScanRange {
                    start_hz: 144_100_000.0,
                    stop_hz: 144_150_000.0,
                    step_hz: 25_000.0,
                },
            ],
            vec![145_500_000.0],
        ))
        .expect("plan");
        assert_eq!(
            plan.targets,
            vec![
                144_000_000.0,
                144_025_000.0,
                144_050_000.0,
                144_075_000.0,
                144_100_000.0,
                144_125_000.0,
                144_150_000.0,
                145_500_000.0,
            ]
        );
    }

    /// A stop that does not land on a step boundary must not invent a target past it.
    #[test]
    fn plan_stops_at_the_last_whole_step() {
        let plan = ScanPlan::build(&settings(
            vec![ScanRange {
                start_hz: 100.0,
                stop_hz: 249.0,
                step_hz: 50.0,
            }],
            Vec::new(),
        ))
        .expect("plan");
        assert_eq!(plan.targets, vec![100.0, 150.0, 200.0]);
    }

    #[test]
    fn plan_rejects_unusable_settings() {
        for bad in [
            settings(Vec::new(), Vec::new()),
            settings(
                vec![ScanRange {
                    start_hz: 100.0,
                    stop_hz: 200.0,
                    step_hz: 0.0,
                }],
                Vec::new(),
            ),
            settings(
                vec![ScanRange {
                    start_hz: 200.0,
                    stop_hz: 100.0,
                    step_hz: 10.0,
                }],
                Vec::new(),
            ),
            settings(Vec::new(), vec![f64::NAN]),
            // 1 Hz steps over 1 GHz: bounded, not attempted.
            settings(
                vec![ScanRange {
                    start_hz: 0.0,
                    stop_hz: 1e9,
                    step_hz: 1.0,
                }],
                Vec::new(),
            ),
        ] {
            assert!(ScanPlan::build(&bad).is_err(), "accepted {bad:?}");
        }
    }

    /// Targets are grouped into as few device tunings as the passband allows, and every
    /// target must sit inside its tuning's usable span — that is the whole efficiency claim.
    #[test]
    fn tunings_cover_every_target_within_the_usable_span() {
        let plan = ScanPlan::build(&settings(
            vec![ScanRange {
                start_hz: 144_000_000.0,
                stop_hz: 146_000_000.0,
                step_hz: 12_500.0,
            }],
            Vec::new(),
        ))
        .expect("plan");
        let usable = 1_000_000.0;
        let tunings = plan.tunings(usable);
        // 161 targets spanning exactly 2 MHz fit in two inclusive 1 MHz windows.
        assert_eq!(
            tunings.len(),
            2,
            "greedy grouping must not split needlessly"
        );
        let mut covered = 0;
        for tuning in &tunings {
            for &target in &plan.targets[tuning.first..=tuning.last] {
                assert!(
                    (target - tuning.center_hz).abs() <= usable / 2.0,
                    "target {target} outside tuning at {}",
                    tuning.center_hz
                );
                covered += 1;
            }
        }
        assert_eq!(covered, plan.targets.len(), "every target scanned once");
    }

    fn snapshot(center_hz: f64, span_hz: f32, db: Vec<f32>) -> SpectrumSnapshot {
        SpectrumSnapshot {
            seq: 1,
            timestamp: 0,
            center_hz,
            span_hz,
            db: Arc::from(db.as_slice()),
        }
    }

    /// The measurement window must find a peak that sits off center, and must report nothing
    /// for a target the tuning does not cover (rather than a bogus edge reading).
    #[test]
    fn measure_finds_the_peak_and_refuses_out_of_span_targets() {
        let mut db = vec![-90.0f32; 1024];
        // Bin 640 == center + 0.125 * span; at 1 MHz span that is +125 kHz.
        db[640] = -20.0;
        let snap = snapshot(100_000_000.0, 1_000_000.0, db);

        assert_eq!(measure(&snap, 100_125_000.0, 10_000.0), Some(-20.0));
        assert_eq!(measure(&snap, 100_300_000.0, 10_000.0), Some(-90.0));
        assert_eq!(measure(&snap, 101_000_000.0, 10_000.0), None);
        assert_eq!(measure(&snap, 99_000_000.0, 10_000.0), None);

        // A window wide enough to reach the peak from a neighbouring channel finds it: the
        // measurement is over the requested bandwidth, not a single bin.
        assert_eq!(measure(&snap, 100_100_000.0, 60_000.0), Some(-20.0));
    }

    /// A scan stripped to what [`Scan::listen`] actually reads: its settings and its stop flag.
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
                current_hz: 100_000_000.0,
                current_db: None,
                sweeps: 0,
                hits: 0,
                error: None,
            })),
            settings,
            stop: Arc::new(AtomicBool::new(false)),
            last_update: None,
        }
    }

    /// Bin 640 of 1024 == center + 0.125 * span; at 1 MHz span that peak sits at 100.125 MHz.
    fn carrier_at_125_khz() -> SpectrumSnapshot {
        let mut db = vec![-90.0f32; 1024];
        db[640] = -20.0;
        snapshot(100_000_000.0, 1_000_000.0, db)
    }

    /// The regression: a tap that has not produced its first frame by the time the dwell
    /// expires — a capture thread still starting, a runner that starved it — must extend the
    /// window and measure, not fail the scan.
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

    /// The bound on that patience: a device that stopped producing has to surface as a scan
    /// error rather than a scan that waits on it forever.
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

    /// Waiting for a frame must not outlast a stop: the whole point of the timeout is that a
    /// dead device is survivable, and a client that stops the scan cannot be made to wait it out.
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
