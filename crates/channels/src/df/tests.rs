use num_complex::Complex;
use sdrmm_dsp::doa::rate_spread;
use sdrmm_dsp::manifold::{Direction, ManifoldTable};
use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
use sdrmm_dsp::special::wrap_deg;
use sdrmm_wire::{
    ArrayElement, ArrayGeometry, ArrayTuningMode, BearingSource, CalPhase, CalSourceKind,
    Coherence, DecoderEvent, DfBearing, DfPeak, DfReading, ProcessorReading, RdsUpdate, Winding,
};

use super::*;
use crate::array_processor::{
    CorrectionView, GeoFix, OutputSlots, Steer, create_processor, processor_descriptor,
};

const RATE: f64 = 2_400_000.0;
const CENTER_HZ: f64 = 433.92e6;
const OFFSET_HZ: f64 = 25_000.0;
const RADIUS_M: f64 = 0.2939;
const BLOCK: usize = 24_000;
const REPORT_MS: u32 = 100;
const BLOCKS_PER_REPORT: usize = 10;
const NANOS_PER_BLOCK: u64 = 10_000_000;

fn kraken() -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m: RADIUS_M,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    }
}

fn half_wave_ula() -> ArrayGeometry {
    ArrayGeometry::Ula {
        spacing_m: 299_792_458.0 / CENTER_HZ / 2.0,
        axis_deg: 90.0,
    }
}

fn params() -> DfParams {
    DfParams {
        offset_hz: OFFSET_HZ,
        report_ms: REPORT_MS,
        ..DfParams::default()
    }
}

fn tone(azimuth_deg: f64, power_db: f32, offset_hz: f64) -> SceneSource {
    SceneSource::new(
        Direction::horizon(azimuth_deg),
        power_db,
        SceneSignal::Tone { offset_hz },
    )
}

fn fix() -> GeoFix {
    GeoFix {
        lat: 48.1,
        lon: 11.5,
        accuracy_m: Some(4.0),
        ..GeoFix::default()
    }
}

fn located(heading_deg: f64) -> Pose {
    Pose {
        heading_deg: Some(heading_deg),
        fix: Some(fix()),
        ..Pose::default()
    }
}

fn error_deg(got: f64, want: f64) -> f64 {
    wrap_deg(got - want).abs()
}

struct Array {
    node: String,
    geometry: ArrayGeometry,
    positions: Vec<[f64; 3]>,
    center_hz: f64,
    centers: Vec<f64>,
    table: Option<ManifoldTable>,
}

impl Array {
    fn new(geometry: ArrayGeometry, lanes: usize) -> Self {
        Self {
            node: "df-1".to_owned(),
            positions: geometry.positions(lanes).unwrap(),
            geometry,
            center_hz: CENTER_HZ,
            centers: vec![CENTER_HZ; lanes],
            table: None,
        }
    }

    fn tune(&mut self, center_hz: f64) {
        self.center_hz = center_hz;
        self.centers.fill(center_hz);
    }

    fn ctx(&self) -> ArrayCtx<'_> {
        ArrayCtx {
            node: &self.node,
            lanes: self.centers.len(),
            sample_rate: RATE,
            center_hz: self.center_hz,
            lane_centers_hz: &self.centers,
            geometry: &self.geometry,
            positions_m: &self.positions,
            manifold: self.table.as_ref(),
            tier: Coherence::PhaseCoherent,
            tuning: ArrayTuningMode::Together,
            max_block: BLOCK,
        }
    }

    fn scene(&self, sources: &[SceneSource], seed: u64) -> ArrayScene {
        let geometry = geometry_of(&self.geometry, self.centers.len()).unwrap();
        sources.iter().fold(
            ArrayScene::new(geometry, self.center_hz, RATE)
                .with_noise_db(0.0)
                .with_seed(seed),
            |scene, source| scene.with_source(*source),
        )
    }
}

#[derive(Default)]
struct Seen {
    readings: Vec<DfReading>,
    events: Vec<DfBearing>,
    steers: Vec<Steer>,
}

