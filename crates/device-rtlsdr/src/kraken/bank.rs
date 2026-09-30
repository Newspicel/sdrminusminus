use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle, Scope, ScopedJoinHandle, Thread},
    time::{Duration, Instant},
};

use sdrmm_device::{
    CaptureConfig, CaptureStream, DeviceError, GapScope, Latency, MarkPoster, Recovery, RxSink,
    SampleConverter, StreamFailure, UNKNOWN_ERROR, Uncertainty, drain_stream, lock, schedule,
};

const SUPERVISOR: &str = "sdrmm-kraken-bank";
const DRAIN: &str = "sdrmm-kraken-rx";
const WATCH: Duration = Duration::from_millis(50);

type Held<L> = Vec<(<L as BankLane>::Stream, <L as BankLane>::Gate)>;

pub(crate) trait BankLane: Send + 'static {
    type Stream: CaptureStream;
    type Gate: Send + 'static;

    fn hold(&mut self) -> Result<(Self::Stream, Self::Gate), DeviceError>;
    fn release(gate: &Self::Gate) -> Result<(), DeviceError>;
    fn rehold(gate: &Self::Gate) -> Result<(), DeviceError>;
}

struct LaneSwitch {
    running: AtomicBool,
    faulted: AtomicBool,
}

impl LaneSwitch {
    fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
        }
    }

    fn arm(&self) {
        self.faulted.store(false, Ordering::Release);
        self.running.store(true, Ordering::Release);
    }

    fn halt(&self) {
        self.running.store(false, Ordering::Release);
    }

    fn injected(&self, lane: usize) -> Option<StreamFailure> {
        self.faulted.load(Ordering::Acquire).then(|| StreamFailure {
            reason: format!("lane {lane} was failed on purpose"),
            gone: false,
        })
    }
}

struct Shared {
    running: AtomicBool,
    failed: AtomicBool,
    rate: AtomicU64,
    lanes: Vec<LaneSwitch>,
}

impl Shared {
    fn live(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn rate(&self) -> f64 {
        f64::from_bits(self.rate.load(Ordering::Acquire))
    }

    fn halt_lanes(&self) {
        for lane in &self.lanes {
            lane.halt();
        }
    }
}

pub(crate) struct Bank {
    shared: Arc<Shared>,
    supervisor: Option<JoinHandle<()>>,
    posters: Vec<MarkPoster>,
}

impl std::fmt::Debug for Bank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bank")
            .field("lanes", &self.posters.len())
            .field("running", &self.is_running())
            .finish()
    }
}

struct Job<C> {
    config: CaptureConfig,
    make_converter: fn() -> C,
    rearmed: Option<Instant>,
}

struct Alarm<'a> {
    shared: &'a Shared,
    supervisor: &'a Thread,
    raised: bool,
}

impl Drop for Alarm<'_> {
    fn drop(&mut self) {
        if self.raised {
            self.shared.failed.store(true, Ordering::Release);
            self.supervisor.unpark();
        }
    }
}

enum Outcome {
    Stopped,
    Refused(DeviceError),
    Failed(StreamFailure),
}

impl Bank {
    pub(crate) fn start<L: BankLane, C: SampleConverter>(
        lanes: Vec<Arc<Mutex<L>>>,
        sinks: Vec<RxSink>,
        config: CaptureConfig,
        make_converter: fn() -> C,
    ) -> Result<Self, DeviceError> {
        if sinks.len() != lanes.len() {
            return Err(DeviceError::Unsupported(format!(
                "this radio has {} rx streams, got {} sinks",
                lanes.len(),
                sinks.len()
            )));
        }
        let held = hold_all(&lanes)?;
        let posters = sinks.iter().map(RxSink::mark_poster).collect();
        let shared = Arc::new(Shared {
            running: AtomicBool::new(true),
            failed: AtomicBool::new(false),
            rate: AtomicU64::new(config.sample_rate.to_bits()),
            lanes: lanes.iter().map(|_| LaneSwitch::new()).collect(),
        });
        let supervised = shared.clone();
        let supervisor = thread::Builder::new()
            .name(SUPERVISOR.to_owned())
            .spawn(move || supervise(&lanes, sinks, held, &supervised, config, make_converter))
            .map_err(|error| DeviceError::Io(format!("spawn {SUPERVISOR}: {error}")))?;
        Ok(Self {
            shared,
            supervisor: Some(supervisor),
            posters,
        })
    }

