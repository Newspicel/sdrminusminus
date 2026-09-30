use std::f64::consts::TAU;

use sdrmm_wire::StitchParams;

use super::*;
use crate::array_processor::bench::{Bench, Sink, block, noise};
use crate::array_processor::create_processor;

const RATE: f64 = 240_000.0;
const OFFSET_HZ: f64 = 30_000.0;
const BLOCK: usize = 12_000;
const BLOCKS: usize = 40;
const NANOS_PER_BLOCK: u64 = 50_000_000;
const WEAK: f32 = 0.6;
const STRONG: f32 = 6.0;

fn params(settings: PolarimeterParams) -> ProcessorParams {
    ProcessorParams::Polarimeter(settings)
}

fn polar(h_lane: u32, v_lane: u32) -> PolarimeterParams {
    PolarimeterParams {
        h_lane,
        v_lane,
        offset_hz: OFFSET_HZ,
        ..PolarimeterParams::default()
    }
}

fn refusal<T>(result: Result<T, ChannelError>) -> Option<String> {
    result.err().map(|error| error.to_string())
}

fn wave(
    len: usize,
    angle_deg: f64,
    phase_deg: f64,
    amplitude: f32,
    seed: u32,
) -> [Vec<Complex<f32>>; 2] {
    let (h_noise, v_noise) = (noise(len, seed), noise(len, seed + 1));
    let h_gain = amplitude * angle_deg.to_radians().cos() as f32;
    let v_gain = Complex::from_polar(
        amplitude * angle_deg.to_radians().sin() as f32,
        phase_deg.to_radians() as f32,
    );
    let tone = |n: usize| Complex::from_polar(1.0f32, (TAU * OFFSET_HZ * n as f64 / RATE) as f32);
    let h = (0..len).map(|n| tone(n) * h_gain + h_noise[n]).collect();
    let v = (0..len).map(|n| tone(n) * v_gain + v_noise[n]).collect();
    [h, v]
}

struct Run {
    reading: PolarimeterReading,
    beam: Vec<Vec<Complex<f32>>>,
}

fn run(bench: &Bench, settings: PolarimeterParams, lanes: &[Vec<Complex<f32>>]) -> Run {
    let params = params(settings);
    let capacity = lane_format(&params, &bench.ctx(), BEAM).capacity;
    let mut processor = PolarimeterProcessor::new(&bench.ctx(), &params).expect("build");
    let mut sink = Sink::new("polarimeter", &[capacity]);
    let mut beam = Vec::new();
    for index in 0..lanes[0].len() / BLOCK {
        let views: Vec<&[Complex<f32>]> = lanes
            .iter()
            .map(|lane| &lane[index * BLOCK..(index + 1) * BLOCK])
            .collect();
        let at = index as u64 * NANOS_PER_BLOCK;
        sink.run(|out| processor.process(&block(&views, at), out));
        assert_eq!(sink.lanes[0].overflowed(), 0);
        beam.push(sink.lanes[0].samples().to_vec());
    }
    assert_eq!(processor.faults(), ProcessorFaults::default());
    let Some(ProcessorReading::Polarimeter(reading)) = sink.report else {
        panic!("a polarimeter reading");
    };
    Run { reading, beam }
}

fn tail(beam: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
    beam[beam.len() - 4..].concat()
}

fn snr_db(samples: &[Complex<f32>]) -> f64 {
    let len = samples.len() as f64;
    let mean: Complex<f64> = samples
        .iter()
        .map(|value| Complex::new(f64::from(value.re), f64::from(value.im)))
        .sum::<Complex<f64>>()
        / len;
    let spread: f64 = samples
        .iter()
        .map(|value| (Complex::new(f64::from(value.re), f64::from(value.im)) - mean).norm_sqr())
        .sum::<f64>()
        / len;
    10.0 * (mean.norm_sqr() / spread).log10()
}

fn power(samples: &[Complex<f32>]) -> f64 {
    samples
        .iter()
        .map(|value| f64::from(value.norm_sqr()))
        .sum::<f64>()
        / samples.len() as f64
}