struct Rig {
    array: Array,
    scene: ArrayScene,
    processor: DfProcessor,
    pose: Pose,
    cal: CalView,
    report: Option<ProcessorReading>,
    events: Vec<DecoderEvent>,
    unix_ns: u64,
    seen: Seen,
}

impl Rig {
    fn new(array: Array, df: DfParams, sources: &[SceneSource], seed: u64) -> Self {
        let scene = array.scene(sources, seed);
        let processor = DfProcessor::new(&array.ctx(), &ProcessorParams::Df(df)).unwrap();
        Self {
            array,
            scene,
            processor,
            pose: located(0.0),
            cal: CalView::default(),
            report: ProcessorReading::empty("df"),
            events: vec![DecoderEvent::Rds(RdsUpdate::default()); 2],
            unix_ns: 1_790_000_000_000_000_000,
            seen: Seen::default(),
        }
    }

    fn kraken(df: DfParams, sources: &[SceneSource]) -> Self {
        Self::new(Array::new(kraken(), 5), df, sources, 7)
    }

    fn step(&mut self) {
        let rendered = self.scene.render(BLOCK).unwrap();
        let lanes: Vec<&[Complex<f32>]> = rendered.iter().map(Vec::as_slice).collect();
        let block = ArrayBlock {
            lanes: &lanes,
            corrected: true,
            correction: CorrectionView::identity(),
            first_index: 0,
            unix_ns: self.unix_ns,
            generation: 0,
            gap_before: false,
            centers_hz: &self.array.centers,
            cal: self.cal,
            pose: self.pose,
        };
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: None,
            events: &mut self.events,
            lanes: &mut [],
        });
        self.processor.process(&block, &mut out);
        let tally = out.tally();
        if tally.report
            && let Some(ProcessorReading::Df(reading)) = &self.report
        {
            self.seen.readings.push(reading.clone());
        }
        for event in &self.events[..tally.events] {
            if let DecoderEvent::Df(bearing) = event {
                self.seen.events.push(bearing.clone());
            }
        }
        self.seen.steers.extend(tally.steer);
        self.unix_ns += NANOS_PER_BLOCK;
    }

    fn run(&mut self, reports: usize) -> &DfReading {
        let want = self.seen.readings.len() + reports;
        while self.seen.readings.len() < want {
            self.step();
        }
        self.last()
    }

    fn last(&self) -> &DfReading {
        self.seen.readings.last().unwrap()
    }

    fn primary(&self) -> &DfPeak {
        self.last().peaks.first().unwrap()
    }
}

fn refused(ctx: &ArrayCtx<'_>, df: DfParams) -> String {
    match create_processor(ctx, &ProcessorParams::Df(df)) {
        Err(error) => error.to_string(),
        Ok(_) => "built".to_owned(),
    }
}

fn loudest_degree(bytes: &[u8]) -> f64 {
    bytes
        .iter()
        .enumerate()
        .max_by_key(|(_, byte)| **byte)
        .map(|(degree, _)| degree as f64)
        .unwrap()
}

#[test]
fn df_reads_the_kraken_uca_bearing_through_the_ddc() {
    for azimuth in [0.0, 137.0, 271.0] {
        let mut rig = Rig::kraken(params(), &[tone(azimuth, 0.0, OFFSET_HZ)]);
        let reading = rig.run(3).clone();
        let primary = &reading.peaks[0];
        assert!(
            error_deg(f64::from(primary.relative_deg), azimuth) < 1.0,
            "{azimuth}: {primary:?}"
        );
        assert_eq!(reading.pseudospectrum.len(), DF_POINTS);
        let top = loudest_degree(&reading.pseudospectrum);
        assert!(error_deg(top, azimuth) <= 1.0, "{azimuth}: {top}");
        assert!((reading.freq_hz - (CENTER_HZ + OFFSET_HZ)).abs() < 1e-3);
        assert!(!reading.squelched && !reading.aliasing && !reading.singular);
        assert_eq!(reading.algorithm, DfAlgorithm::Music);
        assert_eq!(reading.eigenvalues_db.len(), 5);
        assert!(reading.snapshots > 1_000.0, "{}", reading.snapshots);
        assert!(reading.likelihood_true);
        assert_eq!(reading.likelihood.len(), DF_POINTS);
    }
}

