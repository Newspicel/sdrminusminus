use std::f64::consts::TAU;

use sdrmm_dsp::manifold::{Direction, Geometry, Winding as DspWinding};
use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
use sdrmm_wire::{ArrayElement, ArrayGeometry, BeamformerReading, ProcessorReading, Winding};

use super::report::WEIGHT_FLOOR_DB;
use super::*;
use crate::array_processor::bench::{Bench, Sink, block, noise};
use crate::array_processor::{OutputTally, Pose, create_processor};

const RATE: f64 = 960_000.0;
const FREQ: f64 = 433.92e6;
const RADIUS_M: f64 = 0.2939;
const LANES: usize = 5;
const BLOCK: usize = 9_600;
const NANOS_PER_BLOCK: u64 = 10_000_000;
const SOURCE_HZ: f64 = 25_000.0;

fn kraken(node: &str) -> Bench {
    let mut bench = Bench::together(node, LANES, RATE, BLOCK).with_geometry(ArrayGeometry::Uca {
        radius_m: RADIUS_M,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    });
    bench.center_hz = FREQ;
    bench.centers = vec![FREQ; LANES];
    bench
}

fn scene(azimuth: f64, offset_hz: f64, noise_db: f32, seed: u64) -> ArrayScene {
    let geometry = Geometry::uca(RADIUS_M, LANES, 0.0, DspWinding::Clockwise).expect("uca");
    ArrayScene::new(geometry, FREQ, RATE)
        .with_source(SceneSource::new(
            Direction::horizon(azimuth),
            0.0,
            SceneSignal::Tone { offset_hz },
        ))
        .with_noise_db(noise_db)
        .with_seed(seed)
}

fn params(settings: BeamformerParams) -> ProcessorParams {
    ProcessorParams::Beamformer(settings)
}

fn quick(mode: BeamMode) -> BeamformerParams {
    BeamformerParams {
        mode,
        update_ms: 50,
        bandwidth_hz: Some(100_000.0),
        ..BeamformerParams::default()
    }
}

fn fixed(azimuth_deg: f64) -> SteerSource {
    SteerSource::Fixed {
        azimuth_deg,
        elevation_deg: 0.0,
    }
}

fn aimed(relative_deg: f64, wall_ms: u64) -> Steer {
    Steer {
        same_array: true,
        relative_deg,
        wall_ms,
        ..Steer::default()
    }
}

fn refusal<T>(result: Result<T, ChannelError>) -> Option<String> {
    result.err().map(|error| error.to_string())
}

struct Rig {
    processor: BeamformerProcessor,
    sink: Sink,
    blocks: u64,
    heading: Option<f64>,
    beam: Vec<Vec<Complex<f32>>>,
}

impl Rig {
    fn new(bench: &Bench, settings: BeamformerParams) -> Self {
        let params = params(settings);
        let capacity = lane_format(&params, &bench.ctx(), BEAM).capacity;
        let processor = BeamformerProcessor::new(&bench.ctx(), &params).expect("build");
        Self {
            processor,
            sink: Sink::new("beamformer", &[capacity]),
            blocks: 0,
            heading: None,
            beam: Vec::new(),
        }
    }

    fn wall_ms(&self) -> u64 {
        self.blocks * NANOS_PER_BLOCK / 1_000_000
    }

    fn feed(&mut self, lanes: &[Vec<Complex<f32>>]) -> OutputTally {
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut input = block(&views, self.blocks * NANOS_PER_BLOCK);
        input.pose = Pose {
            heading_deg: self.heading,
            ..Pose::default()
        };
        let processor = &mut self.processor;
        let tally = self.sink.run(|out| processor.process(&input, out));
        assert_eq!(self.sink.lanes[0].overflowed(), 0);
        assert_eq!(self.sink.lanes[0].skipped(), 0);
        self.beam.push(self.sink.lanes[0].samples().to_vec());
        self.blocks += 1;
        tally
    }

