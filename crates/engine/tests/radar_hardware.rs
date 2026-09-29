#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[cfg(feature = "rtlsdr")]
mod kraken;

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use common::array::{
    Bench, FULL_RATE, KRAKEN, LOCK_WAIT, WAIT, kraken_array, processor, wait_calibrated,
};
use num_complex::Complex;
use sdrmm_channels::passive_radar::{
    CafBackend, CpiJob, CpiStage, CpuCaf, FrontStage, InLanes, JobSink, RadarCtx,
    RadarPlan as StagePlan, RadarPod, plan,
};
use sdrmm_dsp::manifold::{Geometry, Winding};
use sdrmm_engine::{ArrayEvent, Engine};
use sdrmm_wire::{
    ArrayGain, ArrayTune, ProcessorParams, ProcessorReading,
    radar::{GpuUse, Illuminator, PassiveRadarParams, RadarUpdate, ReferenceCleaning},
};
use tokio::sync::broadcast::{self, error::TryRecvError};

type C32 = Complex<f32>;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const ELEMENTS: usize = 5;
const SECONDS: f64 = 3.0;
const CHUNK: usize = 16_384;
const TIMED_CPIS: usize = 4;
const PIPELINE_WARMUP: usize = 2;
const PIPELINE_CPIS: usize = 10;
const FM_HZ: f64 = 100e6;
const DAB_HZ: f64 = 220e6;
const POLL: Duration = Duration::from_millis(5);
const LIVE_RUN: Duration = Duration::from_secs(20);
const LIVE_SUPPRESSION_DB: f32 = 20.0;
const LIVE_GAIN_DB: f64 = 30.0;

#[derive(Clone, Copy, Debug)]
struct Targets {
    single_ms: Option<f64>,
    crew_ms: Option<f64>,
    auto_ms: Option<f64>,
    gpu: bool,
    front_load: f64,
}

fn targets(dab: bool) -> Option<Targets> {
    let board = std::env::var("SDRMM_BENCH_ASSERT").ok()?;
    match (board.as_str(), dab) {
        ("mac", false) => Some(Targets {
            single_ms: Some(25.0),
            crew_ms: None,
            auto_ms: None,
            gpu: false,
            front_load: 0.10,
        }),
        ("mac", true) => Some(Targets {
            single_ms: Some(120.0),
            crew_ms: Some(50.0),
            auto_ms: Some(30.0),
            gpu: true,
            front_load: 0.15,
        }),
        ("pi5", false) => Some(Targets {
            single_ms: Some(100.0),
            crew_ms: None,
            auto_ms: None,
            gpu: false,
            front_load: 0.40,
        }),
        ("pi5", true) => Some(Targets {
            single_ms: None,
            crew_ms: Some(350.0),
            auto_ms: None,
            gpu: false,
            front_load: 0.60,
        }),
        (other, _) => panic!("SDRMM_BENCH_ASSERT is mac or pi5, got {other}"),
    }
}

fn xorshift(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed | 1;
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}

struct Keep {
    free: Vec<Arc<CpiJob>>,
    ready: Vec<Arc<CpiJob>>,
}

impl JobSink for Keep {
    fn take(&mut self) -> Option<Arc<CpiJob>> {
        self.free.pop()
    }

    fn submit(&mut self, job: Arc<CpiJob>) {
        self.ready.push(job);
    }

    fn dropped(&mut self, unused: Option<Arc<CpiJob>>) {
        self.free.extend(unused);
    }
}

fn params(dab: bool, gpu: GpuUse) -> PassiveRadarParams {
    if dab {
        PassiveRadarParams {
            illuminator: Illuminator::Dab,
            reference: ReferenceCleaning::DabRemod,
            gpu,
            ..PassiveRadarParams::default()
        }
    } else {
        PassiveRadarParams {
            gpu,
            ..PassiveRadarParams::default()
        }
    }
}

fn kraken_stage(center_hz: f64, params: &PassiveRadarParams) -> StagePlan {
    let geometry = Geometry::uca(0.35, ELEMENTS, 0.0, Winding::Clockwise).expect("geometry");
    let positions_m = geometry
        .positions()
        .iter()
        .map(|p| [p.x, p.y, p.z])
        .collect();
    let ctx = RadarCtx {
        sample_rate: FULL_RATE,
        center_hz,
        elements: ELEMENTS,
        positions_m,
        tuned_together: true,
    };
    plan(&ctx, params).expect("plan")
}

fn fm_reference(len: usize) -> Vec<C32> {
    let mut uniform = xorshift(17);
    let mut phase = 0.0f64;
    let mut drive = 0.0f64;
    (0..len)
        .map(|_| {
            drive = 0.995 * drive + 0.02 * (uniform() - 0.5);
            phase += drive;
            C32::from_polar(1.0, phase as f32)
        })
        .collect()
}

fn dab_reference(len: usize) -> Vec<C32> {
    let native = sdrmm_channels::testgen::dab::ensemble(12);
    let step = 2_048_000.0 / FULL_RATE;
    (0..len)
        .map(|n| {
            let at = n as f64 * step;
            let low = at.floor() as usize % native.len();
            let high = (low + 1) % native.len();
            let fraction = (at - at.floor()) as f32;
            native[low] * (1.0 - fraction) + native[high] * fraction
        })
        .collect()
}