#[test]
fn df_true_bearing_adds_the_array_heading() {
    let mut rig = Rig::kraken(params(), &[tone(40.0, 0.0, OFFSET_HZ)]);
    rig.pose = located(90.0);
    rig.run(3);
    let primary = rig.primary().clone();
    let true_deg = f64::from(primary.true_deg.unwrap());
    assert!(error_deg(true_deg, f64::from(primary.relative_deg) + 90.0) < 1e-3);
    assert!(error_deg(true_deg, 130.0) < 1.0, "{primary:?}");
    assert_eq!(rig.last().azimuth_deg.map(f64::round), Some(90.0));
    let event = rig.seen.events.last().unwrap();
    assert!(error_deg(f64::from(event.bearing_deg), true_deg) < 1e-3);
    assert_eq!(event.heading_deg, Some(90.0));
    assert_eq!(event.relative_deg, Some(primary.relative_deg));
    assert_eq!(event.source, BearingSource::Array);
    assert_eq!(event.node, "df-1");
    assert_eq!(event.station_id.as_deref(), Some("df-1"));
    assert_eq!((event.lat, event.lon), (Some(48.1), Some(11.5)));
    assert_eq!(event.accuracy_m, Some(4.0));
    assert_eq!(event.likelihood.len(), DF_POINTS);
    let peak = loudest_degree(&event.likelihood);
    assert!(error_deg(peak, 130.0) <= 1.5, "{peak}");
}

#[test]
fn a_noise_cal_keeps_the_antenna_term() {
    let azimuth = 60.0;
    let freq_hz = CENTER_HZ + OFFSET_HZ;
    let geometry = geometry_of(&kraken(), 5).unwrap();
    let mut rates = [0.0f64; 5];
    Manifold::ideal(geometry).phase_rates(freq_hz, Direction::horizon(azimuth), &mut rates);
    let root_h = rate_spread(&rates).sqrt();
    let expected = |antenna: f64| (0.2f64.hypot(antenna) / root_h).hypot(2.0);
    let mut sigmas = Vec::new();
    for source in [CalSourceKind::Noise, CalSourceKind::Emitter] {
        let mut rig = Rig::kraken(params(), &[tone(azimuth, 10.0, OFFSET_HZ)]);
        rig.cal = CalView {
            phase: CalPhase::Solved,
            source: Some(source),
            phase_ready: true,
            gain_ready: true,
            phase_sigma_deg: 0.2,
            ..CalView::default()
        };
        rig.run(3);
        assert!(rig.last().snr_db > 25.0, "{}", rig.last().snr_db);
        sigmas.push(f64::from(rig.primary().sigma_deg));
    }
    assert!(sigmas[0] >= 2.0 && sigmas[0] >= 5.0 / root_h, "{sigmas:?}");
    assert!((sigmas[0] - expected(5.0)).abs() < 0.1, "{sigmas:?}");
    assert!((sigmas[1] - expected(2.0)).abs() < 0.1, "{sigmas:?}");
    assert!(sigmas[1] < sigmas[0]);
}

#[test]
fn df_follow_mode_without_heading_keeps_relative_and_emits_nothing() {
    let mut rig = Rig::kraken(params(), &[tone(137.0, 0.0, OFFSET_HZ)]);
    rig.pose = Pose {
        heading_deg: None,
        fix: Some(fix()),
        follows: true,
        ..Pose::default()
    };
    let reading = rig.run(3).clone();
    assert_eq!(reading.azimuth_deg, None);
    assert_eq!(reading.heading_sigma_deg, None);
    assert!(!reading.likelihood_true);
    assert_eq!(reading.likelihood.len(), DF_POINTS);
    assert!(error_deg(f64::from(reading.peaks[0].relative_deg), 137.0) < 1.0);
    assert!(reading.peaks.iter().all(|peak| peak.true_deg.is_none()));
    assert!(reading.station.is_some());
    assert!(rig.seen.events.is_empty());
    let steer = rig.seen.steers.last().unwrap();
    assert_eq!(steer.true_deg, None);
    assert!(steer.same_array);
}