    fn run(&mut self, scene: &mut ArrayScene, blocks: usize) {
        for _ in 0..blocks {
            let lanes = scene.render(BLOCK).expect("render");
            self.feed(&lanes);
        }
    }

    fn reading(&self) -> &BeamformerReading {
        match &self.sink.report {
            Some(ProcessorReading::Beamformer(reading)) => reading,
            other => panic!("a beamformer reading, not {other:?}"),
        }
    }
}

fn peak_deg(pattern: &[u8]) -> f64 {
    let top = pattern.iter().copied().max().unwrap_or(0);
    let (sin, cos) = pattern
        .iter()
        .enumerate()
        .filter(|(_, level)| **level == top)
        .map(|(index, _)| (index as f64).to_radians().sin_cos())
        .fold((0.0, 0.0), |(sin, cos), (s, c)| (sin + s, cos + c));
    sdrmm_dsp::special::norm_deg(sin.atan2(cos).to_degrees())
}

fn circular_gap(a: f64, b: f64) -> f64 {
    sdrmm_dsp::special::wrap_deg(a - b).abs()
}

fn dc_amplitude(blocks: &[Vec<Complex<f32>>]) -> f64 {
    let samples: Vec<&Complex<f32>> = blocks.iter().flatten().collect();
    let sum: Complex<f64> = samples
        .iter()
        .map(|value| Complex::new(f64::from(value.re), f64::from(value.im)))
        .sum();
    sum.norm() / samples.len() as f64
}

fn scaled(samples: &[Complex<f32>], gain: Complex<f32>) -> Vec<Complex<f32>> {
    samples.iter().map(|value| value * gain).collect()
}

#[test]
fn beamformer_canceller_nulls_two_interferers_on_five_lanes() {
    let bench = kraken("bf-cancel");
    let settings = BeamformerParams {
        mode: BeamMode::Canceller,
        offset_hz: SOURCE_HZ,
        bandwidth_hz: Some(20_000.0),
        update_ms: 50,
        ..BeamformerParams::default()
    };
    let blocks = 40;
    let len = blocks * BLOCK;
    let interferers = [
        scaled(&noise(len, 101), Complex::new(31.6, 0.0)),
        scaled(&noise(len, 202), Complex::new(0.0, 31.6)),
    ];
    let wanted: Vec<Complex<f32>> = (0..len)
        .map(|n| Complex::from_polar(0.05, (TAU * SOURCE_HZ * n as f64 / RATE) as f32))
        .collect();
    let clean: Vec<Complex<f32>> = wanted
        .iter()
        .zip(noise(len, 300))
        .map(|(tone, hiss)| tone + hiss)
        .collect();
    let lanes: Vec<Vec<Complex<f32>>> = (0..LANES)
        .map(|lane| {
            let hiss = noise(len, 300 + lane as u32);
            (0..len)
                .map(|n| {
                    let a = Complex::from_polar(1.0 + 0.1 * lane as f32, 0.9 * lane as f32);
                    let b = Complex::from_polar(1.2 - 0.1 * lane as f32, -1.7 * lane as f32);
                    let own = if lane == 0 {
                        wanted[n]
                    } else {
                        Complex::new(0.0, 0.0)
                    };
                    a * interferers[0][n] + b * interferers[1][n] + hiss[n] + own
                })
                .collect()
        })
        .collect();
    let mut rig = Rig::new(&bench, settings);
    let mut reference = LaneBand::new(1, RATE, SOURCE_HZ, Some(20_000.0), BLOCK).expect("band");
    let mut kept = Vec::new();
    for index in 0..blocks {
        let range = index * BLOCK..(index + 1) * BLOCK;
        let slice: Vec<Vec<Complex<f32>>> = lanes
            .iter()
            .map(|lane| lane[range.clone()].to_vec())
            .collect();
        rig.feed(&slice);
        let lane = [&clean[range]];
        let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let n = reference.process(&block(&lane, 0), &mut views);
        kept.push(views[0][..n].to_vec());
    }
    let reading = rig.reading();
    let cancelled = reading.cancelled_db.expect("cancelled readout");
    assert!(cancelled > 25.0, "cancelled {cancelled} dB");
    let output = dc_amplitude(&rig.beam[blocks - 20..]);
    let expected = dc_amplitude(&kept[blocks - 20..]);
    let error_db = 20.0 * (output / expected).log10();
    assert!(error_db.abs() < 1.0, "wanted signal moved {error_db} dB");
    assert!(!reading.singular && !reading.no_steer);
    assert_eq!(reading.weights.len(), LANES);
}