fn banded(lanes: &[Vec<Complex<f32>>]) -> Vec<Vec<Complex<f32>>> {
    let mut band = LaneBand::new(2, RATE, OFFSET_HZ, Some(20_000.0), BLOCK).expect("band");
    let mut out = vec![Vec::new(), Vec::new()];
    for index in 0..lanes[0].len() / BLOCK {
        let views: Vec<&[Complex<f32>]> = lanes
            .iter()
            .map(|lane| &lane[index * BLOCK..(index + 1) * BLOCK])
            .collect();
        let mut picked: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let n = band.process(&block(&views, 0), &mut picked);
        if index >= BLOCKS - 4 {
            for (lane, view) in out.iter_mut().zip(&picked) {
                lane.extend_from_slice(&view[..n]);
            }
        }
    }
    out
}

#[test]
fn polarimeter_reads_a_45_degree_linear_wave() {
    let bench = Bench::together("polar-45", 2, RATE, BLOCK);
    let lanes = wave(BLOCKS * BLOCK, 45.0, 0.0, STRONG, 3);
    let reading = run(&bench, polar(0, 1), &lanes).reading;
    assert!((reading.angle_deg - 45.0).abs() < 1.0, "{reading:?}");
    assert!(reading.degree > 0.95, "{reading:?}");
    assert!(reading.u > 0.95 && reading.q.abs() < 0.05);
    assert_eq!(reading.hand, Hand::Linear);
    assert!(reading.snr_db.is_some_and(|snr| snr > 25.0), "{reading:?}");
    assert_eq!(reading.at, "1970-01-01T00:00:01.950Z");
}

#[test]
fn polarimeter_matched_output_gains_on_crossed_dipoles() {
    let bench = Bench::together("polar-gain", 2, RATE, BLOCK);
    let lanes = wave(BLOCKS * BLOCK, 45.0, 0.0, WEAK, 5);
    let output = snr_db(&tail(&run(&bench, polar(0, 1), &lanes).beam));
    let best = banded(&lanes)
        .iter()
        .map(|lane| snr_db(lane))
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        output >= best + 2.5,
        "output {output} dB, best lane {best} dB"
    );
}

#[test]
fn polarimeter_orthogonal_output_nulls_the_wave() {
    let bench = Bench::together("polar-null", 2, RATE, BLOCK);
    let lanes = wave(BLOCKS * BLOCK, 30.0, 20.0, STRONG, 7);
    let matched = power(&tail(&run(&bench, polar(0, 1), &lanes).beam));
    let orthogonal = PolarimeterParams {
        matched: false,
        ..polar(0, 1)
    };
    let nulled = power(&tail(&run(&bench, orthogonal, &lanes).beam));
    let relative = 10.0 * (nulled / matched).log10();
    assert!(relative < -20.0, "orthogonal output {relative} dB");
}

#[test]
fn polarimeter_refuses_equal_lanes() {
    let bench = Bench::together("polar-refuse", 2, RATE, BLOCK);
    assert_eq!(
        refusal(PolarimeterProcessor::new(
            &bench.ctx(),
            &params(polar(1, 1))
        ))
        .as_deref(),
        Some("Lanes must differ")
    );
    assert_eq!(
        refusal(create_processor(&bench.ctx(), &params(polar(1, 1)))).as_deref(),
        Some("Lanes must differ")
    );
    assert_eq!(
        refusal(PolarimeterProcessor::new(
            &bench.ctx(),
            &params(polar(0, 2))
        ))
        .as_deref(),
        Some("Lane out of range")
    );
    let far = PolarimeterParams {
        offset_hz: 200_000.0,
        ..polar(0, 1)
    };
    assert_eq!(
        refusal(PolarimeterProcessor::new(&bench.ctx(), &params(far))).as_deref(),
        Some("Offset out of range")
    );
    let spread = Bench::spread("polar-spread", RATE, &[0.0, 50_000.0], BLOCK);
    assert_eq!(
        refusal(PolarimeterProcessor::new(
            &spread.ctx(),
            &params(polar(0, 1))
        ))
        .as_deref(),
        Some("Needs lanes tuned together")
    );
    let stitch = ProcessorParams::Stitch(StitchParams::default());
    assert_eq!(
        refusal(PolarimeterProcessor::new(&bench.ctx(), &stitch)).as_deref(),
        Some("Wrong settings")
    );
}

