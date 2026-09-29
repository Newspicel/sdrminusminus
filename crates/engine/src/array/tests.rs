use std::{
    f64::consts::TAU,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::{ArrayCal, ArrayCalSource, ArrayGain, ArrayTune, CalPhase, Coherence, SyncState};
use tokio::sync::broadcast;

use super::*;
use crate::array::host::tests::frame;

struct Quiet;

impl ArrayControl for Quiet {
    fn switch_array_noise(&self, _node: &str, _on: bool) -> Result<(), EngineError> {
        Ok(())
    }

    fn tune_array_internal(&self, _node: &str, _tune: ArrayTune) -> Result<(), EngineError> {
        Ok(())
    }
}

#[test]
fn an_array_runtime_starts_its_threads_and_joins_them_on_stop() {
    let quiet: Arc<dyn ArrayControl> = Arc::new(Quiet);
    let (setup, _writers, _ports, board) = setup(2, Arc::downgrade(&quiet), ArrayCalSource::Noise);
    let runtime = ArrayRuntime::start(setup).expect("a running array");
    assert!(!runtime.is_finished());
    runtime
        .control(ControlCommand::NoiseSwitch(None))
        .expect("the controller listens");
    let exit = runtime.stop().expect("the aggregator hands its feeds back");
    assert_eq!(exit.feeds.len(), 2);
    assert!(board.alive());
}

struct Uniform(u64);

impl Uniform {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64 - 0.5)
            as f32
    }

    fn block(&mut self, len: usize, amplitude: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|_| Complex::new(self.next(), self.next()) * amplitude)
            .collect()
    }
}

fn through_hardware(
    len: usize,
    seed: u64,
    amplitude: f32,
    lanes: &[(f64, f64, f64)],
) -> Vec<Vec<Complex<f32>>> {
    let mut source = Uniform(seed).block(len, amplitude);
    let mut fft = FftPair::new(len);
    fft.forward(&mut source);
    lanes
        .iter()
        .map(|&(delay, phase_deg, gain_db)| {
            let mut lane = source.clone();
            for (bin, value) in lane.iter_mut().enumerate() {
                let nu = if bin < len / 2 {
                    bin as f64 / len as f64
                } else {
                    (bin as f64 - len as f64) / len as f64
                };
                let response = Complex::from_polar(
                    10f64.powf(gain_db / 20.0),
                    phase_deg.to_radians() - TAU * nu * delay,
                );
                *value *= Complex::new(response.re as f32, response.im as f32);
            }
            fft.inverse_scaled(&mut lane);
            lane
        })
        .collect()
}

fn setup(
    lanes: usize,
    control: Weak<dyn ArrayControl>,
    source: ArrayCalSource,
) -> (
    RuntimeSetup,
    Vec<TapWriter>,
    Vec<Arc<TapPort>>,
    Arc<StatusBoard>,
) {
    let mut feeds = Vec::new();
    let mut ports = Vec::new();
    let mut writers = Vec::new();
    for stream in 0..lanes as u32 {
        let (port, writer) = TapPort::new(stream);
        feeds.push(Some(port.lease(RATE, 1).expect("lease")));
        ports.push(port);
        writers.push(writer);
    }
    let board = Arc::new(StatusBoard::new(lanes));
    let setup = RuntimeSetup {
        node: "array-1".to_owned(),
        feeds,
        frame: frame(lanes),
        board: board.clone(),
        control,
        config: ControlConfig {
            cal: ArrayCal {
                source,
                check_s: 0,
                equaliser: false,
                warm_start: false,
            },
            gain: ArrayGain::default(),
            needs_time: true,
            needs_phase: true,
            tier: TierDecision {
                tier: Coherence::TimeSync,
                devices: 1,
                keeps_phase: false,
                structural_zero_delay: false,
            },
            sample_rate: RATE,
        },
        events: broadcast::channel(16).0,
    };
    (setup, writers, ports, board)
}

const RATE: f64 = 48_000.0;
const BLOCK: usize = 4_096;

fn stream(writers: &mut [TapWriter], lanes: &[Vec<Complex<f32>>], index: &mut u64) {
    let at = (*index as usize) % (lanes[0].len() - BLOCK);
    for (writer, lane) in writers.iter_mut().zip(lanes) {
        writer.samples(&lane[at..at + BLOCK], *index);
    }
    *index += BLOCK as u64;
}