#[test]
fn beamformer_mvdr_follows_the_wired_steer() {
    let bench = kraken("bf-mvdr");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mvdr));
    for (azimuth, seed) in [(137.0, 3), (40.0, 4)] {
        rig.processor.steer(&aimed(azimuth, rig.wall_ms()));
        rig.run(&mut scene(azimuth, SOURCE_HZ, 0.0, seed), 60);
        let reading = rig.reading();
        assert!(!reading.no_steer && !reading.steer_stale && !reading.singular);
        assert_eq!(reading.steer_deg, Some(azimuth as f32));
        let peak = peak_deg(&reading.pattern);
        assert!(
            circular_gap(peak, azimuth) < 15.0,
            "peak {peak} for {azimuth}"
        );
        assert!(reading.pattern[azimuth as usize] >= 250);
        let gain = reading.sinr_gain_db.expect("gain readout");
        assert!(gain > 6.0, "gain {gain} dB at {azimuth}");
        assert!(reading.loading_used > 0.0);
    }
}

#[test]
fn beamformer_steer_from_another_array_uses_true_bearings() {
    let bench = kraken("bf-true");
    let mut rig = Rig::new(&bench, quick(BeamMode::Das));
    rig.heading = Some(90.0);
    rig.processor.steer(&Steer {
        same_array: false,
        relative_deg: 999.0,
        true_deg: Some(200.0),
        ..Steer::default()
    });
    rig.run(&mut scene(110.0, SOURCE_HZ, 0.0, 5), 10);
    let reading = rig.reading();
    assert_eq!(reading.steer_deg, Some(110.0));
    assert!(!reading.no_steer);
    rig.heading = None;
    rig.run(&mut scene(110.0, SOURCE_HZ, 0.0, 6), 5);
    assert!(rig.reading().no_steer);
}

#[test]
fn beamformer_without_steer_passes_the_main_lane_and_says_so() {
    let bench = kraken("bf-unsteered");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mvdr));
    let mut band = LaneBand::new(LANES, RATE, 0.0, Some(100_000.0), BLOCK).expect("band");
    let mut source = scene(137.0, SOURCE_HZ, 0.0, 7);
    for _ in 0..12 {
        let lanes = source.render(BLOCK).expect("render");
        rig.feed(&lanes);
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut picked: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let n = band.process(&block(&views, 0), &mut picked);
        assert_eq!(rig.beam.last().map(Vec::as_slice), Some(&picked[0][..n]));
    }
    let reading = rig.reading();
    assert!(reading.no_steer);
    assert_eq!(reading.steer_deg, None);
    assert_eq!(reading.weights[0].amplitude_db, 0.0);
    assert!(
        reading.weights[1..]
            .iter()
            .all(|w| w.amplitude_db == WEIGHT_FLOOR_DB)
    );
}

#[test]
fn beamformer_stale_steer_holds_weights_and_flags() {
    let bench = kraken("bf-stale");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mvdr));
    rig.processor.steer(&aimed(137.0, 0));
    let mut source = scene(137.0, SOURCE_HZ, 0.0, 8);
    rig.run(&mut source, 20);
    let fresh = rig.reading().clone();
    assert!(!fresh.steer_stale);
    assert!(fresh.steer_age_ms.is_some_and(|age| age < 1_000));
    rig.blocks = 600;
    rig.run(&mut source, 10);
    let stale = rig.reading();
    assert!(stale.steer_stale);
    assert!(stale.steer_age_ms.is_some_and(|age| age > 5_000));
    assert_eq!(stale.weights, fresh.weights);
    assert_eq!(stale.steer_deg, Some(137.0));
    assert!(rig.beam.last().is_some_and(|beam| !beam.is_empty()));
}

