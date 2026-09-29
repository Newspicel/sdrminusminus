use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use num_complex::Complex;
use sdrmm_dsp::manifold::{Direction, Geometry, Vec3};
use sdrmm_dsp::scene::{ArrayScene, SceneEcho, SceneSignal, SceneSource};
use sdrmm_wire::frame::{SurfaceFrame, VisibilityOwned};
use sdrmm_wire::radar::{
    AoaState, CfarParams, ClutterMethod, ClutterParams, Illuminator, LIGHT_SPEED_M_S,
    MAX_ECA_ORDER, PassiveRadarParams, RadarUpdate, ReferenceCleaning, TrackChange,
};
use sdrmm_wire::{ArrayGeometry, ArrayTuningMode, Coherence, DfParams, ProcessorParams, Winding};

use super::*;
use crate::ChannelError;
use crate::array_processor::ArrayCtx;

type C32 = Complex<f32>;

const INPUT_RATE: f64 = 500_000.0;
const CENTER_HZ: f64 = 100e6;
const BASE_NS: u64 = 1_780_000_000_000_000_000;
const BLOCK: usize = 16_384;
const SCENE_SAMPLES: usize = 2_010_000;
const SPLIT: usize = 1_010_000;
const TARGET_M: f64 = 42_300.0;
const TARGET_MPS: f64 = -90.0;
const TARGET_AZIMUTH_DEG: f64 = 30.0;
const TRANSMITTER_AZIMUTH_DEG: f64 = 200.0;
const KRAKEN_RATE: f64 = 2_400_000.0;
const DAB_RATE: f64 = 2_048_000.0;
const DAB_FRAME: usize = 196_608;

struct Pool {
    free: Vec<Arc<CpiJob>>,
    ready: VecDeque<Arc<CpiJob>>,
    dropped: u32,
}

impl JobSink for Pool {
    fn take(&mut self) -> Option<Arc<CpiJob>> {
        self.free.pop()
    }

    fn submit(&mut self, job: Arc<CpiJob>) {
        self.ready.push_back(job);
    }

    fn dropped(&mut self, unused: Option<Arc<CpiJob>>) {
        self.dropped += 1;
        self.free.extend(unused);
    }
}

struct Radar {
    plan: RadarPlan,
    front: FrontStage,
    cpi: CpiStage,
    pool: Pool,
    pod: Box<RadarPod>,
    update: RadarUpdate,
    updates: Vec<RadarUpdate>,
    ends_ns: Vec<u64>,
    clock_scale: f64,
}

impl Radar {
    fn new(ctx: &RadarCtx, params: &PassiveRadarParams) -> Self {
        let plan = plan(ctx, params).unwrap();
        let backend = Box::new(CpuCaf::new(&plan).unwrap());
        Self::with_backend(plan, backend)
    }

    fn with_backend(plan: RadarPlan, backend: Box<dyn CafBackend>) -> Self {
        let lanes = plan.front.lanes.len();
        let window = plan.shape.window();
        Self {
            front: FrontStage::new(&plan).unwrap(),
            cpi: CpiStage::new(&plan, backend).unwrap(),
            pool: Pool {
                free: (0..2)
                    .map(|_| Arc::new(CpiJob::new(lanes, window)))
                    .collect(),
                ready: VecDeque::with_capacity(4),
                dropped: 0,
            },
            pod: RadarPod::new(&plan),
            update: RadarUpdate::reserved(),
            updates: Vec::new(),
            ends_ns: Vec::new(),
            clock_scale: 1.0,
            plan,
        }
    }

    fn feed(&mut self, elements: &[Vec<C32>], range: std::ops::Range<usize>, phase_ready: bool) {
        let mut start = range.start;
        while start < range.end {
            let end = (start + BLOCK).min(range.end);
            let lanes: Vec<&[C32]> = self
                .plan
                .front
                .lanes
                .iter()
                .map(|&element| &elements[element][start..end])
                .collect();
            let input = InLanes {
                lanes: &lanes,
                first_index: start as u64,
                unix_ns: BASE_NS
                    + (start as f64 / self.plan.front.input_rate * 1e9 * self.clock_scale) as u64,
                gap_before: false,
                phase_ready,
                generation: 0,
            };
            let stats = self.front.push(&input, &mut self.pool);
            assert!(!stats.mismatched);
            assert_eq!(stats.overflowed_samples, 0);
            self.run_ready();
            start = end;
        }
        assert_eq!(self.pool.dropped, 0);
    }

    fn run_ready(&mut self) {
        while let Some(job) = self.pool.ready.pop_front() {
            self.cpi.run(&job, &mut self.pod);
            fill_update(&self.pod, &mut self.update);
            self.updates.push(self.update.clone());
            self.ends_ns.push(self.pod.cpi_end_unix_ns);
            self.pool.free.push(job);
        }
    }

    fn last(&self) -> &RadarUpdate {
        self.updates.last().unwrap()
    }

    fn track_id(&self) -> u32 {
        self.last().tracks.first().map(|track| track.id).unwrap()
    }
}

fn surveillance_array() -> Geometry {
    let radius = 0.4 * LIGHT_SPEED_M_S / CENTER_HZ;
    let mut positions = vec![Vec3::new(0.0, 0.0, 0.0)];
    positions.extend((0..4).map(|element| {
        let (sin, cos) = (f64::from(element) * 90.0).to_radians().sin_cos();
        Vec3::new(radius * sin, radius * cos, 0.0)
    }));
    Geometry::explicit(&positions).unwrap()
}

fn scene_ctx() -> RadarCtx {
    RadarCtx {
        sample_rate: INPUT_RATE,
        center_hz: CENTER_HZ,
        elements: 5,
        positions_m: positions_of(&surveillance_array()),
        manifold: None,
        tuned_together: true,
    }
}