    pub(crate) fn posters(&self) -> &[MarkPoster] {
        &self.posters
    }

    pub(crate) fn retime(&self, rate: f64) {
        if rate.is_finite() && rate > 0.0 {
            self.shared.rate.store(rate.to_bits(), Ordering::Release);
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|supervisor| !supervisor.is_finished())
    }

    pub(crate) fn stop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        self.shared.halt_lanes();
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.thread().unpark();
            if supervisor.join().is_err() {
                tracing::error!("the bank supervisor panicked");
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_lane_for_test(&self, lane: usize) {
        if let Some(switch) = self.shared.lanes.get(lane) {
            switch.faulted.store(true, Ordering::Release);
            switch.halt();
        }
    }
}

impl Drop for Bank {
    fn drop(&mut self) {
        self.stop();
    }
}

fn hold_all<L: BankLane>(lanes: &[Arc<Mutex<L>>]) -> Result<Held<L>, DeviceError> {
    lanes.iter().map(|lane| lock(lane).hold()).collect()
}

fn fail_all(sinks: &mut [RxSink], error: &DeviceError) {
    for sink in sinks {
        sink.fail(error.clone());
    }
}

fn supervise<L: BankLane, C: SampleConverter>(
    lanes: &[Arc<Mutex<L>>],
    mut sinks: Vec<RxSink>,
    mut held: Held<L>,
    shared: &Shared,
    config: CaptureConfig,
    make_converter: fn() -> C,
) {
    let mut policy = config.restart;
    let mut rearmed = None;
    loop {
        let started = Instant::now();
        let job = Job {
            config,
            make_converter,
            rearmed,
        };
        let failure = match run_pass::<L, C>(held, &mut sinks, shared, &job) {
            Outcome::Stopped => return,
            Outcome::Refused(error) => return fail_all(&mut sinks, &error),
            Outcome::Failed(failure) => failure,
        };
        let failed_at = Instant::now();
        if failure.gone {
            return fail_all(&mut sinks, &DeviceError::Disconnected(failure.reason));
        }
        let Recovery::RetryAfter { attempt, delay } = policy.on_failure(started.elapsed()) else {
            let error = DeviceError::Io(format!(
                "device lost after {} restart attempts: {}",
                policy.attempts() - 1,
                failure.reason
            ));
            return fail_all(&mut sinks, &error);
        };
        tracing::warn!(attempt, ?delay, reason = %failure.reason, "a bank lane failed; restarting the bank in place");
        if !rest(shared, delay) {
            return;
        }
        held = match hold_all(lanes) {
            Ok(fresh) => fresh,
            Err(DeviceError::Disconnected(reason)) => {
                return fail_all(&mut sinks, &DeviceError::Disconnected(reason));
            }
            Err(error) => {
                let error = DeviceError::Io(format!("bank restart failed: {error}"));
                return fail_all(&mut sinks, &error);
            }
        };
        rearmed = Some(failed_at);
        tracing::info!(attempt, "bank restarted");
    }
}

fn rest(shared: &Shared, delay: Duration) -> bool {
    let until = Instant::now() + delay;
    while shared.live() {
        let now = Instant::now();
        if now >= until {
            return true;
        }
        thread::park_timeout(until - now);
    }
    false
}

fn lost_since(since: Instant, rate: f64) -> u64 {
    (since.elapsed().as_secs_f64() * rate).ceil() as u64
}

const fn estimate_error(lost: u64) -> u64 {
    if lost == 0 { UNKNOWN_ERROR } else { lost }
}

fn run_pass<L: BankLane, C: SampleConverter>(
    held: Held<L>,
    sinks: &mut [RxSink],
    shared: &Shared,
    job: &Job<C>,
) -> Outcome {
    if !shared.live() {
        return Outcome::Stopped;
    }
    shared.failed.store(false, Ordering::Release);
    for lane in &shared.lanes {
        lane.arm();
    }
    let supervisor = thread::current();
    let work: Vec<_> = held.into_iter().zip(sinks.iter_mut()).collect();
    thread::scope(|scope| {
        let mut gates = Vec::with_capacity(work.len());
        let mut drains = Vec::with_capacity(work.len());
        for (lane, ((stream, gate), sink)) in work.into_iter().enumerate() {
            gates.push(gate);
            match spawn_drain(scope, lane, stream, sink, shared, &supervisor, job) {
                Ok(drain) => drains.push(drain),
                Err(error) => {
                    shared.halt_lanes();
                    return Outcome::Refused(error);
                }
            }
        }
        if let Err(error) = release_all::<L>(&gates) {
            shared.halt_lanes();
            return Outcome::Refused(error);
        }
        watch(shared);
        shared.halt_lanes();
        settle(drains, shared)
    })
}

fn spawn_drain<'scope, 'env, S: CaptureStream, C: SampleConverter>(
    scope: &'scope Scope<'scope, 'env>,
    lane: usize,
    stream: S,
    sink: &'scope mut RxSink,
    shared: &'scope Shared,
    supervisor: &'scope Thread,
    job: &'scope Job<C>,
) -> Result<ScopedJoinHandle<'scope, Option<StreamFailure>>, DeviceError> {
    let rate = shared.rate();
    let rearmed = job.rearmed.map(|since| lost_since(since, rate));
    let config = job.config.with_sample_rate(Some(rate));
    thread::Builder::new()
        .name(DRAIN.to_owned())
        .spawn_scoped(scope, move || {
            let mut alarm = Alarm {
                shared,
                supervisor,
                raised: true,
            };
            schedule::claim(Latency::Critical);
            let switch = &shared.lanes[lane];
            if let Some(lost) = rearmed {
                sink.dropped_estimate(lost, estimate_error(lost), GapScope::Lane);
                sink.realigned(Uncertainty::Rearmed, UNKNOWN_ERROR, GapScope::Lane);
            }
            let mut converter = (job.make_converter)();
            let failure = drain_stream(&stream, &switch.running, sink, &mut converter, &config)
                .or_else(|| switch.injected(lane));
            drop(stream);
            alarm.raised = failure.is_some();
            failure
        })
        .map_err(|error| DeviceError::Io(format!("spawn {DRAIN}: {error}")))
}