#[test]
fn beamformer_singular_covariance_keeps_weights_and_reports() {
    let bench = kraken("bf-singular");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mrc));
    let silence = vec![vec![Complex::new(0.0f32, 0.0); BLOCK]; LANES];
    for _ in 0..12 {
        rig.feed(&silence);
    }
    let reading = rig.reading();
    assert!(reading.singular);
    assert!(rig.processor.faults().solver_failures > 0);
    assert_eq!(reading.weights[0].amplitude_db, 0.0);
    assert!(
        reading.weights[1..]
            .iter()
            .all(|w| w.amplitude_db == WEIGHT_FLOOR_DB)
    );
    assert!(reading.output_db.is_finite());
}

#[test]
fn beamformer_output_lane_rate_and_centre_match_the_band() {
    let bench = kraken("bf-format");
    let settings = BeamformerParams {
        offset_hz: 50_000.0,
        ..quick(BeamMode::Mrc)
    };
    let format = lane_format(&params(settings.clone()), &bench.ctx(), BEAM);
    let band = LaneBand::new(LANES, RATE, 50_000.0, Some(100_000.0), BLOCK).expect("band");
    assert!((format.sample_rate - band.output_rate()).abs() < 1e-9);
    assert!((format.center_hz - FREQ - 50_000.0).abs() < 1e-6);
    let full = BeamformerParams {
        bandwidth_hz: None,
        ..settings.clone()
    };
    let wide = lane_format(&params(full), &bench.ctx(), BEAM);
    assert!((wide.sample_rate - RATE).abs() < f64::EPSILON);
    assert!((wide.center_hz - FREQ).abs() < f64::EPSILON);
    assert_eq!(
        lane_format(&params(settings.clone()), &bench.ctx(), 1).capacity,
        0
    );
    let mut rig = Rig::new(&bench, settings);
    rig.run(&mut scene(137.0, 50_000.0, 0.0, 9), 10);
    let reading = rig.reading();
    assert!((reading.out_rate - band.output_rate()).abs() < 1e-9);
    assert!((reading.out_center_hz - format.center_hz).abs() < 1e-6);
    let produced: usize = rig.beam.iter().map(Vec::len).sum();
    assert!(produced.abs_diff(10 * BLOCK / 7) <= 2, "{produced}");
}

fn switch_steps(crossfade_ms: u32) -> (f32, f32) {
    let bench = kraken("bf-crossfade");
    let settings = BeamformerParams {
        steer: fixed(137.0),
        crossfade_ms,
        ..quick(BeamMode::Das)
    };
    let mut rig = Rig::new(&bench, settings.clone());
    let mut source = scene(137.0, 2_000.0, -100.0, 10);
    rig.run(&mut source, 20);
    let moved = BeamformerParams {
        steer: fixed(250.0),
        ..settings
    };
    rig.processor.apply(&params(moved)).expect("in place");
    rig.run(&mut source, 20);
    let steps = |blocks: &[Vec<Complex<f32>>]| -> Vec<f32> {
        let samples: Vec<Complex<f32>> = blocks.concat();
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).norm())
            .collect()
    };
    let mut before = steps(&rig.beam[10..20]);
    before.sort_by(f32::total_cmp);
    let typical = before[before.len() / 2];
    let largest = steps(&rig.beam[19..40]).into_iter().fold(0.0, f32::max);
    (largest, typical)
}