#[test]
fn polarimeter_hand_follows_the_feed_phase_and_the_flip() {
    let bench = Bench::together("polar-hand", 2, RATE, BLOCK);
    let lanes = wave(BLOCKS * BLOCK, 45.0, -90.0, STRONG, 9);
    let right = run(&bench, polar(0, 1), &lanes).reading;
    assert_eq!(right.hand, Hand::Right);
    assert!(right.v > 0.95 && (right.ellipticity_deg - 45.0).abs() < 1.0);
    let flipped = PolarimeterParams {
        flip_hand: true,
        ..polar(0, 1)
    };
    assert_eq!(run(&bench, flipped, &lanes).reading.hand, Hand::Left);
}

#[test]
fn polarimeter_picks_its_lanes_from_a_wider_array() {
    let bench = Bench::together("polar-picked", 4, RATE, BLOCK);
    let [h, v] = wave(BLOCKS * BLOCK, 60.0, 0.0, STRONG, 11);
    let lanes = vec![noise(h.len(), 21), v, noise(h.len(), 23), h];
    let reading = run(&bench, polar(3, 1), &lanes).reading;
    assert!((reading.angle_deg - 60.0).abs() < 1.0, "{reading:?}");
}

#[test]
fn polarimeter_beam_lane_matches_the_band() {
    let bench = Bench::together("polar-format", 2, RATE, BLOCK);
    let format = lane_format(&params(polar(0, 1)), &bench.ctx(), BEAM);
    assert!((format.center_hz - bench.center_hz - OFFSET_HZ).abs() < 1e-6);
    assert!((format.sample_rate - RATE / 9.0).abs() < 1e-6);
    assert_eq!(
        lane_format(&params(polar(0, 1)), &bench.ctx(), 1).capacity,
        0
    );
    let lanes = wave(BLOCKS * BLOCK, 45.0, 0.0, STRONG, 13);
    let run = run(&bench, polar(0, 1), &lanes);
    assert!((run.reading.out_center_hz - format.center_hz).abs() < 1e-6);
    assert!((run.reading.out_rate - format.sample_rate).abs() < 1e-6);
    let produced: usize = run.beam.iter().map(Vec::len).sum();
    assert!(produced.abs_diff(BLOCKS * BLOCK / 9) <= 2, "{produced}");
}

#[test]
fn polarimeter_settings_apply_in_place() {
    let bench = Bench::together("polar-apply", 2, RATE, BLOCK);
    let mut processor =
        PolarimeterProcessor::new(&bench.ctx(), &params(polar(0, 1))).expect("build");
    let slower = PolarimeterParams {
        report_ms: 1_000,
        matched: false,
        flip_hand: true,
        ..polar(0, 1)
    };
    assert!((DESCRIPTOR.in_place)(&params(polar(0, 1)), &params(slower)));
    processor.apply(&params(slower)).expect("in place");
    let swapped = polar(1, 0);
    assert!(!(DESCRIPTOR.in_place)(
        &params(polar(0, 1)),
        &params(swapped)
    ));
    assert_eq!(
        refusal(processor.apply(&params(swapped))).as_deref(),
        Some("Needs a rebuild")
    );
    processor.retune(&bench.ctx()).expect("retune");
    processor.reset(ResetCause::Gap);
    assert_eq!(processor.faults().resets, 1);
    let lane = noise(900, 1);
    let mut sink = Sink::new("polarimeter", &[1_000]);
    sink.run(|out| processor.process(&block(&[&lane], 0), out));
    assert_eq!(processor.faults().lane_mismatch, 1);
    assert_eq!(sink.lanes[0].skipped(), 100);
}