fn kraken_ctx(center_hz: f64) -> RadarCtx {
    let winding = sdrmm_dsp::manifold::Winding::Clockwise;
    let geometry = Geometry::uca(0.35, 5, 0.0, winding).unwrap();
    RadarCtx {
        sample_rate: KRAKEN_RATE,
        center_hz,
        elements: 5,
        positions_m: positions_of(&geometry),
        manifold: None,
        tuned_together: true,
    }
}

fn dab_params() -> PassiveRadarParams {
    PassiveRadarParams {
        illuminator: Illuminator::Dab,
        reference: ReferenceCleaning::DabRemod,
        ..PassiveRadarParams::default()
    }
}

fn illuminator() -> SceneSource {
    let signal = SceneSignal::NoiseFm {
        offset_hz: 0.0,
        deviation_hz: 25e3,
        bandwidth_hz: 15e3,
    };
    SceneSource::new(Direction::horizon(TRANSMITTER_AZIMUTH_DEG), 40.0, signal)
}

fn reference_beam(element: usize, azimuth_deg: f64) -> C32 {
    let off_axis = angle_error(azimuth_deg, TRANSMITTER_AZIMUTH_DEG).abs();
    if element == 0 && off_axis > 30.0 {
        C32::new(0.03, 0.0)
    } else {
        C32::new(1.0, 0.0)
    }
}

fn scene(target: bool) -> ArrayScene {
    let mut scene = ArrayScene::new(surveillance_array(), CENTER_HZ, INPUT_RATE)
        .with_source(illuminator())
        .with_noise_db(0.0)
        .with_seed(17);
    scene.distortion = Some(reference_beam);
    let clutter = [
        (3.0, -15.0, 190.0),
        (8.0, -20.0, 150.0),
        (20.0, -25.0, 250.0),
    ];
    for (delay, gain_db, azimuth) in clutter {
        let echo = SceneEcho::new(0, Direction::horizon(azimuth), gain_db, delay, 0.0);
        scene = scene.with_echo(echo);
    }
    if target {
        scene = scene.with_echo(SceneEcho::bistatic(
            0,
            Direction::horizon(TARGET_AZIMUTH_DEG),
            -65.0,
            TARGET_M,
            TARGET_MPS,
            INPUT_RATE,
            CENTER_HZ,
        ));
    }
    scene
}

fn target_scene() -> &'static [Vec<C32>] {
    static SCENE: OnceLock<Vec<Vec<C32>>> = OnceLock::new();
    SCENE.get_or_init(|| scene(true).render(SCENE_SAMPLES).unwrap())
}