#[test]
fn beamformer_crossfade_has_no_step() {
    let (largest, typical) = switch_steps(20);
    assert!(
        largest < 3.0 * typical,
        "step {largest} vs typical {typical}"
    );
    let (jump, usual) = switch_steps(0);
    assert!(
        jump > 3.0 * usual,
        "without a crossfade the switch must show: {jump} vs {usual}"
    );
}

#[test]
fn beamformer_refuses_a_wideband_canceller_too_heavy_for_the_band() {
    let bench = Bench::together("bf-heavy", LANES, 2_400_000.0, 16_384);
    let heavy = BeamformerParams {
        mode: BeamMode::Canceller,
        taps: 32,
        adaptation: Adaptation::Rls,
        bandwidth_hz: None,
        ..BeamformerParams::default()
    };
    assert_eq!(
        refusal(BeamformerProcessor::new(
            &bench.ctx(),
            &params(heavy.clone())
        ))
        .as_deref(),
        Some("Too heavy for this band")
    );
    assert_eq!(
        refusal(create_processor(&bench.ctx(), &params(heavy))).as_deref(),
        Some("Too heavy for this band")
    );
    let light = BeamformerParams {
        mode: BeamMode::Canceller,
        taps: 8,
        bandwidth_hz: Some(200_000.0),
        ..BeamformerParams::default()
    };
    assert!(BeamformerProcessor::new(&bench.ctx(), &params(light)).is_ok());
}

#[test]
fn beamformer_auto_nulls_follow_the_df_other_peaks() {
    let bench = kraken("bf-nulls");
    let settings = BeamformerParams {
        auto_nulls: true,
        ..quick(BeamMode::Lcmv)
    };
    let mut rig = Rig::new(&bench, settings);
    rig.processor.steer(&Steer {
        others_relative_deg: [30.0, 0.0, 0.0],
        others: 1,
        ..aimed(137.0, 0)
    });
    rig.run(&mut scene(137.0, SOURCE_HZ, 0.0, 11), 10);
    let reading = rig.reading();
    assert_eq!(reading.nulls_deg, [30.0]);
    let depth = reading.null_depths_db[0];
    assert!(depth < -30.0, "null depth {depth} dB");
    assert!(
        reading.pattern[30] < 60,
        "pattern at the null {}",
        reading.pattern[30]
    );
}

#[test]
fn beamformer_drops_a_null_inside_the_main_lobe() {
    let bench = kraken("bf-lobe");
    let settings = BeamformerParams {
        steer: fixed(137.0),
        nulls_deg: vec![140.0, 300.0],
        ..quick(BeamMode::Lcmv)
    };
    let mut rig = Rig::new(&bench, settings);
    rig.run(&mut scene(137.0, SOURCE_HZ, 0.0, 12), 10);
    assert_eq!(rig.reading().nulls_deg, [300.0]);
}

#[test]
fn beamformer_block_and_blind_modes_point_at_the_source() {
    for mode in [BeamMode::Mrc, BeamMode::Das, BeamMode::Cma] {
        let bench = kraken("bf-modes");
        let settings = BeamformerParams {
            steer: fixed(137.0),
            ..quick(mode)
        };
        let mut rig = Rig::new(&bench, settings);
        rig.run(&mut scene(137.0, SOURCE_HZ, 0.0, 13), 30);
        let reading = rig.reading();
        assert_eq!(reading.mode, mode);
        assert!(!reading.diverged && !reading.singular, "{mode:?}");
        assert_eq!(reading.resets, 0);
        assert_eq!(reading.weights.len(), LANES);
        assert_eq!(reading.pattern.len(), 360);
        let peak = peak_deg(&reading.pattern);
        assert!(circular_gap(peak, 137.0) < 15.0, "{mode:?} peaks at {peak}");
        assert_eq!(rig.processor.faults(), ProcessorFaults::default());
    }
}