#[test]
fn df_without_a_fix_emits_no_event() {
    let mut rig = Rig::kraken(params(), &[tone(137.0, 0.0, OFFSET_HZ)]);
    rig.pose = Pose {
        heading_deg: Some(10.0),
        ..Pose::default()
    };
    let reading = rig.run(3).clone();
    assert_eq!(reading.station, None);
    assert!(reading.peaks[0].true_deg.is_some());
    assert!(rig.seen.events.is_empty());
    assert!(!rig.seen.steers.is_empty());
}

#[test]
fn a_df_without_position_emits_no_event() {
    let mut rig = Rig::kraken(params(), &[tone(20.0, 0.0, OFFSET_HZ)]);
    rig.pose = Pose::default();
    let reading = rig.run(3).clone();
    assert_eq!((reading.station, reading.azimuth_deg), (None, None));
    assert!(error_deg(f64::from(reading.peaks[0].relative_deg), 20.0) < 1.0);
    assert!(rig.seen.events.is_empty());
}

#[test]
fn df_yaw_gate_skips_blocks_while_rotating() {
    let mut rig = Rig::kraken(params(), &[tone(40.0, 0.0, OFFSET_HZ)]);
    rig.pose.yaw_rate_dps = Some(45.0);
    let first = rig.run(1).clone();
    assert!(first.rotating);
    assert_eq!(first.gated_blocks, BLOCKS_PER_REPORT as u32);
    assert!(first.peaks.is_empty());
    assert!(rig.seen.events.is_empty());
    for index in 0..BLOCKS_PER_REPORT {
        let turning = index >= BLOCKS_PER_REPORT / 2;
        rig.pose.yaw_rate_dps = Some(if turning { -45.0 } else { 5.0 });
        if turning {
            rig.scene.sources[0].direction = Direction::horizon(200.0);
        }
        rig.step();
    }
    let mixed = rig.last().clone();
    assert!(mixed.rotating);
    assert_eq!(mixed.gated_blocks, (BLOCKS_PER_REPORT / 2) as u32);
    assert_eq!(mixed.peaks.len(), 1, "{:?}", mixed.peaks);
    assert!(error_deg(f64::from(mixed.peaks[0].relative_deg), 40.0) < 1.0);
    rig.pose.yaw_rate_dps = Some(0.0);
    rig.run(1);
    assert!(!rig.last().rotating);
    assert_eq!(rig.last().gated_blocks, 0);
    assert_eq!(rig.last().peaks.len(), 1);
    let (events, steers) = (rig.seen.events.len(), rig.seen.steers.len());
    rig.pose.yaw_rate_dps = Some(45.0);
    let carried = rig.run(1).clone();
    assert!(carried.rotating && carried.peaks.is_empty(), "{carried:?}");
    assert!(!carried.squelched && carried.likelihood.is_empty());
    assert_eq!(
        (rig.seen.events.len(), rig.seen.steers.len()),
        (events, steers)
    );
}

#[test]
fn df_singular_covariance_is_reported_and_counted() {
    let mut rig = Rig::kraken(params(), &[]);
    rig.scene.noise_db.fill(-1_000.0);
    let reading = rig.run(1).clone();
    assert!(reading.singular, "{reading:?}");
    assert!(reading.peaks.is_empty() && !reading.squelched);
    assert!(reading.pseudospectrum.iter().all(|&byte| byte == 0));
    assert_eq!(rig.processor.faults().solver_failures, 1);
    assert!(rig.seen.events.is_empty() && rig.seen.steers.is_empty());
    rig.scene.noise_db.fill(0.0);
    rig.scene.sources.push(tone(70.0, 0.0, OFFSET_HZ));
    let recovered = rig.run(1).clone();
    assert!(!recovered.singular);
    assert!(error_deg(f64::from(recovered.peaks[0].relative_deg), 70.0) < 1.0);
}