fn angle_error(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

fn group_sizes(plan: &RadarPlan) -> Vec<usize> {
    let groups = plan.groups.as_ref().unwrap();
    groups
        .bounds
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect()
}

#[test]
fn plan_for_fm_on_a_kraken() {
    let plan = plan(&kraken_ctx(100e6), &PassiveRadarParams::default()).unwrap();
    assert!((plan.front.radar_rate - 266_666.67).abs() < 0.01);
    assert!((plan.range_step_m - 1_124.22).abs() < 0.01);
    assert!((plan.wavelength_m - 2.9979).abs() < 1e-4);
    assert!((400.0 / plan.wavelength_m - 133.43).abs() < 0.01);
    assert_eq!((plan.shape.batches, plan.shape.batch_len), (512, 260));
    assert_eq!(plan.shape.samples(), 133_120);
    assert!((plan.cpi_s - 0.4992).abs() < 1e-9);
    assert_eq!(plan.report_rows.len(), 133);
    assert_eq!(plan.shape.gates, 73);
    assert_eq!((plan.shape.taps, plan.shape.lead), (14, 2));
    assert_eq!((plan.shape.span(), plan.shape.fft_len), (75, 512));
    let sizes = group_sizes(&plan);
    assert_eq!(sizes.len(), 10);
    assert!(
        sizes.iter().all(|&size| size == 51 || size == 52),
        "{sizes:?}"
    );
    assert_eq!(plan.front.lanes, vec![0, 1, 2, 3, 4]);
    assert_eq!(plan.front.latency, 0);
    assert_eq!(plan.hop, 133_120);
    assert!(plan.aoa.as_ref().unwrap().mirror_axis_deg.is_none());
}

#[test]
fn plan_for_dab_on_a_kraken() {
    let plan = plan(&kraken_ctx(220e6), &dab_params()).unwrap();
    assert!((plan.front.radar_rate - DAB_RATE).abs() < 1e-9);
    assert!((plan.range_step_m - 146.383).abs() < 0.001);
    assert!((plan.wavelength_m - 1.36269).abs() < 1e-5);
    assert!((400.0 / plan.wavelength_m - 293.54).abs() < 0.01);
    assert_eq!((plan.shape.batches, plan.shape.batch_len), (1024, 1000));
    assert_eq!(plan.shape.samples(), 1_024_000);
    assert!((plan.cpi_s - 0.5).abs() < 1e-12);
    assert_eq!(plan.report_rows.len(), 293);
    assert_eq!(plan.shape.gates, 548);
    assert_eq!((plan.shape.taps, plan.shape.lead), (103, 2));
    assert_eq!((plan.shape.span(), plan.shape.fft_len), (550, 2048));
    let sizes = group_sizes(&plan);
    assert_eq!(sizes.len(), 10);
    assert!(
        sizes.iter().all(|&size| size == 102 || size == 103),
        "{sizes:?}"
    );
    assert_eq!(plan.front.latency, DAB_FRAME_LATENCY);
    assert_eq!(DAB_FRAME_LATENCY, 196_608 + 2 * 2_552);
}

#[test]
fn plan_refuses_dab_below_2048k() {
    let ctx = RadarCtx {
        sample_rate: 2_000_000.0,
        ..kraken_ctx(220e6)
    };
    assert_eq!(plan(&ctx, &dab_params()), Err(PlanError::DabRate));
}

#[test]
fn plan_refuses_a_band_outside_the_capture() {
    let params = PassiveRadarParams {
        offset_hz: 1_100_000.0,
        ..PassiveRadarParams::default()
    };
    assert_eq!(plan(&kraken_ctx(100e6), &params), Err(PlanError::Band));
}

#[test]
fn plan_refuses_spread_tuning() {
    let ctx = RadarCtx {
        tuned_together: false,
        ..kraken_ctx(100e6)
    };
    let refused = plan(&ctx, &PassiveRadarParams::default());
    assert_eq!(refused, Err(PlanError::Spread));
    assert_eq!(
        ChannelError::from(PlanError::Spread).to_string(),
        "Array must be tuned together"
    );
}

#[test]
fn plan_refuses_an_order_over_512() {
    let params = PassiveRadarParams {
        clutter: ClutterParams {
            reach_km: 80.0,
            ..ClutterParams::default()
        },
        ..dab_params()
    };
    assert_eq!(plan(&kraken_ctx(220e6), &params), Err(PlanError::Order));
}

#[test]
fn wire_and_solver_order_limits_agree() {
    assert_eq!(MAX_ECA_ORDER as usize, sdrmm_dsp::linalg::MAX_SOLVE_ORDER);
}

#[test]
fn plan_refuses_a_cpi_too_large() {
    let params = PassiveRadarParams {
        cpi_ms: 2_000,
        ..dab_params()
    };
    assert_eq!(plan(&kraken_ctx(220e6), &params), Err(PlanError::Memory));
}

#[test]
fn plan_changes_are_classified() {
    let ctx = kraken_ctx(100e6);
    let base = plan(&ctx, &PassiveRadarParams::default()).unwrap();
    let with = |params: PassiveRadarParams| plan(&ctx, &params).unwrap();
    assert_eq!(change(&base, &base), PlanChange::Same);
    let cfar = with(PassiveRadarParams {
        cfar: CfarParams {
            min_snr_db: 12.0,
            ..CfarParams::default()
        },
        ..PassiveRadarParams::default()
    });
    assert_eq!(change(&base, &cfar), PlanChange::Live);
    let overlap = with(PassiveRadarParams {
        overlap: 0.5,
        aoa: false,
        ..PassiveRadarParams::default()
    });
    assert_eq!(change(&base, &overlap), PlanChange::Live);
    assert_eq!(overlap.live().hop, 66_560);
    let cpi = with(PassiveRadarParams {
        cpi_ms: 400,
        ..PassiveRadarParams::default()
    });
    assert_eq!(change(&base, &cpi), PlanChange::Rebuild);
    let moved = plan(&kraken_ctx(101e6), &PassiveRadarParams::default()).unwrap();
    assert_eq!(change(&base, &moved), PlanChange::Rebuild);
}

#[test]
fn settings_for_another_processor_are_refused() {
    let geometry = ArrayGeometry::Uca {
        radius_m: 0.35,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    };
    let centers = [100e6; 5];
    let ctx = ArrayCtx {
        node: "radar",
        lanes: 5,
        sample_rate: KRAKEN_RATE,
        center_hz: 100e6,
        lane_centers_hz: &centers,
        geometry: &geometry,
        positions_m: &[],
        manifold: None,
        tier: Coherence::PhaseCoherent,
        tuning: ArrayTuningMode::Together,
        max_block: BLOCK,
    };
    let df = ProcessorParams::Df(DfParams::default());
    assert!(matches!(
        plan_for(&ctx, &df),
        Err(ChannelError::Refused("Wrong settings"))
    ));
    let radar = ProcessorParams::PassiveRadar(PassiveRadarParams::default());
    let planned = plan_for(&ctx, &radar).unwrap();
    assert_eq!(planned.ctx.positions_m.len(), 5);
    assert!(planned.aoa.is_some());
}

#[test]
fn an_echo_is_detected_tracked_and_located_end_to_end() {
    let mut radar = Radar::new(&scene_ctx(), &PassiveRadarParams::default());
    radar.feed(target_scene(), 0..SCENE_SAMPLES, true);
    assert_eq!(radar.updates.len(), 8);
    let last = radar.last();
    assert_eq!(last.tracks.len(), 1, "{:?}", last.tracks);
    let track = &last.tracks[0];
    let range_m = f64::from(track.range_km) * 1_000.0;
    assert!((range_m - TARGET_M).abs() < 1_000.0, "{track:?}");
    assert!(
        (f64::from(track.range_rate_mps) - TARGET_MPS).abs() < 5.0,
        "{track:?}"
    );
    let aoa = track.aoa.unwrap();
    let azimuth = f64::from(aoa.azimuth_deg);
    assert!(
        angle_error(azimuth, TARGET_AZIMUTH_DEG).abs() < 5.0,
        "{aoa:?}"
    );
    assert!(aoa.mirror_deg.is_none());
    assert_eq!(last.health.aoa, AoaState::Ready);
    let health = &last.health;
    assert!(
        health.suppression_db.iter().all(|&db| db > 20.0),
        "{health:?}"
    );
    let confirmations = radar
        .updates
        .iter()
        .flat_map(|update| &update.events)
        .filter(|event| event.change == TrackChange::Confirmed)
        .count();
    assert_eq!(confirmations, 1);
    let detection = last
        .detections
        .iter()
        .find(|detection| detection.track_id == Some(track.id))
        .unwrap();
    assert!(detection.snr_db > 10.0, "{detection:?}");
    assert!(detection.aoa.is_some());
}

const SKEW_DEG: [f64; 5] = [0.0, -15.0, 25.0, 15.0, -25.0];

fn skewed_beam(element: usize, azimuth_deg: f64) -> C32 {
    let skew = SKEW_DEG[element % SKEW_DEG.len()].to_radians();
    reference_beam(element, azimuth_deg) * C32::from_polar(1.0, skew as f32)
}

fn measured_ctx(scene: &ArrayScene) -> RadarCtx {
    let table = scene
        .distortion_table(&[CENTER_HZ - 1e6, CENTER_HZ + 1e6], 1.0)
        .unwrap();
    RadarCtx {
        manifold: Some(Arc::new(table)),
        ..scene_ctx()
    }
}

fn tracked_azimuth_error(ctx: &RadarCtx, lanes: &[Vec<C32>]) -> f64 {
    let mut radar = Radar::new(ctx, &PassiveRadarParams::default());
    radar.feed(lanes, 0..SCENE_SAMPLES, true);
    let last = radar.last();
    assert_eq!(last.tracks.len(), 1, "{:?}", last.tracks);
    let aoa = last.tracks[0].aoa.unwrap();
    angle_error(f64::from(aoa.azimuth_deg), TARGET_AZIMUTH_DEG).abs()
}

#[test]
fn the_array_table_steers_radar_aoa_past_skewed_elements() {
    let mut skewed = scene(true);
    skewed.distortion = Some(skewed_beam);
    let ctx = measured_ctx(&skewed);
    let lanes = skewed.render(SCENE_SAMPLES).unwrap();
    let measured = tracked_azimuth_error(&ctx, &lanes);
    let ideal = tracked_azimuth_error(&scene_ctx(), &lanes);
    assert!(measured < 2.0, "{measured} deg off with the table");
    assert!(ideal > 6.0, "{ideal} deg off without the table");
}

#[test]
fn a_radar_plan_steers_through_the_array_table() {
    let mut skewed = scene(false);
    skewed.distortion = Some(skewed_beam);
    let params = PassiveRadarParams::default();
    let ctx = measured_ctx(&skewed);
    let measured = plan(&ctx, &params).unwrap();
    let aoa = measured.aoa.as_ref().unwrap();
    assert!(aoa.manifold.uses_table_at(measured.carrier_hz));
    let rows = ctx
        .manifold
        .as_ref()
        .unwrap()
        .select(&[1, 2, 3, 4])
        .unwrap();
    assert_eq!(aoa.manifold.table(), Some(&rows));
    let ideal = plan(&scene_ctx(), &params).unwrap();
    assert!(ideal.aoa.as_ref().unwrap().manifold.table().is_none());
    assert_eq!(change(&ideal, &measured), PlanChange::Rebuild);
    let mut three = ArrayScene::new(
        Geometry::uca(1.0, 3, 0.0, sdrmm_dsp::manifold::Winding::Clockwise).unwrap(),
        CENTER_HZ,
        INPUT_RATE,
    );
    three.distortion = Some(skewed_beam);
    let misfit = RadarCtx {
        manifold: measured_ctx(&three).manifold,
        ..scene_ctx()
    };
    assert_eq!(plan(&misfit, &params), Err(PlanError::Geometry));
    let quiet = PassiveRadarParams {
        aoa: false,
        ..params
    };
    assert!(plan(&misfit, &quiet).unwrap().aoa.is_none());
}

fn table_ctx(freqs_hz: &[f64]) -> RadarCtx {
    let mut skewed = scene(false);
    skewed.distortion = Some(skewed_beam);
    RadarCtx {
        manifold: Some(Arc::new(skewed.distortion_table(freqs_hz, 1.0).unwrap())),
        ..scene_ctx()
    }
}

#[test]
fn a_table_that_misses_the_carrier_is_flagged() {
    let lanes = ArrayScene::new(surveillance_array(), CENTER_HZ, INPUT_RATE)
        .with_noise_db(0.0)
        .with_seed(5)
        .render(260_000)
        .unwrap();
    let params = PassiveRadarParams {
        reference: ReferenceCleaning::Off,
        ..PassiveRadarParams::default()
    };
    let covering = table_ctx(&[CENTER_HZ - 1e6, CENTER_HZ + 1e6]);
    let elsewhere = table_ctx(&[CENTER_HZ + 50e6, CENTER_HZ + 52e6]);
    let blind = PassiveRadarParams {
        aoa: false,
        ..params
    };
    for (ctx, params, flagged) in [
        (&scene_ctx(), &params, false),
        (&covering, &params, false),
        (&elsewhere, &params, true),
        (&elsewhere, &blind, false),
    ] {
        let mut radar = Radar::new(ctx, params);
        radar.feed(&lanes, 0..lanes[0].len(), true);
        assert!(!radar.updates.is_empty());
        for update in &radar.updates {
            assert_eq!(update.health.table_out_of_range, flagged);
        }
    }
    let planned = plan(&elsewhere, &params).unwrap();
    let aoa = planned.aoa.unwrap();
    assert!(aoa.misses_table_at(planned.carrier_hz));
    assert!(!aoa.misses_table_at(CENTER_HZ + 51e6));
}

#[test]
fn radar_aoa_needs_no_place_for_the_reference_antenna() {
    let mut skewed = scene(false);
    skewed.distortion = Some(skewed_beam);
    let params = PassiveRadarParams::default();
    for ctx in [scene_ctx(), measured_ctx(&skewed)] {
        let mut positions_m = ctx.positions_m.clone();
        positions_m[0] = positions_m[1];
        let stacked = RadarCtx { positions_m, ..ctx };
        let aoa = plan(&stacked, &params).unwrap().aoa.unwrap();
        assert_eq!(aoa.manifold.len(), 4);
        assert_eq!(aoa.manifold.table().is_some(), stacked.manifold.is_some());
    }
}

#[test]
fn an_empty_scene_reports_empty_lists_every_cpi() {
    let lanes = scene(false).render(SPLIT).unwrap();
    let params = PassiveRadarParams {
        cfar: CfarParams {
            pfa: 1e-7,
            ..CfarParams::default()
        },
        ..PassiveRadarParams::default()
    };
    let mut radar = Radar::new(&scene_ctx(), &params);
    radar.feed(&lanes, 0..SPLIT, true);
    assert_eq!(radar.updates.len(), 4);
    for (seq, update) in (0u64..).zip(&radar.updates) {
        assert_eq!(update.seq, seq);
        assert!(update.detections.is_empty(), "{:?}", update.detections);
        assert!(update.tracks.is_empty());
        assert!(update.events.is_empty());
        assert!(!update.at.is_empty());
        assert_eq!(update.health.suppression_db.len(), 4);
    }
}

fn noise_radar() -> Radar {
    let mut scene = ArrayScene::new(surveillance_array(), CENTER_HZ, INPUT_RATE)
        .with_noise_db(0.0)
        .with_seed(5);
    let lanes = scene.render(520_000).unwrap();
    let params = PassiveRadarParams {
        reference: ReferenceCleaning::Off,
        ..PassiveRadarParams::default()
    };
    let mut radar = Radar::new(&scene_ctx(), &params);
    radar.feed(&lanes, 0..lanes[0].len(), true);
    radar
}

#[test]
fn surface_is_noise_normalised() {
    let radar = noise_radar();
    assert_eq!(radar.updates.len(), 2);
    let mut levels: Vec<f32> = radar
        .pod
        .surface
        .cells
        .iter()
        .map(|&level| RangeDopplerSurface::decode(level))
        .collect();
    levels.sort_by(f32::total_cmp);
    let median = levels[levels.len() / 2];
    assert!(median.abs() <= 1.0, "{median}");
    assert!(radar.last().health.noise_floor_db.is_finite());
}

#[test]
fn surface_axes_match_the_plan() {
    let radar = noise_radar();
    let mut frame = SurfaceFrame::Visibility(VisibilityOwned::default());
    fill_surface(&radar.pod, &mut frame);
    let SurfaceFrame::RangeDoppler(frame) = frame else {
        panic!("not a range doppler frame");
    };
    let plan = &radar.plan;
    let rows = plan.report_rows.len();
    assert_eq!(usize::from(frame.ranges), plan.shape.gates);
    assert_eq!(usize::from(frame.dopplers), rows);
    assert_eq!(frame.cells.len(), plan.shape.gates * rows);
    assert_eq!(frame.range_first_m, 0.0);
    assert!((f64::from(frame.range_step_m) - plan.range_step_m).abs() < 1e-3);
    let first = plan.doppler_of_row(plan.report_rows.start as f64);
    assert!((f64::from(frame.doppler_first_hz) - first).abs() < 1e-3);
    assert!(frame.doppler_first_hz < 0.0);
    assert!((f64::from(frame.doppler_step_hz) - 1.0 / plan.cpi_s).abs() < 1e-4);
    let middle = frame.doppler_first_hz + frame.doppler_step_hz * (rows - 1) as f32 / 2.0;
    assert!(middle.abs() < 1e-3, "{middle}");
    assert_eq!(frame.carrier_hz, CENTER_HZ);
    assert_eq!((frame.db_min, frame.db_max), (-3.0, 30.0));
    assert_eq!(u64::from(frame.seq), radar.pod.seq);
    assert_eq!(frame.timestamp, radar.pod.cpi_end_unix_ns / 1_000_000);
    let axes = radar.last().axes;
    assert_eq!(axes.gates as usize, plan.shape.gates);
    assert_eq!(axes.doppler_rows as usize, rows);
    assert_eq!(axes.lanes, 4);
}

#[test]
fn phase_unknown_blocks_aoa_only() {
    let mut radar = Radar::new(&scene_ctx(), &PassiveRadarParams::default());
    radar.feed(target_scene(), 0..SCENE_SAMPLES, false);
    let last = radar.last();
    assert_eq!(last.tracks.len(), 1);
    assert!(last.tracks[0].aoa.is_none());
    assert!(!last.detections.is_empty());
    assert!(
        last.detections
            .iter()
            .all(|detection| detection.aoa.is_none())
    );
    assert_eq!(last.health.aoa, AoaState::PhaseUnknown);
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

fn random_fm(len: usize, seed: u64) -> Vec<C32> {
    let mut uniform = xorshift(seed);
    let mut phase = 0.0f64;
    let mut drive = 0.0f64;
    (0..len)
        .map(|_| {
            drive = 0.995 * drive + 0.1 * (uniform() - 0.5);
            phase += drive;
            C32::from_polar(1.0, phase as f32)
        })
        .collect()
}

fn gaussian(len: usize, sigma: f32, seed: u64) -> Vec<C32> {
    let mut uniform = xorshift(seed);
    (0..len)
        .map(|_| {
            let radius = (-uniform().ln()).sqrt() as f32 * sigma;
            C32::from_polar(radius, (TAU * uniform()) as f32)
        })
        .collect()
}

fn multipath_lanes(len: usize) -> Vec<Vec<C32>> {
    let clean = random_fm(len, 9);
    let noise = gaussian(len, 1.0, 3);
    let reference = (0..len)
        .map(|n| {
            let echo = n
                .checked_sub(10)
                .map_or(C32::default(), |at| clean[at] * 0.5);
            (clean[n] + echo) * 100.0 + noise[n]
        })
        .collect();
    let mut lanes = vec![reference];
    let gains = [C32::new(80.0, 60.0), C32::new(-30.0, 95.0)];
    for (seed, gain) in (11..).zip(gains) {
        let noise = gaussian(len, 1.0, seed);
        lanes.push(clean.iter().zip(noise).map(|(x, n)| x * gain + n).collect());
    }
    lanes
}

fn mean_suppression(radar: &Radar) -> f32 {
    let tail = &radar.updates[radar.updates.len() - 2..];
    let values: Vec<f32> = tail
        .iter()
        .flat_map(|update| update.health.suppression_db.iter().copied())
        .collect();
    values.iter().sum::<f32>() / values.len() as f32
}

#[test]
fn cma_cleaning_raises_suppression_on_a_multipath_reference() {
    let len = 2_050_000;
    let lanes = multipath_lanes(len);
    let ctx = RadarCtx {
        sample_rate: 250_000.0,
        center_hz: CENTER_HZ,
        elements: 3,
        positions_m: Vec::new(),
        manifold: None,
        tuned_together: true,
    };
    let base = PassiveRadarParams {
        aoa: false,
        clutter: ClutterParams {
            reach_km: 5.0,
            ..ClutterParams::default()
        },
        ..PassiveRadarParams::default()
    };
    let run = |reference: ReferenceCleaning| {
        let mut radar = Radar::new(&ctx, &PassiveRadarParams { reference, ..base });
        radar.feed(&lanes, 0..len, true);
        radar
    };
    let raw = run(ReferenceCleaning::Off);
    let cleaned = run(ReferenceCleaning::Cma {
        taps: 32,
        step: 1e-3,
    });
    let (raw_db, cleaned_db) = (mean_suppression(&raw), mean_suppression(&cleaned));
    assert!(
        cleaned_db >= raw_db + 6.0,
        "raw {raw_db} dB, CMA {cleaned_db} dB"
    );
    let reference = cleaned.last().health.reference;
    assert!(reference.locked, "{reference:?}");
}

#[test]
fn live_changes_keep_tracks() {
    let lanes = target_scene();
    let mut radar = Radar::new(&scene_ctx(), &PassiveRadarParams::default());
    radar.feed(lanes, 0..SPLIT, true);
    let before = radar.track_id();
    let tuned = PassiveRadarParams {
        cfar: CfarParams {
            min_snr_db: 9.0,
            ..CfarParams::default()
        },
        overlap: 0.25,
        ..PassiveRadarParams::default()
    };
    let next = plan(&radar.plan.ctx, &tuned).unwrap();
    assert_eq!(change(&radar.plan, &next), PlanChange::Live);
    let live = next.live();
    radar.front.tune(&live);
    radar.cpi.tune(&live).unwrap();
    let count = radar.updates.len();
    radar.feed(lanes, SPLIT..SCENE_SAMPLES, true);
    assert!(radar.updates.len() > count + 3);
    for update in &radar.updates[count..] {
        assert_eq!(update.tracks.len(), 1);
        assert_eq!(update.tracks[0].id, before);
        assert!(
            update
                .events
                .iter()
                .all(|event| event.change != TrackChange::Lost)
        );
    }
    let hop_ms = (next.hop_s() * 1_000.0) as f32;
    assert!((radar.last().axes.hop_ms - hop_ms).abs() < 1e-3);
}

#[test]
fn rebuild_resets_tracks_and_lists_them_as_ended() {
    let lanes = target_scene();
    let mut radar = Radar::new(&scene_ctx(), &PassiveRadarParams::default());
    radar.feed(lanes, 0..SPLIT, true);
    let old = radar.track_id();
    let rebuilt = PassiveRadarParams {
        cpi_ms: 400,
        ..PassiveRadarParams::default()
    };
    let next = plan(&radar.plan.ctx, &rebuilt).unwrap();
    assert_eq!(change(&radar.plan, &next), PlanChange::Rebuild);
    radar.cpi.retuned(&mut radar.pod);
    fill_update(&radar.pod, &mut radar.update);
    assert!(radar.update.tracks.is_empty());
    assert!(radar.update.detections.is_empty());
    let lost: Vec<u32> = radar
        .update
        .events
        .iter()
        .filter(|event| event.change == TrackChange::Lost)
        .map(|event| event.track_id)
        .collect();
    assert_eq!(lost, vec![old]);
    let mut fresh = Radar::new(&radar.plan.ctx, &rebuilt);
    fresh.cpi.carry_ids(radar.cpi.next_track_id());
    fresh.feed(lanes, SPLIT..SCENE_SAMPLES, true);
    let new = fresh.track_id();
    assert!(new > old, "{new} after {old}");
}

fn dab_channel(clean: &[C32], cfo_hz: f64, sigma: f32) -> Vec<C32> {
    let echo = C32::from_polar(0.4, 1.0);
    let noise = gaussian(clean.len(), sigma, 21);
    (0..clean.len())
        .map(|n| {
            let delayed = n.checked_sub(12).map_or(C32::default(), |at| clean[at]);
            let turn = C32::from_polar(1.0, (TAU * cfo_hz * n as f64 / DAB_RATE) as f32);
            (clean[n] + delayed * echo) * turn + noise[n]
        })
        .collect()
}

fn remodulate(input: &[C32], remod: &mut DabRemod) -> Vec<C32> {
    let mut out = Vec::with_capacity(input.len());
    let mut piece = Vec::with_capacity(FRONT_CHUNK);
    for chunk in input.chunks(FRONT_CHUNK) {
        remod.process(chunk, &mut piece);
        assert_eq!(piece.len(), chunk.len());
        out.extend_from_slice(&piece);
    }
    out
}

fn rebuilt_correlation(cfo: f64) -> (sdrmm_wire::radar::ReferenceHealth, f64) {
    let clean = crate::testgen::dab::ensemble(8);
    let received = dab_channel(&clean, cfo, 0.03);
    let mut remod = DabRemod::new(DAB_RATE).unwrap();
    let out = remodulate(&received, &mut remod);
    let latency = remod.latency();
    let widen = |value: C32| Complex::new(f64::from(value.re), f64::from(value.im));
    let (mut dot, mut rebuilt, mut ideal) = (Complex::<f64>::default(), 0.0f64, 0.0f64);
    for index in 3 * DAB_FRAME..6 * DAB_FRAME {
        let turn = Complex::from_polar(1.0, TAU * cfo * index as f64 / DAB_RATE);
        let want = widen(clean[index]) * turn;
        let got = widen(out[index + latency]);
        dot += got * want.conj();
        rebuilt += got.norm_sqr();
        ideal += want.norm_sqr();
    }
    (remod.health(), dot.norm() / (rebuilt * ideal).sqrt())
}

#[test]
fn dab_remod_rebuilds_a_clean_reference() {
    let (health, correlation) = rebuilt_correlation(300.0);
    assert!(health.locked, "{health:?}");
    assert!(health.quality_db >= 20.0, "{health:?}");
    assert!(correlation >= 0.99, "{correlation}");
}

#[test]
fn dab_remod_finds_a_whole_carrier_offset() {
    let (health, correlation) = rebuilt_correlation(-2_250.0);
    assert!(health.locked, "{health:?}");
    assert!(health.quality_db >= 20.0, "{health:?}");
    assert!(correlation >= 0.99, "{correlation}");
}

#[test]
fn dab_remod_falls_back_and_counts_when_sync_is_lost() {
    let clean = crate::testgen::dab::ensemble(4);
    let mut received = dab_channel(&clean, 120.0, 0.03);
    received.extend(gaussian(3 * DAB_FRAME, 0.87, 77));
    let mut remod = DabRemod::new(DAB_RATE).unwrap();
    let latency = remod.latency();
    let mut out = remodulate(&received[..4 * DAB_FRAME], &mut remod);
    assert!(remod.health().locked, "{:?}", remod.health());
    let before = remod.health().fallback_frames;
    out.extend(remodulate(&received[4 * DAB_FRAME..], &mut remod));
    let health = remod.health();
    assert!(!health.locked, "{health:?}");
    assert!(health.fallback_frames > before, "{health:?}");
    for index in 5 * DAB_FRAME..out.len() - latency {
        assert_eq!(out[index + latency], received[index], "{index}");
    }
    assert!(DabRemod::new(KRAKEN_RATE).is_err());
}

struct Broken;

impl CafBackend for Broken {
    fn run(&mut self, _: &CpiJob, _: &mut CubeOut) -> Result<(), CafError> {
        Err(CafError::Crew)
    }

    fn gpu(&self) -> bool {
        false
    }

    fn threads(&self) -> u32 {
        2
    }
}

#[test]
fn a_failed_caf_reports_empty_lists_and_says_why() {
    let plan = plan(&scene_ctx(), &PassiveRadarParams::default()).unwrap();
    let mut cpi = CpiStage::new(&plan, Box::new(Broken)).unwrap();
    let mut pod = RadarPod::new(&plan);
    pod.surface.cells.fill(200);
    let job = CpiJob::new(plan.front.lanes.len(), plan.shape.window());
    let outcome = cpi.run(&job, &mut pod);
    assert_eq!(outcome.failed, Some(CafError::Crew));
    assert!(pod.detections().is_empty() && pod.tracks().is_empty());
    assert!(pod.surface.cells.iter().all(|&level| level == 0));
    assert_eq!(pod.threads, 2);
}

struct Switchable {
    inner: CpuCaf,
    fail: Arc<AtomicBool>,
}

impl CafBackend for Switchable {
    fn run(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), CafError> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(CafError::Crew);
        }
        self.inner.run(job, out)
    }

    fn gpu(&self) -> bool {
        false
    }

    fn threads(&self) -> u32 {
        0
    }
}

