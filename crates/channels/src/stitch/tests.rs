use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::stitch::auto_offsets;
use sdrmm_wire::DfParams;

use super::*;
use crate::array_processor::bench::{Bench, Sink, block, noise};
use crate::array_processor::{OutputTally, create_processor};

const RATE: f64 = 2_048.0;
const HOP: usize = STITCH_FFT / 2;
const SECOND: usize = 2 * HOP;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

fn params() -> ProcessorParams {
    ProcessorParams::Stitch(StitchParams::default())
}

fn spread(node: &str, lanes: usize) -> Bench {
    Bench::spread(node, RATE, &auto_offsets(lanes, RATE), 4 * HOP)
}

fn refusal<T>(result: Result<T, ChannelError>) -> Option<String> {
    result.err().map(|error| error.to_string())
}

fn wide_sink(bench: &Bench) -> Sink {
    Sink::new(
        "stitch",
        &[lane_format(&params(), &bench.ctx(), WIDE).capacity],
    )
}

fn tone(len: usize, hz: f64, gain: f32) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| Complex::from_polar(gain, (TAU * hz * n as f64 / RATE) as f32))
        .collect()
}

#[test]
fn stitch_refuses_lanes_tuned_together() {
    let together = Bench::together("stitch-together", 3, RATE, 4 * HOP);
    assert_eq!(
        refusal(StitchProcessor::new(&together.ctx(), &params())).as_deref(),
        Some("Needs spread tuning")
    );
    let bench = spread("stitch-retuned", 3);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("spread lanes");
    assert_eq!(
        refusal(processor.retune(&together.ctx())).as_deref(),
        Some("Needs spread tuning")
    );
}

#[test]
fn stitch_reports_lane_gains_and_spurs() {
    let bench = spread("stitch-report", 2);
    let len = 360 * HOP;
    let mut lanes = [noise(len, 5), noise(len, 13)];
    let offsets = auto_offsets(2, RATE);
    for (sample, spur) in lanes[0].iter_mut().zip(tone(len, -20.0 - offsets[0], 0.29)) {
        *sample += spur;
    }
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let mut sink = wide_sink(&bench);
    for (second, start) in (0..len).step_by(SECOND).enumerate() {
        let views = [
            &lanes[0][start..start + SECOND],
            &lanes[1][start..start + SECOND],
        ];
        let at = second as u64 * NANOS_PER_SECOND;
        let tally = sink.run(|out| processor.process(&block(&views, at), out));
        assert!(tally.report, "second {second}");
        assert_eq!(tally.dropped_reports, 0);
    }
    let Some(ProcessorReading::Stitch(reading)) = &sink.report else {
        panic!("a stitch reading");
    };
    assert_eq!(reading.lanes.len(), 2);
    for (lane, entry) in reading.lanes.iter().enumerate() {
        let state = processor.stitcher.lane_state(lane);
        let expected = StitchLane {
            lane: lane as u32,
            center_hz: bench.centers[lane],
            noise_eq_db: state.gain_db,
            coherence: state.coherence,
            phase_deg: state.phase_deg,
            spur_bins: state.spur_bins,
        };
        assert_eq!(*entry, expected);
    }
    assert_eq!(
        (reading.lanes[0].spur_bins, reading.lanes[1].spur_bins),
        (1, 0)
    );
    assert!((reading.span_hz - 2.0 * RATE).abs() < 1e-9);
    assert!(!reading.no_overlap);
    assert_eq!(reading.dropped_blocks, 0);
    assert_eq!(reading.at, "1970-01-01T00:02:59.000Z");
    assert_eq!(processor.faults(), ProcessorFaults::default());
}

