use std::{sync::Arc, time::Instant};

use num_complex::Complex;
use sdrmm_channels::passive_radar::{
    CafBackend, CpiJob, CpiStage, CpuCaf, FrontStage, InLanes, JobSink, RadarCtx,
    RadarPlan as StagePlan, RadarPod, plan,
};
use sdrmm_dsp::manifold::{Geometry, Winding};
use sdrmm_wire::radar::{Illuminator, PassiveRadarParams, ReferenceCleaning};

use super::{super::crew::CrewCaf, Shared, xorshift};

type C32 = Complex<f32>;

const KRAKEN_RATE: f64 = 2_400_000.0;
const ELEMENTS: usize = 5;
const SECONDS: f64 = 3.0;
const CHUNK: usize = 16_384;
const TIMED_CPIS: usize = 4;

struct Targets {
    single_ms: Option<f64>,
    crew_ms: Option<f64>,
    front_load: f64,
}

fn targets(dab: bool) -> Option<Targets> {
    let board = std::env::var("SDRMM_BENCH_ASSERT").ok()?;
    match (board.as_str(), dab) {
        ("mac", false) => Some(Targets {
            single_ms: Some(25.0),
            crew_ms: None,
            front_load: 0.10,
        }),
        ("mac", true) => Some(Targets {
            single_ms: Some(120.0),
            crew_ms: Some(50.0),
            front_load: 0.15,
        }),
        ("pi5", false) => Some(Targets {
            single_ms: Some(100.0),
            crew_ms: None,
            front_load: 0.40,
        }),
        ("pi5", true) => Some(Targets {
            single_ms: None,
            crew_ms: Some(350.0),
            front_load: 0.60,
        }),
        _ => None,
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

fn kraken_stage(center_hz: f64, params: &PassiveRadarParams) -> StagePlan {
    let geometry = Geometry::uca(0.35, ELEMENTS, 0.0, Winding::Clockwise).expect("geometry");
    let positions_m = geometry
        .positions()
        .iter()
        .map(|p| [p.x, p.y, p.z])
        .collect();
    let ctx = RadarCtx {
        sample_rate: KRAKEN_RATE,
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
    let step = 2_048_000.0 / KRAKEN_RATE;
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
    (busy / (len as f64 / KRAKEN_RATE), sink.ready)
}

fn cpi_ms(stage: &StagePlan, backend: Box<dyn CafBackend>, jobs: &[Arc<CpiJob>]) -> f64 {
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
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn benchmark(name: &str, stage: &StagePlan, reference: Vec<C32>, dab: bool) {
    let lanes = lanes_from(reference);
    let (front, jobs) = front_load(stage, &lanes);
    assert!(!jobs.is_empty(), "{name}: no CPI assembled");
    let single = cpi_ms(stage, Box::new(CpuCaf::new(stage).expect("cpu")), &jobs);
    let helpers = super::super::crew_helpers();
    let crew = (helpers > 0).then(|| {
        let shared = Arc::new(Shared::default());
        let backend = CrewCaf::new(stage, helpers, &shared).expect("crew");
        cpi_ms(stage, Box::new(backend), &jobs)
    });
    let crew_text = crew.map_or_else(
        || "no crew".to_owned(),
        |ms| format!("{ms:.1} ms with {helpers} helpers"),
    );
    println!("{name}: front load {front:.3}, CPI {single:.1} ms single, {crew_text}");
    let Some(targets) = targets(dab) else {
        return;
    };
    assert!(front <= targets.front_load, "{name}: front load {front:.3}");
    if let Some(limit) = targets.single_ms {
        assert!(single <= limit, "{name}: {single:.1} ms single");
    }
    if let (Some(limit), Some(crew)) = (targets.crew_ms, crew) {
        assert!(crew <= limit, "{name}: {crew:.1} ms crew");
    }
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_fm_kraken() {
    let stage = kraken_stage(100e6, &PassiveRadarParams::default());
    let len = (SECONDS * KRAKEN_RATE) as usize;
    benchmark("FM", &stage, fm_reference(len), false);
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_dab_kraken() {
    let params = PassiveRadarParams {
        illuminator: Illuminator::Dab,
        reference: ReferenceCleaning::DabRemod,
        ..PassiveRadarParams::default()
    };
    let stage = kraken_stage(220e6, &params);
    let len = (SECONDS * KRAKEN_RATE) as usize;
    benchmark("DAB", &stage, dab_reference(len), true);
}
