use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceError, GapScope, Recovery, RestartPolicy, RxSink, Sample, UNKNOWN_ERROR, Uncertainty,
};

use crate::{
    ProbeIdentity, map_err,
    soapy::{self, Direction, ErrorCode, ReadResult},
    watchdog::{Watch, Watchdog},
};

const READ_TIMEOUT_US: i64 = 100_000;
const MIN_BLOCK: usize = 8192;
const OVERFLOW_LOG_EVERY: u64 = 1000;
const PARK_POLL: Duration = Duration::from_millis(2);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
const START_DELAY_NS: i64 = 100_000_000;

#[derive(Debug, Default)]
pub(crate) struct Quiesce {
    wanted: AtomicBool,
    holding: AtomicBool,
}

impl Quiesce {
    pub(crate) fn pause(self: &Arc<Self>, capturing: bool) -> Option<Paused> {
        if !capturing {
            return None;
        }
        self.wanted.store(true, Ordering::Release);
        let paused = Paused(self.clone());
        let deadline = Instant::now() + RELEASE_TIMEOUT;
        while self.holding.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                tracing::warn!("capture thread kept the stream; writing settings onto it anyway");
                break;
            }
            thread::sleep(PARK_POLL);
        }
        Some(paused)
    }

    fn asked_for(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
    }

    fn held(&self, holding: bool) {
        self.holding.store(holding, Ordering::Release);
    }
}

pub(crate) struct Paused(Arc<Quiesce>);

impl Drop for Paused {
    fn drop(&mut self) {
        self.0.wanted.store(false, Ordering::Release);
    }
}

pub(crate) trait RxRead: Send {
    fn block_len(&self) -> usize;
    fn read_into(
        &mut self,
        buffers: &mut [Vec<Sample>],
        timeout_us: i64,
    ) -> Result<ReadResult, soapy::Error>;
}

impl RxRead for soapy::RxStream<Sample> {
    fn block_len(&self) -> usize {
        self.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK)
    }

    fn read_into(
        &mut self,
        buffers: &mut [Vec<Sample>],
        timeout_us: i64,
    ) -> Result<ReadResult, soapy::Error> {
        self.read(buffers, timeout_us)
    }
}

pub(crate) struct RxPlan {
    device: soapy::Device,
    channels: Vec<usize>,
}

impl RxPlan {
    pub(crate) const fn new(device: soapy::Device, channels: Vec<usize>) -> Self {
        Self { device, channels }
    }

    pub(crate) fn arm(&self) -> Result<Armed, DeviceError> {
        let first = self.channels.first().copied().unwrap_or(0);
        let mut armed = Armed {
            streams: self.open()?,
            rate: self.device.sample_rate(Direction::Rx, first).unwrap_or(0.0),
            timed: false,
        };
        armed.activate(self.start_time()).map_err(map_err)?;
        Ok(armed)
    }

    fn open(&self) -> Result<RxStreams, DeviceError> {
        match self.device.rx_stream::<Sample>(&self.channels) {
            Ok(stream) => Ok(RxStreams::Combined(stream)),
            Err(combined) if self.channels.len() > 1 => self
                .channels
                .iter()
                .map(|channel| self.device.rx_stream::<Sample>(&[*channel]))
                .collect::<Result<Vec<_>, _>>()
                .map(RxStreams::Split)
                .map_err(|split| {
                    DeviceError::Io(format!(
                        "soapy multi-channel stream setup failed: combined: {combined}; \
                         split: {split}"
                    ))
                }),
            Err(error) => Err(map_err(error)),
        }
    }

    fn start_time(&self) -> Option<i64> {
        if !self.device.has_hardware_time(None).unwrap_or(false) {
            return None;
        }
        self.device
            .get_hardware_time(None)
            .ok()
            .map(|now| now.saturating_add(START_DELAY_NS))
    }
}

pub(crate) struct Armed {
    streams: RxStreams,
    rate: f64,
    timed: bool,
}