#[test]
fn beamformer_gsc_nulls_an_interferer_and_keeps_the_steer() {
    let bench = kraken("bf-gsc");
    let settings = BeamformerParams {
        steer: fixed(137.0),
        ..quick(BeamMode::Gsc)
    };
    let mut rig = Rig::new(&bench, settings);
    let mut source = scene(137.0, SOURCE_HZ, 0.0, 15).with_source(SceneSource::new(
        Direction::horizon(30.0),
        20.0,
        SceneSignal::Noise {
            offset_hz: -10_000.0,
            bandwidth_hz: 20_000.0,
        },
    ));
    rig.run(&mut source, 60);
    let reading = rig.reading();
    assert!(!reading.diverged && reading.resets == 0);
    assert_eq!(reading.steer_deg, Some(137.0));
    let mut weights = WeightSet::zeros(LANES);
    rig.processor.core.gsc.weights(&mut weights);
    let mut steer = [Complex::new(0.0f32, 0.0); LANES];
    let mut interferer = [Complex::new(0.0f32, 0.0); LANES];
    let manifold = rig.processor.core.manifold.as_ref().expect("manifold");
    manifold.steer(FREQ, Direction::horizon(137.0), &mut steer);
    manifold.steer(FREQ, Direction::horizon(30.0), &mut interferer);
    assert!((weights.response(&steer) - Complex::new(1.0, 0.0)).norm() < 1e-3);
    let leak = 20.0 * weights.response(&interferer).norm().log10();
    assert!(leak < -20.0, "interferer leaks at {leak} dB");
    assert!(reading.pattern[30] + 100 < reading.pattern[137]);
}

#[test]
fn beamformer_wideband_canceller_reports_its_suppression() {
    let bench = kraken("bf-tdl");
    let settings = BeamformerParams {
        mode: BeamMode::Canceller,
        taps: 4,
        ..quick(BeamMode::Canceller)
    };
    let mut rig = Rig::new(&bench, settings);
    rig.run(&mut scene(137.0, SOURCE_HZ, -20.0, 14), 30);
    let reading = rig.reading();
    assert!(reading.weights.is_empty() && reading.pattern.is_empty());
    assert!(
        reading.cancelled_db.is_some_and(|db| db > 10.0),
        "{reading:?}"
    );
}

#[test]
fn beamformer_settings_apply_in_place_or_ask_for_a_rebuild() {
    let bench = kraken("bf-apply");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mvdr));
    let das = BeamformerParams {
        steer: fixed(90.0),
        loading: 0.5,
        crossfade_ms: 0,
        ..quick(BeamMode::Das)
    };
    assert!((DESCRIPTOR.in_place)(
        &params(quick(BeamMode::Mvdr)),
        &params(das.clone())
    ));
    rig.processor.apply(&params(das)).expect("in place");
    let mrc = quick(BeamMode::Mrc);
    assert!(!(DESCRIPTOR.in_place)(
        &params(quick(BeamMode::Mvdr)),
        &params(mrc.clone())
    ));
    assert_eq!(
        refusal(rig.processor.apply(&params(mrc))).as_deref(),
        Some("Needs a rebuild")
    );
    let narrow = BeamformerParams {
        bandwidth_hz: Some(50_000.0),
        ..quick(BeamMode::Das)
    };
    assert_eq!(
        refusal(rig.processor.apply(&params(narrow))).as_deref(),
        Some("Needs a rebuild")
    );
    rig.processor.retune(&bench.ctx()).expect("retune");
    rig.processor.reset(ResetCause::Gap);
    assert_eq!(rig.processor.faults().resets, 1);
    assert!(rig.processor.action(ProcessorAction::ClearTracks).is_err());
}