#[test]
fn df_heading_spread_widens_the_event_sigma() {
    let mut rig = Rig::kraken(params(), &[tone(40.0, 0.0, OFFSET_HZ)]);
    for index in 0..3 * BLOCKS_PER_REPORT {
        let drift = 10.0 * (index % BLOCKS_PER_REPORT) as f64 / (BLOCKS_PER_REPORT - 1) as f64;
        rig.pose = located(100.0 + drift);
        rig.step();
    }
    let reading = rig.last();
    let event = rig.seen.events.last().unwrap();
    let heading_sigma = f64::from(event.heading_sigma_deg.unwrap());
    assert!(heading_sigma > 2.5, "{heading_sigma}");
    assert!(
        event.sigma_deg > reading.peaks[0].sigma_deg + 0.5,
        "{event:?}"
    );
    assert_eq!(reading.heading_sigma_deg, event.heading_sigma_deg);
}

#[test]
fn df_noise_closes_the_squelch_and_emits_nothing() {
    let mut rig = Rig::kraken(params(), &[]);
    let reading = rig.run(3).clone();
    assert!(reading.squelched);
    assert!(reading.peaks.is_empty());
    assert!(reading.likelihood.is_empty());
    assert_eq!(reading.pseudospectrum.len(), DF_POINTS);
    assert!(rig.seen.events.is_empty());
    assert!(rig.seen.steers.is_empty());
}

fn two_emitters() -> [SceneSource; 2] {
    [
        tone(40.0, 0.0, OFFSET_HZ - 3_000.0),
        tone(200.0, 0.0, OFFSET_HZ + 4_000.0),
    ]
}

#[test]
fn df_two_emitters_give_two_peaks_and_others_in_the_event() {
    let mut rig = Rig::kraken(params(), &two_emitters());
    let reading = rig.run(3).clone();
    assert_eq!(reading.peaks.len(), 2, "{:?}", reading.peaks);
    assert_eq!(reading.sources, 2);
    assert!(reading.sources_auto);
    let near = |want: f64| {
        reading
            .peaks
            .iter()
            .any(|peak| error_deg(f64::from(peak.relative_deg), want) < 2.0)
    };
    assert!(near(40.0) && near(200.0), "{:?}", reading.peaks);
    let event = rig.seen.events.last().unwrap();
    assert_eq!(event.others.len(), 1);
    let other = f64::from(reading.peaks[1].true_deg.unwrap());
    assert!(error_deg(f64::from(event.others[0].bearing_deg), other) < 1e-3);
}

#[test]
fn df_ula_event_carries_the_mirror() {
    let array = Array::new(half_wave_ula(), 4);
    let mut rig = Rig::new(array, params(), &[tone(30.0, 0.0, OFFSET_HZ)], 11);
    let reading = rig.run(3).clone();
    assert!(reading.mirror);
    let primary = &reading.peaks[0];
    assert!(
        error_deg(f64::from(primary.relative_deg), 30.0) < 1.0,
        "{primary:?}"
    );
    let event = rig.seen.events.last().unwrap();
    let mirror = f64::from(event.mirror_deg.unwrap());
    assert!(error_deg(mirror, 150.0) < 1.0, "{mirror}");
    assert!(event.confidence <= 0.5);
    assert_eq!(primary.mirror_true_deg, event.mirror_deg);
}

#[test]
fn df_refuses_root_music_on_an_explicit_triangle() {
    let element = |x_m: f64, y_m: f64| ArrayElement { x_m, y_m, z_m: 0.0 };
    let triangle = ArrayGeometry::Explicit {
        positions: vec![
            element(0.0, 0.3),
            element(0.26, -0.15),
            element(-0.26, -0.15),
        ],
    };
    let array = Array::new(triangle, 3);
    let root_music = DfParams {
        algorithm: DfAlgorithm::RootMusic,
        ..params()
    };
    assert_eq!(refused(&array.ctx(), root_music), "Needs a line or circle");
    assert_eq!(refused(&array.ctx(), params()), "built");
}