#[test]
fn a_live_array_syncs_through_its_threads() {
    let quiet: Arc<dyn ArrayControl> = Arc::new(Quiet);
    let (setup, mut writers, _ports, board) = setup(2, Arc::downgrade(&quiet), ArrayCalSource::Off);
    let runtime = ArrayRuntime::start(setup).expect("a running array");
    let pair = through_hardware(1 << 19, 3, 0.2, &[(0.0, 0.0, 0.0), (37.3, 0.0, 0.0)]);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut index = 0u64;
    while board.sync() != SyncState::Locked {
        assert!(Instant::now() < deadline, "the array never locked");
        stream(&mut writers, &pair, &mut index);
        std::thread::sleep(Duration::from_millis(2));
    }
    let delay = board.lanes[1].delay_milli.load(Ordering::Relaxed);
    assert!((delay - 37_300).abs() <= 50, "{delay}");
    assert_eq!(board.cal(), CalPhase::None);
    drop(runtime.stop());
}

#[derive(Default)]
struct Bench {
    on: AtomicBool,
    flips: std::sync::atomic::AtomicU32,
}

impl ArrayControl for Bench {
    fn switch_array_noise(&self, _node: &str, on: bool) -> Result<(), EngineError> {
        self.on.store(on, Ordering::SeqCst);
        self.flips.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn tune_array_internal(&self, _node: &str, _tune: ArrayTune) -> Result<(), EngineError> {
        Ok(())
    }
}

#[test]
fn a_noise_burst_calibrates_through_its_threads() {
    let bench = Arc::new(Bench::default());
    let control: Arc<dyn ArrayControl> = bench.clone();
    let (setup, mut writers, _ports, board) =
        setup(3, Arc::downgrade(&control), ArrayCalSource::Noise);
    let len = 1 << 19;
    let truths = [(0.0, 0.0, 0.0), (37.3, 50.0, -2.0), (-5.6, -100.0, 1.0)];
    let mut noise = through_hardware(len, 5, 0.3, &truths);
    let mut spread = Uniform(9);
    let live: Vec<Vec<Complex<f32>>> = (0..3).map(|_| spread.block(len, 0.01)).collect();
    for (lane, floor) in noise.iter_mut().zip(&live) {
        for (sample, extra) in lane.iter_mut().zip(floor) {
            *sample += extra;
        }
    }
    let mut index = 0u64;
    for _ in 0..8 {
        stream(&mut writers, &live, &mut index);
    }
    let runtime = ArrayRuntime::start(setup).expect("a running array");
    runtime
        .control(ControlCommand::NoiseSwitch(Some(controller::NoiseSwitch {
            device_set: 1,
            kind: sdrmm_wire::NoiseSource::Isolated,
            all_lanes_held: true,
        })))
        .expect("the controller listens");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut was_on = false;
    while board.cal() != CalPhase::Solved {
        assert!(Instant::now() < deadline, "the array never calibrated");
        let on = bench.on.load(Ordering::SeqCst);
        if on != was_on {
            for writer in &mut writers {
                writer.event(sdrmm_device::LaneEvent::Mark {
                    at: index,
                    mark: sdrmm_device::LaneMark::NoiseSource { on, in_flight: 0 },
                });
            }
            was_on = on;
        }
        stream(&mut writers, if on { &noise } else { &live }, &mut index);
        std::thread::sleep(Duration::from_millis(2));
    }
    for (lane, (delay, phase, gain)) in truths.iter().enumerate().skip(1) {
        let status = &board.lanes[lane];
        let measured = status.delay_milli.load(Ordering::Relaxed) as f64 / 1e3;
        let degrees = f64::from(status.phase_mdeg.load(Ordering::Relaxed)) / 1e3;
        let db = f64::from(status.gain_mdb.load(Ordering::Relaxed)) / 1e3;
        assert!(
            (measured - delay).abs() < 0.05,
            "lane {lane} delay {measured}"
        );
        assert!((degrees - phase).abs() < 1.0, "lane {lane} phase {degrees}");
        assert!((db - gain).abs() < 0.1, "lane {lane} gain {db}");
    }
    assert!(board.phase_ready());
    assert_eq!(board.sync(), SyncState::Locked);
    assert!(!bench.on.load(Ordering::SeqCst));
    assert_eq!(bench.flips.load(Ordering::SeqCst), 2);
    drop(runtime.stop());
}

fn crosses_threads<T: Send>() {}

fn shared_between_threads<T: Send + Sync>() {}

#[test]
fn runtime_parts_cross_threads() {
    crosses_threads::<ArrayRuntime>();
    crosses_threads::<Command>();
    crosses_threads::<Retired>();
    crosses_threads::<ProcessorHost>();
    crosses_threads::<LaneFeed>();
    crosses_threads::<TapWriter>();
    shared_between_threads::<ArrayEvent>();
    shared_between_threads::<StatusBoard>();
    shared_between_threads::<TapPort>();
    shared_between_threads::<CommandQueue>();
}
