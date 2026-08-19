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
use sdrmm_wire::{CalParams, CalState, Coherence};

mod align;
mod cal;
mod host;
mod tap;

pub(crate) use align::Aligner;
pub(crate) use host::CoherentHost;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use host::CoherentSinks;
pub use host::{CoherentUpdate, SurfaceUpdate};
pub(crate) use tap::{BeamSink, CoherentTaps, lane_taps};

/// Backstop only: the aggregator has samples to chew on almost every pass.
const IDLE_PARK: Duration = Duration::from_millis(5);

/// What every lane's range shares: where it sits in the stream, what the front end was doing when
/// it was captured, and what the calibration currently believes.
#[derive(Clone, Copy, Debug)]
pub struct AlignedContext<'a> {
    pub index: u64,
    pub sample_rate: f64,
    pub center_hz: f64,
    /// Set once after every realignment, so a processor with internal history can start over
    /// rather than average across a discontinuity it cannot see.
    pub realigned: bool,
    pub cal: &'a CalState,
}

/// Something the aggregator drives with lanes that are already lined up and calibrated.
pub(crate) trait AlignedSink: Send {
    fn process(&mut self, lanes: &[&[Complex<f32>]], ctx: AlignedContext<'_>);
}

type CoherentSinkList = Vec<Box<CoherentHost>>;

pub(crate) enum CoherentCommand {
    Add { node: u32, host: Box<CoherentHost> },
    Remove { node: u32 },
    Meta { center_hz: f64, retuned: bool },
    Cal { params: Box<CalParams> },
    Recalibrate,
}

/// The cross-lane home the per-lane `DspCommand` queues deliberately are not.
///
/// One OS thread per device set pops the largest range every lane covers, solves and applies the
/// calibration once for everyone, and hands the result to whatever coherent processors are
/// running. Same rules as the per-lane dsp threads: no locks, no allocation, no async.
pub struct CoherentRuntime {
    cmd_tx: mpsc::Sender<CoherentCommand>,
    stop: Arc<AtomicBool>,
    armed: Arc<AtomicBool>,
    realignments: Arc<AtomicU64>,
    thread: Option<JoinHandle<CoherentTaps>>,
}

pub(crate) struct CoherentStart {
    pub(crate) set: u32,
    pub(crate) taps: CoherentTaps,
    pub(crate) tier: Coherence,
    pub(crate) center_hz: f64,
    pub(crate) cal: CalParams,
}

impl CoherentRuntime {
    pub(crate) fn start(start: CoherentStart) -> Result<Self, DeviceError> {
        let CoherentStart {
            set,
            mut taps,
            tier,
            center_hz,
            cal,
        } = start;
        taps.rewind();
        let lanes = taps.feeds.len();
        let sample_rate = taps.sample_rate;
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
                let calibrator = cal::Calibrator::new(lanes, tier, cal, sample_rate);
                aggregate(
                    Aligner::new(taps),
                    calibrator,
                    &cmd_rx,
                    &halt,
                    &counted,
                    center_hz,
                )
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

/// Sums the lanes into one, pointed wherever the last steering said.
///
/// Nothing downstream knows: the beam arrives in an ordinary capture ring with an ordinary dsp
/// thread on it, so a channel, a recorder or a spectrum subscription works on the array's
/// combined output exactly as it works on one antenna.
struct Beam {
    sink: BeamSink,
    weights: Vec<Complex<f32>>,
    summed: Vec<Complex<f32>>,
}

impl Beam {
    fn new(sink: BeamSink) -> Self {
        Self {
            sink,
            weights: Vec::new(),
            summed: Vec::new(),
        }
    }

    fn steer(&mut self, weights: Vec<Complex<f32>>) {
        self.weights = weights;
    }

    fn sum(&mut self, lanes: &[&[Complex<f32>]], count: usize) {
        if self.weights.is_empty() || lanes.is_empty() {
            return;
        }
        self.summed.clear();
        self.summed.resize(count, Complex::new(0.0, 0.0));
        for (lane, weight) in lanes.iter().zip(&self.weights) {
            if *weight == Complex::new(0.0, 0.0) {
                continue;
            }
            for (slot, sample) in self.summed.iter_mut().zip(lane.iter().take(count)) {
                *slot += sample * weight;
            }
        }
        self.sink.push(&self.summed);
    }
}

fn aggregate(
    mut aligner: Aligner,
    mut calibrator: cal::Calibrator,
    commands: &mpsc::Receiver<CoherentCommand>,
    stop: &AtomicBool,
    realignments: &AtomicU64,
    center_hz: f64,
) -> CoherentTaps {
    let mut sinks: CoherentSinkList = Vec::new();
    let mut seen = 0u64;
    let mut center = center_hz;
    let sample_rate = aligner.sample_rate();
    let mut beam = aligner.take_beam().map(Beam::new);
    while !stop.load(Ordering::Acquire) {
        drain_commands(
            commands,
            &mut sinks,
            &mut center,
            &mut calibrator,
            sample_rate,
        );
        let Some(count) = aligner.next() else {
            std::thread::park_timeout(IDLE_PARK);
            continue;
        };
        let realigned = aligner.realignments() != seen;
        seen = aligner.realignments();
        realignments.store(seen, Ordering::Relaxed);
        aligner.with_lanes(count, |lanes| calibrator.process(lanes));
        if sinks.is_empty() {
            continue;
        }
        let ctx = AlignedContext {
            index: aligner.index(),
            sample_rate,
            center_hz: center,
            realigned,
            cal: calibrator.state(),
        };
        calibrator.with_lanes(count, |lanes| {
            for host in &mut sinks {
                host.process(lanes, ctx);
            }
        });
        if let Some(beam) = beam.as_mut() {
            for host in &mut sinks {
                if let Some(weights) = host.take_weights() {
                    beam.steer(weights);
                }
            }
            calibrator.with_lanes(count, |lanes| beam.sum(lanes, count));
        }
    }
    let mut taps = aligner.release();
    taps.beam = beam.map(|beam| beam.sink);
    taps
}

fn drain_commands(
    commands: &mpsc::Receiver<CoherentCommand>,
    sinks: &mut CoherentSinkList,
    center: &mut f64,
    calibrator: &mut cal::Calibrator,
    sample_rate: f64,
) {
    while let Ok(command) = commands.try_recv() {
        match command {
            CoherentCommand::Add { node, host } => {
                sinks.retain(|existing| existing.node() != node);
                sinks.push(host);
            }
            CoherentCommand::Remove { node } => sinks.retain(|existing| existing.node() != node),
            CoherentCommand::Meta { center_hz, retuned } => {
                *center = center_hz;
                if retuned {
                    calibrator.invalidate(true);
                }
            }
            CoherentCommand::Cal { params } => calibrator.apply(*params, sample_rate),
            CoherentCommand::Recalibrate => calibrator.invalidate(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, time::Duration};

    use sdrmm_channels::coherent::CoherentCtx;
    use sdrmm_wire::{CoherentParams, DfAlgorithm, DfParams};
    use tokio::sync::broadcast;

    use super::*;
    use crate::runtime::DecodedSink;

    static SERIAL: Mutex<()> = Mutex::new(());

    const RATE: f64 = 1_024_000.0;
    const BLOCK: usize = 4_096;

    fn sinks() -> (CoherentSinks, broadcast::Receiver<CoherentUpdate>) {
        let (updates, update_rx) = broadcast::channel(256);
        let (surfaces, _) = broadcast::channel(8);
        (
            CoherentSinks {
                updates,
                surfaces,
                decoded: DecodedSink::null(),
            },
            update_rx,
        )
    }

    fn steered(bearing_deg: f64, round: u64, lane_phase: f32) -> Vec<Vec<Complex<f32>>> {
        let elements = sdrmm_dsp::steering::uca(0.35, 4);
        let wavelength = sdrmm_dsp::steering::LIGHT_SPEED_M_S / 300e6;
        (0..4)
            .map(|lane| {
                let steer = Complex::from_polar(
                    1.0f32,
                    (std::f64::consts::TAU * elements[lane].projected(bearing_deg) / wavelength)
                        as f32
                        + lane as f32 * lane_phase,
                );
                (0..BLOCK)
                    .map(|k| {
                        let index = round * BLOCK as u64 + k as u64;
                        let phase =
                            std::f32::consts::TAU * 25_000.0 * (index % 1_024) as f32 / RATE as f32;
                        Complex::from_polar(1.0f32, phase) * steer
                    })
                    .collect()
            })
            .collect()
    }

    fn df_params(algorithm: DfAlgorithm) -> CoherentParams {
        CoherentParams::Df(DfParams {
            report_ms: 100,
            offset_hz: 25_000.0,
            bandwidth_hz: 20_000.0,
            algorithm,
            ..DfParams::default()
        })
    }

    fn ctx() -> CoherentCtx {
        CoherentCtx {
            lanes: 4,
            sample_rate: RATE,
            center_hz: 300e6,
        }
    }

    #[test]
    fn a_bearing_reaches_the_update_channel_through_the_whole_stack() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut taps, shared) = lane_taps(4, RATE);
        let runtime = CoherentRuntime::start(CoherentStart {
            set: 0,
            taps: shared,
            tier: Coherence::PhaseCoherent,
            center_hz: 300e6,
            cal: CalParams::default(),
        })
        .expect("start");
        let (sinks, mut updates) = sinks();
        let host = CoherentHost::build(
            1,
            ctx(),
            &df_params(DfAlgorithm::Music),
            sinks,
            vec![0, 1, 2, 3],
        )
        .expect("host builds");
        runtime.send(CoherentCommand::Add { node: 1, host });

        let mut found = None;
        for round in 0..120u64 {
            let block = steered(137.0, round, 0.0);
            for (tap, lane) in taps.iter_mut().zip(&block) {
                tap.push(lane, round * BLOCK as u64);
            }
            std::thread::sleep(Duration::from_millis(2));
            while let Ok(update) = updates.try_recv() {
                if let Some(reading) = update.reading {
                    found = Some(reading);
                }
            }
            if found.is_some() {
                break;
            }
        }
        let reading = found.expect("a bearing came out of the aggregator");
        let error = (f64::from(reading.bearing_deg) - 137.0).abs();
        assert!(error.min(360.0 - error) < 3.0, "read {reading:?}");
        assert!(runtime.stop().is_some());
    }

    #[test]
    fn a_time_synced_array_without_a_pilot_reports_no_bearing_at_all() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut taps, shared) = lane_taps(4, RATE);
        let runtime = CoherentRuntime::start(CoherentStart {
            set: 1,
            taps: shared,
            tier: Coherence::TimeSync,
            center_hz: 300e6,
            cal: CalParams::default(),
        })
        .expect("start");
        let (sinks, mut updates) = sinks();
        let host = CoherentHost::build(
            2,
            ctx(),
            &df_params(DfAlgorithm::Music),
            sinks,
            vec![0, 1, 2, 3],
        )
        .expect("host builds");
        runtime.send(CoherentCommand::Add { node: 2, host });

        let mut saw_state = false;
        for round in 0..200u64 {
            let block = steered(137.0, round, 1.1);
            for (tap, lane) in taps.iter_mut().zip(&block) {
                tap.push(lane, round * BLOCK as u64);
            }
            std::thread::sleep(Duration::from_millis(1));
            while let Ok(update) = updates.try_recv() {
                assert!(update.reading.is_none(), "a bearing was reported anyway");
                assert!(update.cal.phase_unknown, "{:?}", update.cal);
                saw_state = true;
            }
            if saw_state {
                break;
            }
        }
        assert!(saw_state, "the unknown-phase state was never published");
        assert!(runtime.stop().is_some());
    }
}