#[test]
fn beamformer_refuses_lanes_and_geometry_it_cannot_use() {
    let bench = kraken("bf-refuse");
    let far = BeamformerParams {
        mode: BeamMode::Canceller,
        main_lane: 5,
        ..quick(BeamMode::Canceller)
    };
    assert_eq!(
        refusal(BeamformerProcessor::new(&bench.ctx(), &params(far))).as_deref(),
        Some("Lane out of range")
    );
    let odd = Bench::together("bf-odd", 3, RATE, BLOCK).with_geometry(ArrayGeometry::Explicit {
        positions: vec![ArrayElement::default(); 2],
    });
    assert_eq!(
        refusal(BeamformerProcessor::new(
            &odd.ctx(),
            &params(quick(BeamMode::Mvdr))
        ))
        .as_deref(),
        Some("Needs array geometry")
    );
    assert!(BeamformerProcessor::new(&odd.ctx(), &params(quick(BeamMode::Mrc))).is_ok());
    let spread = Bench::spread("bf-spread", RATE, &[0.0, 1e5], BLOCK);
    assert_eq!(
        refusal(BeamformerProcessor::new(
            &spread.ctx(),
            &params(quick(BeamMode::Mrc))
        ))
        .as_deref(),
        Some("Needs lanes tuned together")
    );
}

#[test]
fn a_lane_count_mismatch_is_counted_not_silent() {
    let bench = kraken("bf-mismatch");
    let mut rig = Rig::new(&bench, quick(BeamMode::Mrc));
    let lane = noise(BLOCK, 1);
    let processor = &mut rig.processor;
    let tally = rig
        .sink
        .run(|out| processor.process(&block(&[&lane, &lane], 0), out));
    assert_eq!(tally, OutputTally::default());
    assert_eq!(rig.processor.faults().lane_mismatch, 1);
    assert_eq!(rig.sink.lanes[0].skipped(), 1_371);
}

#[test]
fn beamformer_apply_agrees_with_the_descriptor() {
    let bench = kraken("bf-agree");
    let full = BeamformerParams {
        bandwidth_hz: None,
        ..quick(BeamMode::Mrc)
    };
    let cases = [
        (
            BeamformerParams {
                offset_hz: 30_000.0,
                ..full.clone()
            },
            true,
        ),
        (
            BeamformerParams {
                mode: BeamMode::Cma,
                ..full.clone()
            },
            true,
        ),
        (
            BeamformerParams {
                mode: BeamMode::Canceller,
                ..full.clone()
            },
            false,
        ),
        (
            BeamformerParams {
                offset_hz: 30_000.0,
                ..quick(BeamMode::Mrc)
            },
            false,
        ),
    ];
    for (next, expected) in cases {
        let mut processor =
            BeamformerProcessor::new(&bench.ctx(), &params(full.clone())).expect("build");
        let in_place = (DESCRIPTOR.in_place)(&params(full.clone()), &params(next.clone()));
        assert_eq!(in_place, expected, "{next:?}");
        assert_eq!(
            processor.apply(&params(next.clone())).is_ok(),
            in_place,
            "{next:?}"
        );
    }
}

#[test]
fn beamformer_losing_the_steer_drops_its_nulls() {
    let bench = kraken("bf-lost");
    let settings = BeamformerParams {
        nulls_deg: vec![300.0],
        ..quick(BeamMode::Lcmv)
    };
    let mut rig = Rig::new(&bench, settings);
    rig.processor.steer(&aimed(137.0, 0));
    rig.run(&mut scene(137.0, SOURCE_HZ, 0.0, 16), 10);
    let aimed_reading = rig.reading();
    assert_eq!(aimed_reading.nulls_deg, [300.0]);
    assert_eq!(aimed_reading.null_depths_db.len(), 1);
    assert!(aimed_reading.loading_used > 0.0);
    rig.processor.steer(&Steer {
        same_array: false,
        true_deg: Some(200.0),
        wall_ms: rig.wall_ms(),
        ..Steer::default()
    });
    rig.run(&mut scene(137.0, SOURCE_HZ, 0.0, 17), 10);
    let lost = rig.reading();
    assert!(lost.no_steer);
    assert!(lost.nulls_deg.is_empty() && lost.null_depths_db.is_empty());
    assert!(lost.loading_used.abs() < f32::EPSILON);
    assert_eq!(lost.weights[0].amplitude_db, 0.0);
}