#[test]
fn df_refuses_spread_tuning() {
    let mut array = Array::new(kraken(), 5);
    for (lane, center) in array.centers.iter_mut().enumerate() {
        *center = CENTER_HZ + 1e6 * lane as f64;
    }
    assert_eq!(
        refused(&array.ctx(), params()),
        "Needs lanes tuned together"
    );
    let outside = DfParams {
        offset_hz: 1.195e6,
        ..params()
    };
    assert_eq!(
        refused(&Array::new(kraken(), 5).ctx(), outside),
        "Offset out of range"
    );
}

#[test]
fn df_algorithm_change_applies_in_place() {
    let music = ProcessorParams::Df(params());
    let capon = ProcessorParams::Df(DfParams {
        algorithm: DfAlgorithm::Capon,
        report_ms: 2 * REPORT_MS,
        ..params()
    });
    let descriptor = processor_descriptor("df").unwrap();
    assert!((descriptor.in_place)(&music, &capon));
    let mut rig = Rig::kraken(params(), &[tone(137.0, 0.0, OFFSET_HZ)]);
    rig.run(1);
    rig.processor.apply(&capon).unwrap();
    let before = rig.seen.readings.len();
    for _ in 0..2 * BLOCKS_PER_REPORT - 1 {
        rig.step();
    }
    assert_eq!(rig.seen.readings.len(), before);
    let reading = rig.run(1).clone();
    assert_eq!(reading.algorithm, DfAlgorithm::Capon);
    assert_eq!(reading.span_db, 30.0);
    assert!(error_deg(f64::from(reading.peaks[0].relative_deg), 137.0) < 1.0);
    let broken = ProcessorParams::Df(DfParams {
        sources: Some(5),
        ..params()
    });
    assert_eq!(
        rig.processor.apply(&broken).map_err(|e| e.to_string()),
        Err("Too many sources".to_owned())
    );
}

#[test]
fn df_step_change_asks_for_a_rebuild() {
    let descriptor = processor_descriptor("df").unwrap();
    let base = ProcessorParams::Df(params());
    for changed in [
        DfParams {
            azimuth_step_deg: 2.0,
            ..params()
        },
        DfParams {
            elevation: true,
            ..params()
        },
        DfParams {
            offset_hz: 10_000.0,
            ..params()
        },
        DfParams {
            station_id: Some("roof".to_owned()),
            ..params()
        },
    ] {
        assert!(!(descriptor.in_place)(&base, &ProcessorParams::Df(changed)));
    }
}

#[test]
fn df_retune_rebuilds_the_grid_in_place_and_resets() {
    let mut rig = Rig::kraken(params(), &[tone(137.0, 0.0, OFFSET_HZ)]);
    rig.run(2);
    let retuned_hz = 420e6;
    rig.array.tune(retuned_hz);
    rig.scene.center_hz = retuned_hz;
    rig.scene.sources[0].direction = Direction::horizon(250.0);
    rig.processor.retune(&rig.array.ctx()).unwrap();
    let reading = rig.run(1).clone();
    assert!((reading.freq_hz - (retuned_hz + OFFSET_HZ)).abs() < 1e-3);
    assert_eq!(reading.peaks.len(), 1, "{:?}", reading.peaks);
    assert!(error_deg(f64::from(reading.peaks[0].relative_deg), 250.0) < 1.0);
    let mut spread = Array::new(kraken(), 5);
    spread.centers[1] += 1e6;
    assert!(rig.processor.retune(&spread.ctx()).is_err());
}

#[test]
fn df_steer_out_carries_relative_true_and_others() {
    let mut rig = Rig::kraken(params(), &two_emitters());
    rig.pose = located(90.0);
    let reading = rig.run(3).clone();
    let steer = *rig.seen.steers.last().unwrap();
    let (primary, other) = (&reading.peaks[0], &reading.peaks[1]);
    assert!(steer.same_array);
    assert_eq!(steer.relative_deg as f32, primary.relative_deg);
    assert_eq!(steer.true_deg.map(|deg| deg as f32), primary.true_deg);
    assert_eq!(steer.sigma_deg, primary.sigma_deg);
    assert_eq!(steer.others, 1);
    assert_eq!(steer.others_relative_deg[0] as f32, other.relative_deg);
    assert_eq!(
        steer.others_true_deg[0].map(|deg| deg as f32),
        other.true_deg
    );
    assert!(steer.wall_ms > 0);
}