#[test]
fn stitch_counts_a_lane_mismatch() {
    let bench = spread("stitch-mismatch", 3);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let lane = noise(512, 1);
    let views = [&lane[..], &lane[..]];
    let mut sink = wide_sink(&bench);
    let tally = sink.run(|out| processor.process(&block(&views, 0), out));
    assert_eq!(processor.faults().lane_mismatch, 1);
    assert_eq!(tally, OutputTally::default());
    assert!(sink.lanes[0].samples().is_empty());
    assert_eq!(sink.lanes[0].skipped(), 3 * 512);
    let uneven = noise(500, 2);
    let views = [&lane[..], &lane[..], &uneven[..]];
    sink.run(|out| processor.process(&block(&views, 0), out));
    assert_eq!(processor.faults().lane_mismatch, 2);
}

#[test]
fn the_wide_lane_matches_its_lane_format() {
    let bench = spread("stitch-format", 3);
    let format = lane_format(&params(), &bench.ctx(), WIDE);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let offset = processor.stitcher.output_center_offset_hz();
    assert!((format.center_hz - bench.center_hz - offset).abs() < 1e-9);
    assert!((format.sample_rate - 3.0 * RATE).abs() < 1e-9);
    assert_eq!(format.capacity, (bench.max_block + STITCH_FFT) * 3);
    assert_eq!(lane_format(&params(), &bench.ctx(), 1).capacity, 0);
    let lanes: Vec<_> = (0..3)
        .map(|seed| noise(bench.max_block, seed + 1))
        .collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut sink = wide_sink(&bench);
    for _ in 0..3 {
        sink.run(|out| processor.process(&block(&views, 0), out));
        assert_eq!(sink.lanes[0].overflowed(), 0);
        assert!(!sink.lanes[0].samples().is_empty());
    }
    assert!(create_processor(&bench.ctx(), &params()).is_ok());
}

#[test]
fn stitch_settings_apply_in_place() {
    let bench = spread("stitch-apply", 2);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let equal = ProcessorParams::Stitch(StitchParams {
        blend: StitchBlend::Equal,
        spur_reject: false,
        ..StitchParams::default()
    });
    assert!((DESCRIPTOR.in_place)(&params(), &equal));
    assert!(processor.apply(&equal).is_ok());
    let df = ProcessorParams::Df(DfParams::default());
    assert!(!(DESCRIPTOR.in_place)(&params(), &df));
    assert_eq!(
        refusal(processor.apply(&df)).as_deref(),
        Some("Wrong settings")
    );
    assert!(processor.action(ProcessorAction::ClearTracks).is_err());
    processor.reset(ResetCause::Gap);
    assert_eq!(processor.faults().resets, 1);
}

#[test]
fn lanes_without_overlap_run_and_say_so() {
    let bench = Bench::spread("stitch-gap", RATE, &[-0.9 * RATE, 0.9 * RATE], 4 * HOP);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let lanes = [noise(SECOND, 3), noise(SECOND, 4)];
    let views = [&lanes[0][..], &lanes[1][..]];
    let mut sink = wide_sink(&bench);
    let tally = sink.run(|out| processor.process(&block(&views, 0), out));
    assert!(tally.report);
    let Some(ProcessorReading::Stitch(reading)) = &sink.report else {
        panic!("a stitch reading");
    };
    assert!(reading.no_overlap);
}

#[test]
fn stitch_reports_at_the_wire_cadence() {
    let bench = spread("stitch-cadence", 2);
    let mut processor = StitchProcessor::new(&bench.ctx(), &params()).expect("stitch");
    let cadence = (RATE * f64::from(STITCH_REPORT_MS) / 1_000.0) as usize;
    let lanes = [noise(2 * cadence, 5), noise(2 * cadence, 6)];
    let mut sink = wide_sink(&bench);
    let mut reports = Vec::new();
    for start in (0..2 * cadence).step_by(HOP) {
        let views = [&lanes[0][start..start + HOP], &lanes[1][start..start + HOP]];
        let tally = sink.run(|out| processor.process(&block(&views, 0), out));
        if tally.report {
            reports.push(start + HOP);
        }
    }
    assert_eq!(reports, [cadence, 2 * cadence]);
}
