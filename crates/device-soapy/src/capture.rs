use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceError, Recovery, RestartPolicy, RxSink, Sample};

use crate::{
    ProbeIdentity, map_err,
    soapy::{self, ErrorCode},
    watchdog::{Watch, Watchdog},
};

const READ_TIMEOUT_US: i64 = 100_000;
const MIN_BLOCK: usize = 8192;
const OVERFLOW_LOG_EVERY: u64 = 1000;
const PARK_POLL: Duration = Duration::from_millis(2);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

/// Hands the stream back to the control thread for the settings a radio can only take while it is
/// not streaming: a bladeRF resets its sample counter inside `setSampleRate`, which leaves the
/// sync worker reading into a stream that no longer exists.
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

pub(crate) struct RxPlan {
    device: soapy::Device,
    channels: Vec<usize>,
}

impl RxPlan {
    pub(crate) const fn new(device: soapy::Device, channels: Vec<usize>) -> Self {
        Self { device, channels }
    }

    pub(crate) fn arm(&self) -> Result<RxStreams, DeviceError> {
        let mut streams = match self.device.rx_stream::<Sample>(&self.channels) {
            Ok(stream) => RxStreams::Combined(stream),
            Err(combined) if self.channels.len() > 1 => {
                match self
                    .channels
                    .iter()
                    .map(|channel| self.device.rx_stream::<Sample>(&[*channel]))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(streams) => RxStreams::Split(streams),
                    Err(split) => {
                        return Err(DeviceError::Io(format!(
                            "soapy multi-channel stream setup failed: combined: {combined}; \
                             split: {split}"
                        )));
                    }
                }
            }
            Err(error) => return Err(map_err(error)),
        };
        streams.activate().map_err(map_err)?;
        Ok(streams)
    }
}

pub(crate) enum RxStreams {
    Combined(soapy::RxStream<Sample>),
    Split(Vec<soapy::RxStream<Sample>>),
}

