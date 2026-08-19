use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use num_complex::Complex;
use sdrmm_device::DeviceError;

mod align;
mod tap;

pub(crate) use align::Aligner;
pub(crate) use tap::{CoherentTaps, lane_taps};

/// Backstop only: the aggregator has samples to chew on almost every pass.
const IDLE_PARK: Duration = Duration::from_millis(5);

/// What every lane's range shares: where it sits in the stream, and what the front end was doing
/// when it was captured.
#[derive(Clone, Copy, Debug)]
pub struct AlignedContext {
    pub index: u64,
    pub sample_rate: f64,
    pub center_hz: f64,
    /// Set once after every realignment, so a processor with internal history can start over
    /// rather than average across a discontinuity it cannot see.
    pub realigned: bool,
}

/// Something the aggregator drives with lanes that are already lined up and calibrated.
pub(crate) trait AlignedSink: Send {
    fn process(&mut self, lanes: &[&[Complex<f32>]], ctx: AlignedContext);
}

pub(crate) enum CoherentCommand {
    Add {
        node: u32,
        sink: Box<dyn AlignedSink>,
    },
    Remove {
        node: u32,
    },
    Meta {
        center_hz: f64,
    },
}

/// The cross-lane home the per-lane `DspCommand` queues deliberately are not.
///
/// One OS thread per device set pops the largest range every lane covers, hands it to whatever
/// coherent processors are running, and stays under the same rules as the per-lane dsp threads:
/// no locks, no allocation, no async once it is going.
pub struct CoherentRuntime {
    cmd_tx: mpsc::Sender<CoherentCommand>,
    stop: Arc<AtomicBool>,
    armed: Arc<AtomicBool>,
    realignments: Arc<AtomicU64>,
    thread: Option<JoinHandle<CoherentTaps>>,
}

impl CoherentRuntime {
    pub(crate) fn start(
        set: u32,
        mut taps: CoherentTaps,
        center_hz: f64,
    ) -> Result<Self, DeviceError> {
        taps.rewind();
        let stop = Arc::new(AtomicBool::new(false));
        let armed = taps.armed.clone();
        let realignments = Arc::new(AtomicU64::new(0));
        let (cmd_tx, cmd_rx) = mpsc::channel::<CoherentCommand>();
        let halt = stop.clone();
        let counted = realignments.clone();
        armed.store(true, Ordering::Release);
        let spawned = std::thread::Builder::new()
            .name(format!("sdrmm-coh-{set}"))
            .spawn(move || {
                sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
                aggregate(Aligner::new(taps), &cmd_rx, &halt, &counted, center_hz)
            });
        match spawned {
            Ok(thread) => Ok(Self {
                cmd_tx,
                stop,
                armed,
                realignments,
                thread: Some(thread),
            }),
            Err(e) => {
                armed.store(false, Ordering::Release);
                Err(DeviceError::Io(format!("spawn coherent thread: {e}")))
            }
        }
    }

    pub(crate) fn send(&self, command: CoherentCommand) {
        let _ = self.cmd_tx.send(command);
    }

    #[must_use]
    pub fn realignments(&self) -> u64 {
        self.realignments.load(Ordering::Relaxed)
    }

    /// Stops the aggregator and hands the taps back, so the next coherent node can pick them up
    /// without the capture stream ever being touched.
    pub(crate) fn stop(mut self) -> Option<CoherentTaps> {
        self.stop.store(true, Ordering::Release);
        self.armed.store(false, Ordering::Release);
        self.thread.take().and_then(|thread| thread.join().ok())
    }
}

impl Drop for CoherentRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.armed.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn aggregate(
    mut aligner: Aligner,
    commands: &mpsc::Receiver<CoherentCommand>,
    stop: &AtomicBool,
    realignments: &AtomicU64,
    center_hz: f64,
) -> CoherentTaps {
    let mut sinks: Vec<(u32, Box<dyn AlignedSink>)> = Vec::new();
    let mut seen = 0u64;
    let mut center = center_hz;
    while !stop.load(Ordering::Acquire) {
        drain_commands(commands, &mut sinks, &mut center);
        let Some(count) = aligner.next() else {
            std::thread::park_timeout(IDLE_PARK);
            continue;
        };
        let realigned = aligner.realignments() != seen;
        seen = aligner.realignments();
        realignments.store(seen, Ordering::Relaxed);
        if sinks.is_empty() {
            continue;
        }
        let ctx = AlignedContext {
            index: aligner.index(),
            sample_rate: aligner.sample_rate(),
            center_hz: center,
            realigned,
        };
        aligner.with_lanes(count, |lanes| {
            for (_, sink) in &mut sinks {
                sink.process(lanes, ctx);
            }
        });
    }
    aligner.release()
}

fn drain_commands(
    commands: &mpsc::Receiver<CoherentCommand>,
    sinks: &mut Vec<(u32, Box<dyn AlignedSink>)>,
    center: &mut f64,
) {
    while let Ok(command) = commands.try_recv() {
        match command {
            CoherentCommand::Add { node, sink } => {
                sinks.retain(|(existing, _)| *existing != node);
                sinks.push((node, sink));
            }
            CoherentCommand::Remove { node } => sinks.retain(|(existing, _)| *existing != node),
            CoherentCommand::Meta { center_hz } => *center = center_hz,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc as std_mpsc};

    use super::*;

    struct Recorder(std_mpsc::Sender<(u64, usize)>);

    impl AlignedSink for Recorder {
        fn process(&mut self, lanes: &[&[Complex<f32>]], ctx: AlignedContext) {
            let _ = self.0.send((ctx.index, lanes.len()));
        }
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn the_aggregator_delivers_aligned_lanes_to_its_sinks() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut taps, shared) = lane_taps(2, 48_000.0);
        let runtime = CoherentRuntime::start(0, shared, 100.0).expect("start");
        let (tx, rx) = std_mpsc::channel();
        runtime.send(CoherentCommand::Add {
            node: 1,
            sink: Box::new(Recorder(tx)),
        });
        for round in 0..40 {
            for tap in &mut taps {
                tap.push(&vec![Complex::new(1.0, 0.0); 256], round * 256);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let (_, lanes) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a range reaches the sink");
        assert_eq!(lanes, 2);
        assert!(runtime.stop().is_some());
    }
}