#[test]
fn a_failed_cpi_keeps_the_lost_events_of_a_retune() {
    let plan = plan(&scene_ctx(), &PassiveRadarParams::default()).unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let backend = Switchable {
        inner: CpuCaf::new(&plan).unwrap(),
        fail: Arc::clone(&fail),
    };
    let mut radar = Radar::with_backend(plan, Box::new(backend));
    radar.feed(target_scene(), 0..SPLIT, true);
    let old = radar.track_id();
    fail.store(true, Ordering::Relaxed);
    let mut job = radar.pool.free.pop().unwrap();
    Arc::get_mut(&mut job).unwrap().generation = 1;
    let outcome = radar.cpi.run(&job, &mut radar.pod);
    assert_eq!(outcome.failed, Some(CafError::Crew));
    assert!(radar.pod.detections().is_empty() && radar.pod.tracks().is_empty());
    let lost: Vec<u32> = radar
        .pod
        .events()
        .iter()
        .filter(|event| event.change == TrackChange::Lost)
        .map(|event| event.track_id)
        .collect();
    assert_eq!(lost, vec![old]);
}

#[test]
fn track_events_repeat_every_five_seconds() {
    let mut radar = Radar::new(&scene_ctx(), &PassiveRadarParams::default());
    radar.clock_scale = 4.0;
    radar.feed(target_scene(), 0..SCENE_SAMPLES, true);
    let events: Vec<(u64, TrackChange)> = radar
        .updates
        .iter()
        .zip(&radar.ends_ns)
        .flat_map(|(update, &at)| update.events.iter().map(move |event| (at, event.change)))
        .collect();
    let changes: Vec<TrackChange> = events.iter().map(|&(_, change)| change).collect();
    assert_eq!(changes, vec![TrackChange::Confirmed, TrackChange::Update]);
    let gap = events[1].0 - events[0].0;
    assert!(gap >= 5_000_000_000, "{gap}");
    assert!(radar.updates.iter().all(|update| update.tracks.len() <= 1));
}