impl Armed {
    fn activate(&mut self, start: Option<i64>) -> Result<(), soapy::Error> {
        if let Some(at) = start {
            match self.streams.activate(Some(at)) {
                Ok(()) => {
                    self.timed = true;
                    return Ok(());
                }
                Err(error) => tracing::debug!("soapy refused a timed start: {error}"),
            }
        }
        self.timed = false;
        self.streams.activate(None)
    }
}

enum RxStreams {
    Combined(soapy::RxStream<Sample>),
    Split(Vec<soapy::RxStream<Sample>>),
}

impl RxStreams {
    fn activate(&mut self, at: Option<i64>) -> Result<(), soapy::Error> {
        match self {
            Self::Combined(stream) => stream.activate(at),
            Self::Split(streams) => {
                for index in 0..streams.len() {
                    if let Err(error) = streams[index].activate(at) {
                        for active in &mut streams[..index] {
                            let _ = active.deactivate(None);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }

    fn deactivate(&mut self) -> Result<(), soapy::Error> {
        match self {
            Self::Combined(stream) => stream.deactivate(None),
            Self::Split(streams) => streams
                .iter_mut()
                .map(|stream| stream.deactivate(None))
                .fold(Ok(()), Result::and),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Stopped,
    Rearm,
    Failed(String),
    Gone(String),
}

impl Outcome {
    const fn rank(&self) -> u8 {
        match self {
            Self::Stopped => 0,
            Self::Rearm => 1,
            Self::Failed(_) => 2,
            Self::Gone(_) => 3,
        }
    }

    fn worse(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

enum Stall {
    Wait,
    Failed(String),
    Gone(String),
}

#[derive(Clone, Copy)]
struct Session<'a> {
    identity: &'a ProbeIdentity,
    quiesce: &'a Quiesce,
    running: &'a AtomicBool,
}

impl Session<'_> {
    fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

pub(crate) fn run(
    plan: &RxPlan,
    armed: Armed,
    identity: &ProbeIdentity,
    quiesce: &Quiesce,
    running: &AtomicBool,
    mut sinks: Vec<RxSink>,
) {
    let session = Session {
        identity,
        quiesce,
        running,
    };
    let sinks = &mut sinks[..];
    let mut policy = RestartPolicy::default();
    let mut streams = Some(armed);
    let mut ended: Option<Instant> = None;
    quiesce.held(true);
    while session.running() {
        let Some(mut active) = streams.take() else {
            while quiesce.asked_for() && session.running() {
                thread::sleep(PARK_POLL);
            }
            if !session.running() {
                return;
            }
            match plan.arm() {
                Ok(fresh) => {
                    quiesce.held(true);
                    streams = Some(fresh);
                }
                Err(error) => {
                    let reason = format!("stream restart failed: {error}");
                    if exhausted(&mut policy, Duration::ZERO, sinks, &reason) {
                        return;
                    }
                }
            }
            continue;
        };
        if let Some(since) = ended.take() {
            rearmed(sinks, since, active.rate);
        }
        let started = Instant::now();
        let outcome = session_of(&mut active, session, sinks);
        if let Err(error) = active.streams.deactivate() {
            tracing::debug!("soapy stream deactivate failed: {error}");
        }
        drop(active);
        quiesce.held(false);
        ended = Some(Instant::now());
        match outcome {
            Outcome::Stopped => return,
            Outcome::Gone(reason) => return fail_all_gone(sinks, &reason),
            Outcome::Rearm => {}
            Outcome::Failed(reason) => {
                if exhausted(&mut policy, started.elapsed(), sinks, &reason) {
                    return;
                }
            }
        }
    }
}

fn rearmed(sinks: &mut [RxSink], since: Instant, rate: f64) {
    let lost = (since.elapsed().as_secs_f64() * rate).ceil() as u64;
    for sink in sinks {
        sink.dropped_estimate(lost, estimate_error(lost), GapScope::Device);
        sink.realigned(Uncertainty::Rearmed, UNKNOWN_ERROR, GapScope::Device);
    }
}

const fn estimate_error(lost: u64) -> u64 {
    if lost == 0 { UNKNOWN_ERROR } else { lost }
}

fn session_of(armed: &mut Armed, session: Session<'_>, sinks: &mut [RxSink]) -> Outcome {
    let rate = armed.rate;
    match &mut armed.streams {
        RxStreams::Combined(stream) => combined(stream, rate, session, sinks),
        RxStreams::Split(streams) => split(streams, rate, armed.timed, session, sinks),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gap {
    Exact(u64),
    Estimated(u64),
    Backwards,
}

#[derive(Debug)]
struct Timeline {
    rate: f64,
    scope: GapScope,
    origin: Option<Instant>,
    counted: u64,
    next_ns: Option<i64>,
    overflowed: bool,
}

impl Timeline {
    const fn new(rate: f64, scope: GapScope) -> Self {
        Self {
            rate,
            scope,
            origin: None,
            counted: 0,
            next_ns: None,
            overflowed: false,
        }
    }

    fn gap(&mut self, time_ns: Option<i64>, samples: usize, now: Instant) -> Option<Gap> {
        if !std::mem::take(&mut self.overflowed) {
            return None;
        }
        match (time_ns, self.next_ns) {
            (Some(at), Some(expected)) if self.rate > 0.0 => {
                let missing = ((at - expected) as f64 * self.rate / 1e9).round();
                Some(if missing < 0.0 {
                    Gap::Backwards
                } else {
                    Gap::Exact(missing as u64)
                })
            }
            _ => Some(Gap::Estimated(self.estimate(samples, now))),
        }
    }

    fn estimate(&self, samples: usize, now: Instant) -> u64 {
        let Some(origin) = self.origin else {
            return 0;
        };
        let due = (now.duration_since(origin).as_secs_f64() * self.rate).round() as u64;
        due.saturating_sub(self.counted + samples as u64)
    }

    fn advance(&mut self, time_ns: Option<i64>, samples: usize, skipped: u64, now: Instant) {
        if self.origin.is_none() {
            self.origin = Some(now);
        } else {
            self.counted += skipped + samples as u64;
        }
        self.next_ns = time_ns
            .filter(|_| self.rate > 0.0)
            .map(|at| at + (samples as f64 * 1e9 / self.rate).round() as i64);
    }
}

fn book(sinks: &mut [RxSink], gap: Option<Gap>, scope: GapScope) -> u64 {
    match gap {
        None | Some(Gap::Exact(0)) => 0,
        Some(Gap::Exact(missing)) => {
            for sink in sinks {
                sink.dropped(missing);
            }
            missing
        }
        Some(Gap::Estimated(missing)) => {
            for sink in sinks {
                sink.dropped_estimate(missing, estimate_error(missing), scope);
            }
            missing
        }
        Some(Gap::Backwards) => {
            for sink in sinks {
                sink.realigned(Uncertainty::Overflow, UNKNOWN_ERROR, scope);
            }
            0
        }
    }
}

enum Read {
    Samples(ReadResult),
    Nothing,
    End(Outcome),
}

struct Reader {
    watchdog: Watchdog,
    overflows: u64,
    timeline: Timeline,
    lane: Option<usize>,
}

impl Reader {
    fn new(rate: f64, lane: Option<usize>) -> Self {
        let scope = if lane.is_some() {
            GapScope::Lane
        } else {
            GapScope::Device
        };
        Self {
            watchdog: Watchdog::new(Instant::now()),
            overflows: 0,
            timeline: Timeline::new(rate, scope),
            lane,
        }
    }

    fn take(&mut self, result: Result<ReadResult, soapy::Error>, identity: &ProbeIdentity) -> Read {
        match result {
            Ok(read) => {
                self.watchdog.delivered(Instant::now());
                if read.samples > 0 {
                    Read::Samples(read)
                } else {
                    Read::Nothing
                }
            }
            Err(error) if error.code == ErrorCode::Timeout => {
                match stalled(&mut self.watchdog, identity, self.lane) {
                    Stall::Wait => Read::Nothing,
                    Stall::Gone(reason) => Read::End(Outcome::Gone(reason)),
                    Stall::Failed(reason) => Read::End(Outcome::Failed(reason)),
                }
            }
            Err(error) if error.code == ErrorCode::Overflow => {
                self.overflows += 1;
                log_overflow(self.lane, self.overflows);
                self.timeline.overflowed = true;
                Read::Nothing
            }
            Err(error) => Read::End(Outcome::Failed(labelled(
                self.lane,
                &format!("read failed: {error}"),
            ))),
        }
    }

    fn deliver(&mut self, sinks: &mut [RxSink], buffers: &[Vec<Sample>], read: ReadResult) {
        let now = Instant::now();
        let skipped = book(
            sinks,
            self.timeline.gap(read.time_ns, read.samples, now),
            self.timeline.scope,
        );
        for (sink, buffer) in sinks.iter_mut().zip(buffers) {
            if let Some(ns) = read.time_ns {
                sink.stamp_hardware(ns);
            }
            sink.push(&buffer[..read.samples.min(buffer.len())]);
        }
        self.timeline
            .advance(read.time_ns, read.samples, skipped, now);
    }
}

fn combined<R: RxRead>(
    stream: &mut R,
    rate: f64,
    session: Session<'_>,
    sinks: &mut [RxSink],
) -> Outcome {
    let mut buffers = vec![vec![Sample::new(0.0, 0.0); stream.block_len()]; sinks.len()];
    let mut reader = Reader::new(rate, None);
    while session.running() {
        if session.quiesce.asked_for() {
            return Outcome::Rearm;
        }
        let result = stream.read_into(&mut buffers, READ_TIMEOUT_US);
        match reader.take(result, session.identity) {
            Read::Samples(read) => reader.deliver(sinks, &buffers, read),
            Read::Nothing => {}
            Read::End(outcome) => return outcome,
        }
    }
    Outcome::Stopped
}

fn split<R: RxRead>(
    streams: &mut [R],
    rate: f64,
    aligned: bool,
    session: Session<'_>,
    sinks: &mut [RxSink],
) -> Outcome {
    if !aligned {
        for sink in sinks.iter_mut() {
            sink.realigned(Uncertainty::Unaligned, UNKNOWN_ERROR, GapScope::Lane);
        }
    }
    let halt = &AtomicBool::new(false);
    thread::scope(|scope| {
        let mut readers = Vec::with_capacity(streams.len());
        for (channel, (stream, sink)) in streams.iter_mut().zip(sinks.iter_mut()).enumerate() {
            let spawned = thread::Builder::new()
                .name(format!("sdrmm-soapy-rx-{channel}"))
                .spawn_scoped(scope, move || {
                    let outcome = one_lane(stream, sink, channel, rate, session, halt);
                    if outcome != Outcome::Stopped {
                        halt.store(true, Ordering::Release);
                    }
                    outcome
                });
            match spawned {
                Ok(reader) => readers.push(reader),
                Err(error) => {
                    halt.store(true, Ordering::Release);
                    return Outcome::Failed(format!("stream {channel}: no read thread: {error}"));
                }
            }
        }
        readers
            .into_iter()
            .map(|reader| {
                reader
                    .join()
                    .unwrap_or_else(|_| Outcome::Failed("a read thread panicked".to_string()))
            })
            .fold(Outcome::Stopped, Outcome::worse)
    })
}

fn one_lane<R: RxRead>(
    stream: &mut R,
    sink: &mut RxSink,
    channel: usize,
    rate: f64,
    session: Session<'_>,
    halt: &AtomicBool,
) -> Outcome {
    let mut buffer = [vec![Sample::new(0.0, 0.0); stream.block_len()]];
    let mut reader = Reader::new(rate, Some(channel));
    while session.running() && !halt.load(Ordering::Acquire) {
        if session.quiesce.asked_for() {
            return Outcome::Rearm;
        }
        let result = stream.read_into(&mut buffer, READ_TIMEOUT_US);
        match reader.take(result, session.identity) {
            Read::Samples(read) => reader.deliver(std::slice::from_mut(sink), &buffer, read),
            Read::Nothing => {}
            Read::End(outcome) => return outcome,
        }
    }
    Outcome::Stopped
}

fn stalled(watchdog: &mut Watchdog, identity: &ProbeIdentity, lane: Option<usize>) -> Stall {
    match watchdog.timed_out(Instant::now()) {
        Watch::Wait => return Stall::Wait,
        Watch::Silent => return Stall::Failed(labelled(lane, &silent_stream(watchdog.silence()))),
        Watch::Probe => {}
    }
    match identity.is_present() {
        Ok(true) => {
            watchdog.present();
            Stall::Wait
        }
        Ok(false) => Stall::Gone("it no longer enumerates".to_string()),
        Err(probe) => {
            if watchdog.probe_failed() {
                Stall::Failed(format!("device lost: enumerate failed: {probe}"))
            } else {
                Stall::Wait
            }
        }
    }
}

fn exhausted(
    policy: &mut RestartPolicy,
    uptime: Duration,
    sinks: &mut [RxSink],
    reason: &str,
) -> bool {
    match policy.on_failure(uptime) {
        Recovery::RetryAfter { attempt, delay } => {
            tracing::warn!(attempt, reason, "soapy stream failed; restarting in place");
            thread::sleep(delay);
            false
        }
        Recovery::GiveUp { attempts } => {
            fail_all(
                sinks,
                &format!("{reason}; gave up after {attempts} restart attempts"),
            );
            true
        }
    }
}

fn labelled(lane: Option<usize>, message: &str) -> String {
    match lane {
        Some(lane) => format!("stream {lane}: {message}"),
        None => message.to_string(),
    }
}

fn log_overflow(lane: Option<usize>, overflows: u64) {
    if overflows == 1 || overflows.is_multiple_of(OVERFLOW_LOG_EVERY) {
        tracing::warn!(channel = lane, overflows, "soapy rx overflow");
    }
}

fn silent_stream(silence: Duration) -> String {
    format!(
        "the radio stopped sending samples for {silence:?} but is still plugged in: another \
         program may have taken it over, or it needs to be re-plugged"
    )
}

fn fail_all(sinks: &mut [RxSink], message: &str) {
    for sink in sinks {
        sink.fail(DeviceError::Io(message.to_string()));
    }
}

fn fail_all_gone(sinks: &mut [RxSink], reason: &str) {
    for sink in sinks {
        sink.fail(DeviceError::Disconnected(reason.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Mutex, PoisonError, atomic::AtomicUsize},
    };

    use sdrmm_device::{LaneEvent, SinkItem};

    use super::*;
    use crate::soapy::Args;

    const RATE: f64 = 1_000_000.0;
    const BLOCK: usize = 100;

    #[derive(Clone, Copy, Debug)]
    enum Step {
        Read {
            samples: usize,
            time_ns: Option<i64>,
        },
        Overflow,
    }

    struct Scripted {
        steps: VecDeque<Step>,
        running: Arc<AtomicBool>,
        left: Arc<AtomicUsize>,
        finished: bool,
    }

    impl Scripted {
        fn new(steps: &[Step], running: &Arc<AtomicBool>, left: &Arc<AtomicUsize>) -> Self {
            Self {
                steps: steps.iter().copied().collect(),
                running: running.clone(),
                left: left.clone(),
                finished: false,
            }
        }

        fn finish(&mut self) {
            if !std::mem::replace(&mut self.finished, true)
                && self.left.fetch_sub(1, Ordering::AcqRel) == 1
            {
                self.running.store(false, Ordering::Release);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    impl RxRead for Scripted {
        fn block_len(&self) -> usize {
            BLOCK
        }

        fn read_into(
            &mut self,
            buffers: &mut [Vec<Sample>],
            _timeout_us: i64,
        ) -> Result<ReadResult, soapy::Error> {
            match self.steps.pop_front() {
                Some(Step::Read { samples, time_ns }) => {
                    for buffer in buffers.iter_mut() {
                        buffer[..samples].fill(Sample::new(1.0, 0.0));
                    }
                    Ok(ReadResult {
                        samples,
                        flags: 0,
                        time_ns,
                    })
                }
                Some(Step::Overflow) => Err(soapy::Error {
                    code: ErrorCode::Overflow,
                    message: "overflow".to_string(),
                }),
                None => {
                    self.finish();
                    Ok(ReadResult {
                        samples: 0,
                        flags: 0,
                        time_ns: None,
                    })
                }
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Seen {
        Samples { index: u64, len: usize },
        Event(LaneEvent),
    }

    type Log = Arc<Mutex<Vec<Seen>>>;

    fn sinks(count: usize) -> (Vec<RxSink>, Vec<Log>) {
        (0..count)
            .map(|_| {
                let log: Log = Arc::default();
                let seen = log.clone();
                let sink = RxSink::with_items(
                    move |item: SinkItem<'_>| {
                        seen.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(match item {
                                SinkItem::Samples { samples, index } => Seen::Samples {
                                    index,
                                    len: samples.len(),
                                },
                                SinkItem::Event(event) => Seen::Event(event),
                            });
                    },
                    |_| {},
                );
                (sink, log)
            })
            .unzip()
    }

    fn seen(log: &Log) -> Vec<Seen> {
        log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn uncertain(log: &Log) -> Vec<(u64, u64, GapScope, Uncertainty)> {
        seen(log)
            .into_iter()
            .filter_map(|item| match item {
                Seen::Event(LaneEvent::Uncertain {
                    at,
                    error,
                    scope,
                    cause,
                }) => Some((at, error, scope, cause)),
                _ => None,
            })
            .collect()
    }

    fn indices(log: &Log) -> Vec<u64> {
        seen(log)
            .into_iter()
            .filter_map(|item| match item {
                Seen::Samples { index, .. } => Some(index),
                Seen::Event(_) => None,
            })
            .collect()
    }

    fn identity() -> ProbeIdentity {
        ProbeIdentity::from_args(&Args::from("driver=fake, serial=1"))
    }

    fn run_combined(steps: &[Step], lanes: usize) -> (Outcome, Vec<Log>) {
        let running = Arc::new(AtomicBool::new(true));
        let quiesce = Quiesce::default();
        let identity = identity();
        let session = Session {
            identity: &identity,
            quiesce: &quiesce,
            running: &running,
        };
        let (mut sinks, logs) = sinks(lanes);
        let mut stream = Scripted::new(steps, &running, &Arc::new(AtomicUsize::new(1)));
        let outcome = combined(&mut stream, RATE, session, &mut sinks);
        (outcome, logs)
    }

    const fn read(samples: usize, time_ns: Option<i64>) -> Step {
        Step::Read { samples, time_ns }
    }

    const MICROSECOND: i64 = 1_000;

    #[test]
    fn a_timed_overflow_becomes_an_exact_gap() {
        let (outcome, logs) = run_combined(
            &[
                read(BLOCK, Some(0)),
                Step::Overflow,
                read(BLOCK, Some(350 * MICROSECOND)),
                read(BLOCK, Some(450 * MICROSECOND)),
            ],
            2,
        );
        assert_eq!(outcome, Outcome::Stopped);
        for log in &logs {
            assert!(uncertain(log).is_empty(), "{:?}", seen(log));
            assert_eq!(indices(log), vec![0, 350, 450]);
            let stamps: Vec<(u64, i64)> = seen(log)
                .into_iter()
                .filter_map(|item| match item {
                    Seen::Event(LaneEvent::HardwareTime { at, ns }) => Some((at, ns)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                stamps,
                vec![(0, 0), (350, 350 * MICROSECOND), (450, 450 * MICROSECOND)]
            );
        }
    }

    #[test]
    fn a_timed_read_that_went_backwards_is_uncertain() {
        let (_, logs) = run_combined(
            &[
                read(BLOCK, Some(1_000_000)),
                Step::Overflow,
                read(BLOCK, Some(0)),
            ],
            1,
        );
        assert_eq!(
            uncertain(&logs[0]),
            vec![(100, UNKNOWN_ERROR, GapScope::Device, Uncertainty::Overflow)]
        );
    }

    #[test]
    fn an_untimed_overflow_is_a_device_wide_estimate() {
        let (_, logs) = run_combined(
            &[
                read(BLOCK, None),
                read(BLOCK, None),
                Step::Overflow,
                read(BLOCK, None),
            ],
            3,
        );
        for log in &logs {
            let events = uncertain(log);
            assert_eq!(events.len(), 1, "{:?}", seen(log));
            let (at, error, scope, cause) = events[0];
            assert_eq!(scope, GapScope::Device);
            assert_eq!(cause, Uncertainty::EstimatedGap);
            assert!(error > 0);
            assert_eq!(at, 200 + if error == UNKNOWN_ERROR { 0 } else { error });
            assert_eq!(indices(log).last(), Some(&at));
        }
    }

    #[test]
    fn an_untimed_gap_counts_what_the_wall_clock_says_is_missing() {
        let origin = Instant::now();
        let at = |micros: u64| origin + Duration::from_micros(micros);
        let mut timeline = Timeline::new(RATE, GapScope::Device);
        assert_eq!(timeline.gap(None, BLOCK, at(0)), None);
        timeline.advance(None, BLOCK, 0, at(0));
        timeline.advance(None, BLOCK, 0, at(100));
        timeline.overflowed = true;
        assert_eq!(
            timeline.gap(None, BLOCK, at(500)),
            Some(Gap::Estimated(300)),
            "500 us after the first block, 100 counted and 100 just read"
        );
        timeline.advance(None, BLOCK, 300, at(500));
        timeline.overflowed = true;
        assert_eq!(timeline.gap(None, BLOCK, at(600)), Some(Gap::Estimated(0)));
    }

    #[test]
    fn a_timed_gap_is_counted_from_the_hardware_clock() {
        let now = Instant::now();
        let mut timeline = Timeline::new(RATE, GapScope::Device);
        timeline.advance(Some(1_000_000), BLOCK, 0, now);
        timeline.overflowed = true;
        assert_eq!(
            timeline.gap(Some(1_000_000 + 350 * MICROSECOND), BLOCK, now),
            Some(Gap::Exact(250))
        );
        timeline.advance(Some(1_000_000 + 350 * MICROSECOND), BLOCK, 250, now);
        timeline.overflowed = true;
        assert_eq!(
            timeline.gap(None, BLOCK, now),
            Some(Gap::Estimated(0)),
            "an untimed read after a timed one falls back to the wall clock"
        );
    }

    #[test]
    fn reads_without_an_overflow_carry_no_gap() {
        let (_, logs) = run_combined(&[read(BLOCK, None), read(40, None), read(BLOCK, None)], 2);
        for log in &logs {
            assert_eq!(indices(log), vec![0, 100, 140]);
            assert!(uncertain(log).is_empty());
        }
    }

    fn run_split(aligned: bool) -> (Outcome, Vec<Log>) {
        let running = Arc::new(AtomicBool::new(true));
        let quiesce = Quiesce::default();
        let identity = identity();
        let session = Session {
            identity: &identity,
            quiesce: &quiesce,
            running: &running,
        };
        let (mut sinks, logs) = sinks(3);
        let left = Arc::new(AtomicUsize::new(3));
        let mut streams: Vec<Scripted> = (0..3)
            .map(|_| Scripted::new(&[read(BLOCK, None), read(BLOCK, None)], &running, &left))
            .collect();
        let outcome = split(&mut streams, RATE, aligned, session, &mut sinks);
        (outcome, logs)
    }

    #[test]
    fn split_streams_without_timed_start_are_unaligned() {
        let (outcome, logs) = run_split(false);
        assert_eq!(outcome, Outcome::Stopped);
        for log in &logs {
            assert_eq!(
                seen(log)[0],
                Seen::Event(LaneEvent::Uncertain {
                    at: 0,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Lane,
                    cause: Uncertainty::Unaligned,
                })
            );
            assert!(indices(log).starts_with(&[0, 100]));
        }
    }

    #[test]
    fn split_streams_started_on_one_hardware_time_stay_aligned() {
        let (_, logs) = run_split(true);
        for log in &logs {
            assert!(uncertain(log).is_empty());
            assert!(indices(log).starts_with(&[0, 100]));
        }
    }

    #[test]
    fn a_failed_split_lane_stops_its_neighbours() {
        struct Broken;

        impl RxRead for Broken {
            fn block_len(&self) -> usize {
                BLOCK
            }

            fn read_into(
                &mut self,
                _buffers: &mut [Vec<Sample>],
                _timeout_us: i64,
            ) -> Result<ReadResult, soapy::Error> {
                Err(soapy::Error {
                    code: ErrorCode::StreamError,
                    message: "broken".to_string(),
                })
            }
        }

        let running = AtomicBool::new(true);
        let quiesce = Quiesce::default();
        let identity = identity();
        let session = Session {
            identity: &identity,
            quiesce: &quiesce,
            running: &running,
        };
        let (mut sinks, _) = sinks(2);
        let mut streams = [Broken, Broken];
        let outcome = split(&mut streams, RATE, true, session, &mut sinks);
        assert!(matches!(outcome, Outcome::Failed(reason) if reason.starts_with("stream ")));
    }

    #[test]
    fn a_restart_estimates_the_samples_it_missed() {
        let (mut sinks, logs) = sinks(2);
        let since = Instant::now() - Duration::from_millis(10);
        rearmed(&mut sinks, since, RATE);
        for log in &logs {
            let events = uncertain(log);
            assert_eq!(events.len(), 2);
            let (at, error, scope, cause) = events[0];
            assert!(at >= 10_000, "{at}");
            assert_eq!(
                (error, scope, cause),
                (at, GapScope::Device, Uncertainty::EstimatedGap)
            );
            assert_eq!(
                events[1],
                (at, UNKNOWN_ERROR, GapScope::Device, Uncertainty::Rearmed)
            );
        }
    }

    #[test]
    fn the_worst_outcome_of_split_lanes_wins() {
        assert_eq!(Outcome::Stopped.worse(Outcome::Rearm), Outcome::Rearm);
        assert_eq!(
            Outcome::Failed("a".to_string()).worse(Outcome::Gone("b".to_string())),
            Outcome::Gone("b".to_string())
        );
        assert_eq!(
            Outcome::Gone("b".to_string()).worse(Outcome::Failed("a".to_string())),
            Outcome::Gone("b".to_string())
        );
    }

    #[test]
    fn a_pause_with_nothing_capturing_takes_no_guard() {
        let quiesce = Arc::new(Quiesce::default());
        assert!(quiesce.pause(false).is_none());
        assert!(!quiesce.asked_for());
    }

    #[test]
    fn a_pause_waits_for_the_capture_thread_to_let_go() {
        let quiesce = Arc::new(Quiesce::default());
        quiesce.held(true);
        let worker = {
            let quiesce = quiesce.clone();
            thread::spawn(move || {
                while !quiesce.asked_for() {
                    thread::sleep(PARK_POLL);
                }
                quiesce.held(false);
            })
        };
        let paused = quiesce.pause(true).expect("a capturing radio pauses");
        assert!(!quiesce.holding.load(Ordering::Acquire));
        worker.join().expect("the capture thread let go");
        drop(paused);
        assert!(!quiesce.asked_for(), "the stream is released again");
    }

    #[test]
    fn a_capture_thread_that_never_lets_go_does_not_block_settings_forever() {
        let quiesce = Arc::new(Quiesce::default());
        quiesce.held(true);
        let started = Instant::now();
        let paused = quiesce.pause(true).expect("a capturing radio pauses");
        assert!(started.elapsed() >= RELEASE_TIMEOUT);
        drop(paused);
    }

    #[test]
    fn a_lane_names_itself_in_the_fault_it_reports() {
        assert_eq!(labelled(Some(1), "read failed"), "stream 1: read failed");
        assert_eq!(labelled(None, "read failed"), "read failed");
    }
}