impl RxStreams {
    fn activate(&mut self) -> Result<(), soapy::Error> {
        match self {
            Self::Combined(stream) => stream.activate(None),
            Self::Split(streams) => {
                for index in 0..streams.len() {
                    if let Err(error) = streams[index].activate(None) {
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

enum Outcome {
    Stopped,
    Rearm,
    Failed(String),
    Gone(String),
}

enum Stall {
    Wait,
    Failed(String),
    Gone(String),
}

pub(crate) fn run(
    plan: &RxPlan,
    armed: RxStreams,
    identity: &ProbeIdentity,
    quiesce: &Quiesce,
    running: &AtomicBool,
    mut sinks: Vec<RxSink>,
) {
    let sinks = &mut sinks[..];
    let mut policy = RestartPolicy::default();
    let mut streams = Some(armed);
    quiesce.held(true);
    while running.load(Ordering::Acquire) {
        let Some(mut active) = streams.take() else {
            while quiesce.asked_for() && running.load(Ordering::Acquire) {
                thread::sleep(PARK_POLL);
            }
            if !running.load(Ordering::Acquire) {
                return;
            }
            match plan.arm() {
                Ok(fresh) => {
                    quiesce.held(true);
                    streams = Some(fresh);
                }
                Err(error) => {
                    if exhausted(
                        &mut policy,
                        Duration::ZERO,
                        sinks,
                        &format!("stream restart failed: {error}"),
                    ) {
                        return;
                    }
                }
            }
            continue;
        };
        let started = Instant::now();
        let outcome = session(&mut active, identity, quiesce, running, sinks);
        if let Err(error) = active.deactivate() {
            tracing::debug!("soapy stream deactivate failed: {error}");
        }
        drop(active);
        quiesce.held(false);
        match outcome {
            Outcome::Stopped => return,
            Outcome::Gone(reason) => {
                fail_all_gone(sinks, &reason);
                return;
            }
            Outcome::Rearm => {}
            Outcome::Failed(reason) => {
                if exhausted(&mut policy, started.elapsed(), sinks, &reason) {
                    return;
                }
            }
        }
    }
}

fn session(
    streams: &mut RxStreams,
    identity: &ProbeIdentity,
    quiesce: &Quiesce,
    running: &AtomicBool,
    sinks: &mut [RxSink],
) -> Outcome {
    match streams {
        RxStreams::Combined(stream) => combined(stream, identity, quiesce, running, sinks),
        RxStreams::Split(streams) => split(streams, identity, quiesce, running, sinks),
    }
}

fn combined(
    stream: &mut soapy::RxStream<Sample>,
    identity: &ProbeIdentity,
    quiesce: &Quiesce,
    running: &AtomicBool,
    sinks: &mut [RxSink],
) -> Outcome {
    let block = stream.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK);
    let mut buffers = vec![vec![Sample::new(0.0, 0.0); block]; sinks.len()];
    let mut watchdog = Watchdog::new(Instant::now());
    let mut overflows = 0u64;
    while running.load(Ordering::Acquire) {
        if quiesce.asked_for() {
            return Outcome::Rearm;
        }
        let result = {
            let mut slices: Vec<&mut [Sample]> =
                buffers.iter_mut().map(Vec::as_mut_slice).collect();
            stream.read(&mut slices, READ_TIMEOUT_US)
        };
        match result {
            Ok(count) => {
                watchdog.delivered(Instant::now());
                if count > 0 {
                    for (sink, buffer) in sinks.iter_mut().zip(&buffers) {
                        sink.push(&buffer[..count]);
                    }
                }
            }
            Err(error) if error.code == ErrorCode::Timeout => {
                match stalled(&mut watchdog, identity, None) {
                    Stall::Wait => {}
                    Stall::Gone(reason) => return Outcome::Gone(reason),
                    Stall::Failed(reason) => return Outcome::Failed(reason),
                }
            }
            Err(error) if error.code == ErrorCode::Overflow => {
                overflows += 1;
                log_overflow(None, overflows);
            }
            Err(error) => return Outcome::Failed(format!("stream read failed: {error}")),
        }
    }
    Outcome::Stopped
}

fn split(
    streams: &mut [soapy::RxStream<Sample>],
    identity: &ProbeIdentity,
    quiesce: &Quiesce,
    running: &AtomicBool,
    sinks: &mut [RxSink],
) -> Outcome {
    let mut buffers: Vec<Vec<Sample>> = streams
        .iter()
        .map(|stream| vec![Sample::new(0.0, 0.0); stream.mtu().unwrap_or(MIN_BLOCK).max(MIN_BLOCK)])
        .collect();
    let mut watchdogs: Vec<Watchdog> = (0..streams.len())
        .map(|_| Watchdog::new(Instant::now()))
        .collect();
    let mut overflows = vec![0u64; streams.len()];
    while running.load(Ordering::Acquire) {
        if quiesce.asked_for() {
            return Outcome::Rearm;
        }
        for channel in 0..streams.len() {
            let result = streams[channel].read(&mut [&mut buffers[channel]], READ_TIMEOUT_US);
            match result {
                Ok(count) => {
                    watchdogs[channel].delivered(Instant::now());
                    if count > 0 {
                        sinks[channel].push(&buffers[channel][..count]);
                    }
                }
                Err(error) if error.code == ErrorCode::Timeout => {
                    match stalled(&mut watchdogs[channel], identity, Some(channel)) {
                        Stall::Wait => {}
                        Stall::Gone(reason) => return Outcome::Gone(reason),
                        Stall::Failed(reason) => return Outcome::Failed(reason),
                    }
                }
                Err(error) if error.code == ErrorCode::Overflow => {
                    overflows[channel] += 1;
                    log_overflow(Some(channel), overflows[channel]);
                }
                Err(error) => {
                    return Outcome::Failed(labelled(
                        Some(channel),
                        &format!("read failed: {error}"),
                    ));
                }
            }
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
    use super::*;

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