#[test]
fn canceller_resets_are_counted_as_unsuppressed() {
    let len = 1_300_000;
    let mut lanes = multipath_lanes(len);
    lanes[1][400_000] = C32::new(f32::NAN, 0.0);
    let ctx = RadarCtx {
        sample_rate: 250_000.0,
        center_hz: CENTER_HZ,
        elements: 3,
        positions_m: Vec::new(),
        manifold: None,
        tuned_together: true,
    };
    let params = PassiveRadarParams {
        aoa: false,
        reference: ReferenceCleaning::Off,
        clutter: ClutterParams {
            method: ClutterMethod::Nlms,
            reach_km: 5.0,
            ..ClutterParams::default()
        },
        ..PassiveRadarParams::default()
    };
    let mut radar = Radar::new(&ctx, &params);
    radar.feed(&lanes, 0..len, true);
    let health = &radar.last().health;
    assert!(health.unsuppressed_groups > 0, "{health:?}");
    assert!(
        radar
            .updates
            .iter()
            .all(|update| update.detections.iter().all(|d| d.snr_db.is_finite()))
    );
}

const DAB_CFO_HZ: f64 = 150.0;
const DAB_ECHO_GATES: usize = 200;
const DAB_ECHO_HZ: f64 = 40.0;

fn dab_radar_lanes(frames: usize) -> Vec<Vec<C32>> {
    let clean = crate::testgen::dab::ensemble(frames);
    let len = clean.len();
    let turn = |n: usize, hz: f64| C32::from_polar(1.0, (TAU * hz * n as f64 / DAB_RATE) as f32);
    let multipath = C32::from_polar(0.4, 1.0);
    let noise = gaussian(len, 0.03, 31);
    let reference = (0..len)
        .map(|n| {
            let ghost = n.checked_sub(12).map_or(C32::default(), |at| clean[at]);
            (clean[n] + ghost * multipath) * turn(n, DAB_CFO_HZ) + noise[n]
        })
        .collect();
    let mut lanes = vec![reference];
    let gains = [C32::new(0.8, 0.6), C32::new(-0.6, 0.8)];
    for (seed, gain) in (41..).zip(gains) {
        let noise = gaussian(len, 0.03, seed);
        let lane = (0..len)
            .map(|n| {
                let echo = n
                    .checked_sub(DAB_ECHO_GATES)
                    .map_or(C32::default(), |at| clean[at] * 0.01 * turn(n, DAB_ECHO_HZ));
                (clean[n] * gain + echo * gain.conj()) * turn(n, DAB_CFO_HZ) + noise[n]
            })
            .collect();
        lanes.push(lane);
    }
    lanes
}