fn lanes_from(reference: Vec<C32>) -> Vec<Vec<C32>> {
    let mut uniform = xorshift(29);
    let mut lanes = Vec::with_capacity(ELEMENTS);
    for element in 1..ELEMENTS {
        let gain = C32::from_polar(0.8, element as f32);
        lanes.push(
            reference
                .iter()
                .map(|x| {
                    x * gain + C32::new((uniform() - 0.5) as f32, (uniform() - 0.5) as f32) * 0.05
                })
                .collect(),
        );
    }
    lanes.insert(0, reference);
    lanes
}

fn front_load(stage: &StagePlan, lanes: &[Vec<C32>]) -> (f64, Vec<Arc<CpiJob>>) {
    let mut front = FrontStage::new(stage).expect("front");
    let window = stage.shape.window();
    let mut sink = Keep {
        free: (0..32)
            .map(|_| Arc::new(CpiJob::new(ELEMENTS, window)))
            .collect(),
        ready: Vec::new(),
    };
    let len = lanes[0].len();
    let mut busy = 0.0;
    for start in (0..len).step_by(CHUNK) {
        let end = (start + CHUNK).min(len);
        let views: Vec<&[C32]> = lanes.iter().map(|lane| &lane[start..end]).collect();
        let input = InLanes {
            lanes: &views,
            first_index: start as u64,
            unix_ns: 0,
            gap_before: false,
            phase_ready: true,
            generation: 0,
        };
        busy += front.push(&input, &mut sink).busy_ns as f64 * 1e-9;
    }
    (busy / (len as f64 / FULL_RATE), sink.ready)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

fn single_ms(stage: &StagePlan, jobs: &[Arc<CpiJob>]) -> f64 {
    let backend: Box<dyn CafBackend> = Box::new(CpuCaf::new(stage).expect("cpu"));
    let mut cpi = CpiStage::new(stage, backend).expect("CPI stage");
    let mut pod = RadarPod::new(stage);
    let mut times: Vec<f64> = jobs
        .iter()
        .cycle()
        .take(TIMED_CPIS + 1)
        .map(|job| {
            let started = Instant::now();
            let outcome = cpi.run_shared(job, &mut pod);
            assert!(outcome.failed.is_none());
            started.elapsed().as_secs_f64() * 1_000.0
        })
        .skip(1)
        .collect();
    median(&mut times)
}

fn radar_updates(
    events: &mut broadcast::Receiver<ArrayEvent>,
    wanted: usize,
    timeout: Duration,
) -> Vec<RadarUpdate> {
    let started = Instant::now();
    let mut updates = Vec::with_capacity(wanted);
    while updates.len() < wanted {
        assert!(
            started.elapsed() < timeout,
            "{} of {wanted} radar updates",
            updates.len()
        );
        match events.try_recv() {
            Ok(ArrayEvent::Report { processor, reading }) if processor == "radar" => {
                if let ProcessorReading::PassiveRadar(update) = reading.as_ref() {
                    updates.push(update.clone());
                }
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(POLL),
            Err(TryRecvError::Closed) => panic!("the array events closed"),
        }
    }
    updates
}

struct Pipeline {
    compute_ms: f64,
    front_load: f64,
    gpu: bool,
    threads: u32,
    dropped_cpis: u64,
}

fn start_radar(engine: &Arc<Engine>, params: PassiveRadarParams) {
    engine
        .apply_processor(processor("radar", ProcessorParams::PassiveRadar(params)))
        .unwrap();
}

fn pipeline(dab: bool, gpu: GpuUse) -> Pipeline {
    let bench = Bench::new();
    let ds = bench.open_at(KRAKEN, FULL_RATE);
    let mut spec = kraken_array(ds);
    spec.tune = Some(ArrayTune {
        center_hz: if dab { DAB_HZ } else { FM_HZ },
        gain: ArrayGain::default(),
    });
    bench.engine.apply_array(spec).unwrap();
    let mut events = bench.engine.subscribe_arrays();
    start_radar(&bench.engine, params(dab, gpu));
    wait_calibrated(&bench.engine, LOCK_WAIT);
    let updates = radar_updates(&mut events, PIPELINE_WARMUP + PIPELINE_CPIS, 4 * WAIT);
    let timed = &updates[PIPELINE_WARMUP..];
    let mut compute: Vec<f64> = timed
        .iter()
        .map(|update| f64::from(update.health.compute_ms))
        .collect();
    let mut front: Vec<f64> = timed
        .iter()
        .map(|update| f64::from(update.health.front_load))
        .collect();
    let last = &timed[timed.len() - 1].health;
    Pipeline {
        compute_ms: median(&mut compute),
        front_load: median(&mut front),
        gpu: last.gpu,
        threads: last.threads,
        dropped_cpis: last.dropped_cpis - timed[0].health.dropped_cpis,
    }
}

fn benchmark(name: &str, dab: bool, reference: Vec<C32>) {
    let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let center = if dab { DAB_HZ } else { FM_HZ };
    let stage = kraken_stage(center, &params(dab, GpuUse::Off));
    let lanes = lanes_from(reference);
    let (front, jobs) = front_load(&stage, &lanes);
    assert!(!jobs.is_empty(), "{name}: no CPI assembled");
    let single = single_ms(&stage, &jobs);
    let crew = pipeline(dab, GpuUse::Off);
    let auto = pipeline(dab, GpuUse::Auto);
    println!(
        "{name}: front load {front:.3}, CPI {single:.1} ms single, {:.1} ms crew of {} threads (front {:.3}), {:.1} ms auto on {} (front {:.3})",
        crew.compute_ms,
        crew.threads,
        crew.front_load,
        auto.compute_ms,
        if auto.gpu { "GPU" } else { "CPU" },
        auto.front_load
    );
    assert_eq!(crew.dropped_cpis, 0, "{name}: the crew dropped CPIs");
    assert_eq!(auto.dropped_cpis, 0, "{name}: auto dropped CPIs");
    let Some(targets) = targets(dab) else {
        return;
    };
    assert!(front <= targets.front_load, "{name}: front load {front:.3}");
    assert!(
        auto.gpu || !targets.gpu,
        "{name}: Auto stayed on the CPU, build with gpu-fft on a machine with a GPU"
    );
    if let Some(limit) = targets.single_ms {
        assert!(single <= limit, "{name}: {single:.1} ms single");
    }
    if let Some(limit) = targets.crew_ms {
        assert!(
            crew.compute_ms <= limit,
            "{name}: {:.1} ms crew",
            crew.compute_ms
        );
    }
    if let Some(limit) = targets.auto_ms {
        assert!(
            auto.compute_ms <= limit,
            "{name}: {:.1} ms on the GPU",
            auto.compute_ms
        );
    }
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_fm_kraken() {
    let len = (SECONDS * FULL_RATE) as usize;
    benchmark("FM", false, fm_reference(len));
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_dab_kraken() {
    let len = (SECONDS * FULL_RATE) as usize;
    benchmark("DAB", true, dab_reference(len));
}

#[cfg(feature = "rtlsdr")]
fn live_rows(updates: &[RadarUpdate]) -> Vec<String> {
    updates
        .iter()
        .map(|update| {
            let health = &update.health;
            let suppression: Vec<String> = health
                .suppression_db
                .iter()
                .map(|db| format!("{db:.1}"))
                .collect();
            format!(
                "{},{:.3},{:.3},{:.1},{},{},{:.1},{}",
                update.seq,
                health.load,
                health.front_load,
                health.compute_ms,
                suppression.join(" "),
                health.dropped_cpis,
                health.reference.quality_db,
                update.detections.len()
            )
        })
        .collect()
}

#[cfg(feature = "rtlsdr")]
#[test]
#[ignore = "needs a KrakenSDR with antennas and SDRMM_RADAR_FM_HZ set to a local FM station"]
fn kraken_fm_live() {
    use common::array::status;

    let fm_hz = kraken::env_f64("SDRMM_RADAR_FM_HZ")
        .expect("kraken_fm_live needs SDRMM_RADAR_FM_HZ set to a local FM station in Hz");
    let rig = kraken::Kraken::open();
    let engine = &rig.engine;
    engine
        .apply_array(rig.array(fm_hz, LIVE_GAIN_DB, sdrmm_wire::ArrayCal::default()))
        .unwrap();
    let mut events = engine.subscribe_arrays();
    start_radar(engine, PassiveRadarParams::default());
    wait_calibrated(engine, LOCK_WAIT);
    common::array::drain(&mut events);
    let cpis =
        (LIVE_RUN.as_secs_f64() * 1e3 / f64::from(PassiveRadarParams::default().cpi_ms)) as usize;
    let updates = radar_updates(&mut events, cpis, LIVE_RUN + WAIT);
    let levels: Vec<String> = status(engine)
        .lanes
        .iter()
        .map(|lane| format!("{:.1}", lane.level_dbfs))
        .collect();
    kraken::write_csv(
        "kraken_fm_live",
        &format!(
            "seq,load,front_load,compute_ms,suppression_db,dropped_cpis,reference_quality_db,detections (lane levels {} dBFS at {fm_hz} Hz)",
            levels.join(" ")
        ),
        &live_rows(&updates),
    );
    let first = &updates[0].health;
    let last = &updates[updates.len() - 1].health;
    let load = updates
        .iter()
        .map(|update| update.health.load)
        .fold(0.0, f32::max);
    assert!(load < 1.0, "load {load}");
    assert_eq!(last.dropped_cpis, first.dropped_cpis, "dropped CPIs");
    for (lane, db) in last.suppression_db.iter().enumerate() {
        assert!(
            *db > LIVE_SUPPRESSION_DB,
            "lane {lane} suppression {db:.1} dB; lane levels {levels:?} dBFS: is an antenna on every input and {fm_hz} Hz a strong local station?"
        );
    }
}