fn warp(element: usize, azimuth_deg: f64) -> Complex<f32> {
    let phase = 0.9 * (2.0 * azimuth_deg.to_radians() + 1.3 * element as f64).sin();
    Complex::from_polar(1.0, phase as f32)
}

#[test]
fn df_measured_table_is_used_and_out_of_range_is_flagged() {
    let mut array = Array::new(kraken(), 5);
    let mut scene = array.scene(&[tone(100.0, 0.0, OFFSET_HZ)], 13);
    scene.distortion = Some(warp);
    let freqs: Vec<f64> = (0..=50).map(|step| 400e6 + 1e6 * f64::from(step)).collect();
    array.table = Some(scene.distortion_table(&freqs, 2.0).unwrap());
    let mut rig = Rig::new(array, params(), &[], 13);
    rig.scene = scene.clone();
    let reading = rig.run(3).clone();
    assert!(!reading.table_out_of_range);
    assert!(
        error_deg(f64::from(reading.peaks[0].relative_deg), 100.0) < 1.0,
        "{:?}",
        reading.peaks
    );
    let mut ideal = Rig::kraken(params(), &[]);
    ideal.scene = scene;
    let unaware = ideal.run(3).peaks.first().map(|peak| peak.relative_deg);
    assert!(
        unaware.is_none_or(|deg| error_deg(f64::from(deg), 100.0) > 3.0),
        "{unaware:?}"
    );
    rig.array.tune(500e6);
    rig.scene.center_hz = 500e6;
    rig.processor.retune(&rig.array.ctx()).unwrap();
    assert!(rig.run(1).table_out_of_range);
}

#[test]
fn df_refuses_root_music_while_a_measured_table_is_in_use() {
    let mut array = Array::new(kraken(), 5);
    let mut scene = array.scene(&[], 17);
    scene.distortion = Some(warp);
    let freqs = [CENTER_HZ - 1e6, CENTER_HZ + 1e6];
    array.table = Some(scene.distortion_table(&freqs, 2.0).unwrap());
    let root = DfParams {
        algorithm: DfAlgorithm::RootMusic,
        ..params()
    };
    assert_eq!(refused(&array.ctx(), root), "Table needs a grid method");
    assert_eq!(refused(&array.ctx(), params()), "built");
}

#[test]
fn df_keeps_grid_methods_on_a_circle_too_wide_for_phase_modes() {
    let mut array = Array::new(kraken(), 5);
    array.tune(12e9);
    let root = DfParams {
        algorithm: DfAlgorithm::RootMusic,
        ..params()
    };
    assert_eq!(refused(&array.ctx(), root), "Circle too wide here");
    assert_eq!(refused(&array.ctx(), params()), "built");
}

#[test]
fn df_is_built_through_the_registry_and_counts_lane_mismatch() {
    let array = Array::new(kraken(), 5);
    let mut processor = create_processor(&array.ctx(), &ProcessorParams::Df(params())).unwrap();
    let lane = vec![Complex::new(0.0f32, 0.0); 64];
    let lanes: Vec<&[Complex<f32>]> = vec![&lane; 4];
    let block = ArrayBlock {
        lanes: &lanes,
        corrected: true,
        correction: CorrectionView::identity(),
        first_index: 0,
        unix_ns: 0,
        generation: 0,
        gap_before: false,
        centers_hz: &array.centers,
        cal: CalView::default(),
        pose: Pose::default(),
    };
    let mut report = ProcessorReading::empty("df");
    let mut out = ProcessorOutput::new(OutputSlots {
        report: report.as_mut(),
        surface: None,
        events: &mut [],
        lanes: &mut [],
    });
    processor.process(&block, &mut out);
    assert!(!out.tally().report);
    assert_eq!(processor.faults().lane_mismatch, 1);
    processor.reset(ResetCause::Gap);
    assert_eq!(processor.faults().resets, 1);
    assert!(processor.action(ProcessorAction::ClearTracks).is_err());
}