fn dab_echo_run(method: ClutterMethod) {
    let lanes = dab_radar_lanes(9);
    let ctx = RadarCtx {
        sample_rate: DAB_RATE,
        center_hz: 220e6,
        elements: 3,
        positions_m: Vec::new(),
        manifold: None,
        tuned_together: true,
    };
    let params = PassiveRadarParams {
        cpi_ms: 100,
        aoa: false,
        clutter: ClutterParams {
            method,
            ..ClutterParams::default()
        },
        ..dab_params()
    };
    let mut radar = Radar::new(&ctx, &params);
    radar.feed(&lanes, 0..lanes[0].len(), true);
    assert!(
        radar.updates.len() >= 5,
        "{method:?}: {}",
        radar.updates.len()
    );
    let step_km = radar.plan.range_step_m / 1_000.0;
    let want_km = DAB_ECHO_GATES as f64 * step_km;
    let step_hz = 1.0 / radar.plan.cpi_s;
    for update in &radar.updates[2..] {
        let reference = update.health.reference;
        assert!(reference.locked, "{method:?}: {reference:?}");
        assert!(reference.quality_db >= 20.0, "{method:?}: {reference:?}");
        let strongest = update
            .detections
            .iter()
            .max_by(|a, b| a.snr_db.total_cmp(&b.snr_db))
            .unwrap();
        let range_km = f64::from(strongest.range_km);
        assert!(
            (range_km - want_km).abs() < step_km,
            "{method:?}: {strongest:?}"
        );
        let doppler = f64::from(strongest.doppler_hz);
        assert!(
            (doppler - DAB_ECHO_HZ).abs() < step_hz,
            "{method:?}: {strongest:?}"
        );
    }
}