fn release_all<L: BankLane>(gates: &[L::Gate]) -> Result<(), DeviceError> {
    for (index, gate) in gates.iter().enumerate() {
        if let Err(error) = L::release(gate) {
            for released in &gates[..index] {
                if let Err(rehold) = L::rehold(released) {
                    tracing::warn!(%rehold, "a released lane could not be held again");
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

fn watch(shared: &Shared) {
    while shared.live() && !shared.failed.load(Ordering::Acquire) {
        thread::park_timeout(WATCH);
    }
}

fn settle(drains: Vec<ScopedJoinHandle<'_, Option<StreamFailure>>>, shared: &Shared) -> Outcome {
    let failures: Vec<StreamFailure> = drains
        .into_iter()
        .filter_map(|drain| {
            drain.join().unwrap_or_else(|_| {
                Some(StreamFailure {
                    reason: "a lane thread panicked".to_owned(),
                    gone: false,
                })
            })
        })
        .collect();
    if !shared.live() {
        return Outcome::Stopped;
    }
    let first = failures
        .iter()
        .find(|failure| failure.gone)
        .or_else(|| failures.first())
        .cloned()
        .unwrap_or_else(|| StreamFailure {
            reason: "a lane stopped without a reason".to_owned(),
            gone: false,
        });
    Outcome::Failed(first)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{sync::atomic::AtomicUsize, thread::ThreadId};

    use sdrmm_device::{LaneEvent, Next, SinkItem, StopHandle};

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Seen {
        Samples { index: u64, len: usize },
        Event(LaneEvent),
    }

    #[derive(Clone, Debug)]
    struct Release {
        lane: usize,
        thread: ThreadId,
        name: Option<String>,
        at: Instant,
    }

    #[derive(Clone, Default)]
    struct NoStop;

    impl StopHandle for NoStop {
        fn stop(&self) {}
    }

    #[derive(Default)]
    struct LaneControl {
        holds: AtomicUsize,
        reholds: AtomicUsize,
        refuse_release: AtomicBool,
        panic: AtomicBool,
        failure: Mutex<Option<StreamFailure>>,
    }

    impl LaneControl {
        fn holds(&self) -> usize {
            self.holds.load(Ordering::SeqCst)
        }
    }

    struct FakeStream {
        control: Arc<LaneControl>,
        released: Arc<AtomicBool>,
        ended: Mutex<Option<StreamFailure>>,
    }

    impl CaptureStream for FakeStream {
        type Block = Vec<u8>;
        type Stop = NoStop;

        fn stop_handle(&self) -> NoStop {
            NoStop
        }

        fn next_block(&self, _timeout: Duration) -> Next<Vec<u8>> {
            thread::sleep(Duration::from_millis(1));
            if self.control.panic.swap(false, Ordering::SeqCst) {
                panic!("a lane thread broke");
            }
            if let Some(failure) = lock(&self.control.failure).take() {
                *lock(&self.ended) = Some(failure);
                return Next::Ended;
            }
            if self.released.load(Ordering::Acquire) {
                Next::Block(vec![128; 64])
            } else {
                Next::Idle
            }
        }

        fn dropped(&self) -> u64 {
            0
        }

        fn failure(&self) -> StreamFailure {
            lock(&self.ended).clone().unwrap_or(StreamFailure {
                reason: "ended".to_owned(),
                gone: false,
            })
        }
    }

    struct FakeGate {
        lane: usize,
        released: Arc<AtomicBool>,
        control: Arc<LaneControl>,
        log: Arc<Mutex<Vec<Release>>>,
    }

    struct FakeLane {
        lane: usize,
        control: Arc<LaneControl>,
        log: Arc<Mutex<Vec<Release>>>,
    }

    impl BankLane for FakeLane {
        type Stream = FakeStream;
        type Gate = FakeGate;

        fn hold(&mut self) -> Result<(FakeStream, FakeGate), DeviceError> {
            self.control.holds.fetch_add(1, Ordering::SeqCst);
            let released = Arc::new(AtomicBool::new(false));
            Ok((
                FakeStream {
                    control: self.control.clone(),
                    released: released.clone(),
                    ended: Mutex::new(None),
                },
                FakeGate {
                    lane: self.lane,
                    released,
                    control: self.control.clone(),
                    log: self.log.clone(),
                },
            ))
        }

        fn release(gate: &FakeGate) -> Result<(), DeviceError> {
            if gate.control.refuse_release.load(Ordering::SeqCst) {
                return Err(DeviceError::Io("the endpoint stayed in reset".to_owned()));
            }
            let current = thread::current();
            lock(&gate.log).push(Release {
                lane: gate.lane,
                thread: current.id(),
                name: current.name().map(str::to_owned),
                at: Instant::now(),
            });
            gate.released.store(true, Ordering::Release);
            Ok(())
        }

        fn rehold(gate: &FakeGate) -> Result<(), DeviceError> {
            gate.control.reholds.fetch_add(1, Ordering::SeqCst);
            gate.released.store(false, Ordering::Release);
            Ok(())
        }
    }

    pub(crate) type Lane = Arc<Mutex<Vec<Seen>>>;

    pub(crate) struct FakeBank {
        controls: Vec<Arc<LaneControl>>,
        log: Arc<Mutex<Vec<Release>>>,
        fatal: Arc<Mutex<Vec<(usize, String)>>>,
    }

    impl FakeBank {
        pub(crate) fn new(lanes: usize) -> Self {
            Self {
                controls: (0..lanes).map(|_| Arc::default()).collect(),
                log: Arc::default(),
                fatal: Arc::default(),
            }
        }

        fn sink(&self, lane: usize) -> (RxSink, Lane) {
            let seen: Lane = Arc::default();
            let log = seen.clone();
            let fatal = self.fatal.clone();
            let sink = RxSink::with_items(
                move |item: SinkItem<'_>| {
                    lock(&log).push(match item {
                        SinkItem::Samples { samples, index } => Seen::Samples {
                            index,
                            len: samples.len(),
                        },
                        SinkItem::Event(event) => Seen::Event(event),
                    });
                },
                move |error| lock(&fatal).push((lane, error.to_string())),
            );
            (sink, seen)
        }

        fn try_start(&self, sinks: usize) -> Result<(Bank, Vec<Lane>), DeviceError> {
            let lanes = self
                .controls
                .iter()
                .enumerate()
                .map(|(lane, control)| {
                    Arc::new(Mutex::new(FakeLane {
                        lane,
                        control: control.clone(),
                        log: self.log.clone(),
                    }))
                })
                .collect();
            let (sinks, seen): (Vec<RxSink>, Vec<Lane>) =
                (0..sinks).map(|lane| self.sink(lane)).unzip();
            let config =
                CaptureConfig::new("sdrmm-test-rx", "kraken").with_sample_rate(Some(2.4e6));
            Bank::start(lanes, sinks, config, crate::convert::converter).map(|bank| (bank, seen))
        }

        pub(crate) fn start(&self) -> (Bank, Vec<Lane>) {
            self.try_start(self.controls.len())
                .expect("the bank starts")
        }

        pub(crate) fn wait_for_samples(&self, seen: &[Lane], blocks: usize) {
            wait("every lane to deliver", || {
                seen.iter().all(|lane| {
                    lock(lane)
                        .iter()
                        .filter(|item| matches!(item, Seen::Samples { .. }))
                        .count()
                        >= blocks
                })
            });
        }

        fn fail(&self, lane: usize, gone: bool) {
            *lock(&self.controls[lane].failure) = Some(StreamFailure {
                reason: format!("lane {lane} broke"),
                gone,
            });
        }

        fn fatal(&self) -> Vec<(usize, String)> {
            lock(&self.fatal).clone()
        }
    }

    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn samples_after(lane: &Lane, from: usize) -> usize {
        lock(lane)[from..]
            .iter()
            .filter(|item| matches!(item, Seen::Samples { .. }))
            .count()
    }

    #[test]
    fn gates_are_released_back_to_back_from_one_thread() {
        let fake = FakeBank::new(5);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        bank.stop();
        let log = lock(&fake.log).clone();
        assert_eq!(
            log.iter().map(|release| release.lane).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(log.iter().all(|release| release.thread == log[0].thread));
        assert_eq!(log[0].name.as_deref(), Some(SUPERVISOR));
        let spread = log[4].at.duration_since(log[0].at);
        assert!(spread < Duration::from_millis(20), "{spread:?}");
    }

    #[test]
    fn a_lane_io_error_restarts_the_bank_in_place() {
        let fake = FakeBank::new(5);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        fake.fail(2, false);
        wait("every lane to be held again", || {
            fake.controls.iter().all(|control| control.holds() == 2)
        });
        let marks: Vec<usize> = seen.iter().map(|lane| lock(lane).len()).collect();
        wait("samples after the restart", || {
            seen.iter()
                .zip(&marks)
                .all(|(lane, from)| samples_after(lane, *from) > 0)
        });
        assert!(bank.is_running());
        bank.stop();
        assert!(fake.fatal().is_empty(), "{:?}", fake.fatal());
    }

    #[test]
    fn a_lane_failed_on_purpose_restarts_the_bank() {
        let fake = FakeBank::new(5);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        bank.fail_lane_for_test(2);
        wait("every lane to be held again", || {
            fake.controls.iter().all(|control| control.holds() == 2)
        });
        bank.stop();
        assert!(fake.fatal().is_empty(), "{:?}", fake.fatal());
    }

    #[test]
    fn a_panicking_lane_restarts_the_bank_instead_of_going_quiet() {
        let fake = FakeBank::new(3);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        fake.controls[1].panic.store(true, Ordering::SeqCst);
        wait("every lane to be held again", || {
            fake.controls.iter().all(|control| control.holds() == 2)
        });
        let marks: Vec<usize> = seen.iter().map(|lane| lock(lane).len()).collect();
        wait("samples after the restart", || {
            seen.iter()
                .zip(&marks)
                .all(|(lane, from)| samples_after(lane, *from) > 0)
        });
        bank.stop();
        assert!(fake.fatal().is_empty(), "{:?}", fake.fatal());
    }

    #[test]
    fn a_restart_counts_the_gap_at_the_rate_the_bank_runs_at_now() {
        let fake = FakeBank::new(2);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        bank.retime(1.0);
        fake.fail(0, false);
        wait("every lane to be held again", || {
            fake.controls.iter().all(|control| control.holds() == 2)
        });
        wait("the restart to be reported", || {
            seen.iter().all(|lane| {
                lock(lane).iter().any(|item| {
                    matches!(
                        item,
                        Seen::Event(LaneEvent::Uncertain {
                            cause: Uncertainty::Rearmed,
                            ..
                        })
                    )
                })
            })
        });
        bank.stop();
        for lane in &seen {
            let estimate = lock(lane).iter().find_map(|item| match item {
                Seen::Event(LaneEvent::Uncertain {
                    cause: Uncertainty::EstimatedGap,
                    error,
                    ..
                }) => Some(*error),
                _ => None,
            });
            assert_eq!(estimate, Some(1), "a restart under a second at 1 S/s");
        }
    }

    #[test]
    fn a_disconnected_lane_faults_the_bank() {
        let fake = FakeBank::new(5);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        fake.fail(1, true);
        wait("every lane to hear of the fault", || {
            fake.fatal().len() == 5
        });
        wait("the supervisor to end", || !bank.is_running());
        let mut told: Vec<usize> = fake.fatal().iter().map(|(lane, _)| *lane).collect();
        told.sort_unstable();
        assert_eq!(told, vec![0, 1, 2, 3, 4]);
        assert!(
            fake.fatal()
                .iter()
                .all(|(_, why)| why.contains("no longer attached") && why.contains("lane 1 broke"))
        );
        assert!(fake.controls.iter().all(|control| control.holds() == 1));
        bank.stop();
    }

    #[test]
    fn a_restart_reports_an_uncertain_timeline() {
        let fake = FakeBank::new(3);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 2);
        fake.fail(0, false);
        wait("every lane to be held again", || {
            fake.controls.iter().all(|control| control.holds() == 2)
        });
        let marks: Vec<usize> = seen.iter().map(|lane| lock(lane).len()).collect();
        wait("samples after the restart", || {
            seen.iter()
                .zip(&marks)
                .all(|(lane, from)| samples_after(lane, *from) > 0)
        });
        bank.stop();
        for lane in &seen {
            let items = lock(lane).clone();
            let estimated = items
                .iter()
                .position(|item| {
                    matches!(
                        item,
                        Seen::Event(LaneEvent::Uncertain {
                            cause: Uncertainty::EstimatedGap,
                            scope: GapScope::Lane,
                            ..
                        })
                    )
                })
                .expect("the gap is an estimate");
            let Seen::Event(LaneEvent::Uncertain { at, error, .. }) = items[estimated] else {
                unreachable!();
            };
            assert!(error > 0, "a restart takes time, so samples were lost");
            assert_eq!(
                items[estimated + 1],
                Seen::Event(LaneEvent::Uncertain {
                    at,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Lane,
                    cause: Uncertainty::Rearmed,
                })
            );
            let before = items[..estimated]
                .iter()
                .rev()
                .find_map(|item| match item {
                    Seen::Samples { index, len } => Some(index + *len as u64),
                    Seen::Event(_) => None,
                })
                .expect("samples before the fault");
            assert_eq!(at, before + error, "the index jumps by the estimate");
            let resumed = items[estimated + 2..]
                .iter()
                .find_map(|item| match item {
                    Seen::Samples { index, .. } => Some(*index),
                    Seen::Event(_) => None,
                })
                .expect("samples after the restart");
            assert_eq!(resumed, at);
        }
    }

    #[test]
    fn a_refused_release_holds_the_released_lanes_again_and_fails_every_sink() {
        let fake = FakeBank::new(4);
        fake.controls[2]
            .refuse_release
            .store(true, Ordering::SeqCst);
        let (mut bank, _seen) = fake.start();
        wait("every lane to hear of the fault", || {
            fake.fatal().len() == 4
        });
        assert!(
            fake.fatal()
                .iter()
                .all(|(_, why)| why.contains("stayed in reset"))
        );
        let reheld: Vec<usize> = fake
            .controls
            .iter()
            .map(|control| control.reholds.load(Ordering::SeqCst))
            .collect();
        assert_eq!(reheld, vec![1, 1, 0, 0]);
        bank.stop();
    }

    #[test]
    fn the_wrong_number_of_sinks_is_refused() {
        let fake = FakeBank::new(5);
        let Err(DeviceError::Unsupported(message)) = fake.try_start(4) else {
            panic!("four sinks for five lanes must be refused");
        };
        assert!(message.contains('5') && message.contains('4'), "{message}");
        assert!(fake.controls.iter().all(|control| control.holds() == 0));
    }

    #[test]
    fn a_stopped_bank_stops_every_lane() {
        let fake = FakeBank::new(3);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        bank.stop();
        assert!(!bank.is_running());
        let counts: Vec<usize> = seen.iter().map(|lane| lock(lane).len()).collect();
        thread::sleep(Duration::from_millis(20));
        let later: Vec<usize> = seen.iter().map(|lane| lock(lane).len()).collect();
        assert_eq!(counts, later);
        assert!(fake.fatal().is_empty());
    }
}