#[test]
fn a_dab_echo_lands_on_its_range_through_the_rebuilt_reference() {
    dab_echo_run(ClutterMethod::EcaBatch);
    dab_echo_run(ClutterMethod::BlockNlms);
}

#[test]
fn a_job_still_in_use_goes_back_to_the_pool() {
    let params = PassiveRadarParams {
        reference: ReferenceCleaning::Off,
        ..PassiveRadarParams::default()
    };
    let plan = plan(&scene_ctx(), &params).unwrap();
    let mut front = FrontStage::new(&plan).unwrap();
    let window = plan.shape.window();
    let busy = Arc::new(CpiJob::new(plan.front.lanes.len(), window));
    let mut pool = Pool {
        free: vec![Arc::clone(&busy)],
        ready: VecDeque::new(),
        dropped: 0,
    };
    let base = multipath_lanes(600_000);
    let lanes: Vec<Vec<C32>> = (0..5).map(|element| base[element.min(2)].clone()).collect();
    let mut start = 0;
    while pool.dropped == 0 {
        assert!(start < lanes[0].len(), "no CPI was assembled");
        let end = (start + BLOCK).min(lanes[0].len());
        let views: Vec<&[C32]> = plan
            .front
            .lanes
            .iter()
            .map(|&element| &lanes[element][start..end])
            .collect();
        let input = InLanes {
            lanes: &views,
            first_index: start as u64,
            unix_ns: BASE_NS,
            gap_before: false,
            phase_ready: true,
            generation: 0,
        };
        front.push(&input, &mut pool);
        start = end;
    }
    assert!(pool.ready.is_empty());
    assert_eq!(pool.free.len(), 1);
    assert!(Arc::ptr_eq(&pool.free[0], &busy));
    drop(busy);
    assert!(Arc::get_mut(&mut pool.free[0]).is_some());
}
