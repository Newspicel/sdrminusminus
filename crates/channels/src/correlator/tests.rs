use num_complex::Complex;
use sdrmm_wire::{ArrayGeometry, SpatialSpectrumOwned, StitchParams};

use super::*;
use crate::array_processor::bench::{Bench, Sink, block, noise};
use crate::array_processor::{OutputTally, create_processor};

const RATE: f64 = 2_400_000.0;
const BLOCK: usize = 24_576;
const NANOS_PER_BLOCK: u64 = 10_240_000;

fn params(settings: CorrelatorParams) -> ProcessorParams {
    ProcessorParams::Correlator(settings)
}

fn quick() -> CorrelatorParams {
    CorrelatorParams {
        integrate_s: 0.05,
        ..CorrelatorParams::default()
    }
}

fn refusal<T>(result: Result<T, ChannelError>) -> Option<String> {
    result.err().map(|error| error.to_string())
}

fn sink() -> Sink {
    let mut sink = Sink::new("correlator", &[]);
    sink.surface = Some(SurfaceFrame::SpatialSpectrum(
        SpatialSpectrumOwned::default(),
    ));
    sink
}

fn delayed_lanes(lanes: usize, len: usize, step: usize) -> Vec<Vec<Complex<f32>>> {
    let reach = step * (lanes - 1);
    let source = noise(len + reach, 17);
    (0..lanes)
        .map(|lane| source[reach - lane * step..reach - lane * step + len].to_vec())
        .collect()
}

fn run(
    processor: &mut CorrelatorProcessor,
    sink: &mut Sink,
    lanes: &[Vec<Complex<f32>>],
) -> Vec<OutputTally> {
    (0..lanes[0].len() / BLOCK)
        .map(|index| {
            let views: Vec<&[Complex<f32>]> = lanes
                .iter()
                .map(|lane| &lane[index * BLOCK..(index + 1) * BLOCK])
                .collect();
            let at = index as u64 * NANOS_PER_BLOCK;
            sink.run(|out| processor.process(&block(&views, at), out))
        })
        .collect()
}

fn reading(sink: &Sink) -> &CorrelatorReading {
    match &sink.report {
        Some(ProcessorReading::Correlator(reading)) => reading,
        other => panic!("a correlator reading, not {other:?}"),
    }
}

fn surface(sink: &Sink) -> &VisibilityOwned {
    match &sink.surface {
        Some(SurfaceFrame::Visibility(frame)) => frame,
        other => panic!("a visibility frame, not {other:?}"),
    }
}

#[test]
fn correlator_reports_ten_baselines_on_five_lanes() {
    let bench = Bench::together("correlator-five", 5, RATE, BLOCK);
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let mut sink = sink();
    let tallies = run(&mut processor, &mut sink, &delayed_lanes(5, 6 * BLOCK, 0));
    assert_eq!(tallies.iter().filter(|tally| tally.report).count(), 1);
    assert!(tallies[4].report && tallies[4].surface);
    let reading = reading(&sink);
    let pairs: Vec<(u32, u32)> = reading.baselines.iter().map(|b| (b.a, b.b)).collect();
    assert_eq!(
        pairs,
        [
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 2),
            (1, 3),
            (1, 4),
            (2, 3),
            (2, 4),
            (3, 4)
        ]
    );
    assert_eq!(reading.frames, 120);
    assert!((reading.integrated_s - 0.0512).abs() < 1e-4);
    assert_eq!(reading.at, "1970-01-01T00:00:00.040Z");
    for baseline in &reading.baselines {
        assert!(baseline.coherence > 0.99, "{baseline:?}");
        assert!(baseline.phase_deg.abs() < 0.5);
        assert!(baseline.snr_db > 40.0);
    }
    assert_eq!(processor.faults(), ProcessorFaults::default());
}

#[test]
fn correlator_delay_readout_in_nanoseconds() {
    let bench = Bench::together("correlator-delay", 3, RATE, BLOCK);
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let mut sink = sink();
    run(&mut processor, &mut sink, &delayed_lanes(3, 6 * BLOCK, 2));
    let reading = reading(&sink);
    let expected = [833.3, 1666.7, 833.3];
    for (baseline, want) in reading.baselines.iter().zip(expected) {
        assert!(
            (baseline.delay_ns - want).abs() < 20.0,
            "{}-{}: {} ns",
            baseline.a,
            baseline.b,
            baseline.delay_ns
        );
    }
}

#[test]
fn correlator_surface_has_channels_per_baseline() {
    let bench = Bench::together("correlator-surface", 5, RATE, BLOCK);
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let mut sink = sink();
    run(&mut processor, &mut sink, &delayed_lanes(5, 6 * BLOCK, 0));
    let frame = surface(&sink);
    assert_eq!(frame.amplitude.len(), 10 * 256);
    assert_eq!(frame.phase.len(), 10 * 256);
    assert_eq!((frame.baselines, frame.bins), (10, 256));
    assert_eq!((frame.db_min, frame.db_max), (-40.0, 0.0));
    assert!((f64::from(frame.span_hz) - RATE).abs() < 1.0);
    assert!((frame.center_hz - bench.center_hz).abs() < f64::EPSILON);
    assert_eq!(frame.timestamp, 40);
    assert!(frame.amplitude.iter().all(|&level| level >= 250));
    assert!(frame.phase.iter().all(|&phase| phase.abs_diff(128) <= 1));
    let independent: Vec<Vec<Complex<f32>>> =
        (0..5).map(|lane| noise(6 * BLOCK, lane + 40)).collect();
    processor.reset(ResetCause::Gap);
    run(&mut processor, &mut sink, &independent);
    let frame = surface(&sink);
    let mean = frame
        .amplitude
        .iter()
        .map(|&level| f64::from(level))
        .sum::<f64>()
        / frame.amplitude.len() as f64;
    assert!(mean < 150.0, "noise stays low: {mean}");
}

#[test]
fn correlator_fringe_band_follows_offset_and_bandwidth() {
    let narrow = CorrelatorParams {
        offset_hz: 300_000.0,
        bandwidth_hz: Some(100_000.0),
        ..quick()
    };
    let bins = fringe_bins(&narrow, RATE).expect("inside the span");
    assert_eq!(bins, 619..662);
    assert_eq!(fringe_bins(&quick(), RATE).expect("whole"), 0..1024);
    let thin = CorrelatorParams {
        bandwidth_hz: Some(1_000.0),
        ..narrow
    };
    assert_eq!(fringe_bins(&thin, RATE).expect("one bin").len(), 1);
    let outside = CorrelatorParams {
        offset_hz: 2_000_000.0,
        ..quick()
    };
    assert_eq!(
        refusal(fringe_bins(&outside, RATE)).as_deref(),
        Some("Offset out of range")
    );
}

#[test]
fn correlator_baselines_carry_the_array_geometry() {
    let bench =
        Bench::together("correlator-geometry", 3, RATE, BLOCK).with_geometry(ArrayGeometry::Ula {
            spacing_m: 0.5,
            axis_deg: 90.0,
        });
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let mut sink = sink();
    run(&mut processor, &mut sink, &delayed_lanes(3, 6 * BLOCK, 0));
    let reading = reading(&sink);
    let lengths: Vec<f32> = reading.baselines.iter().map(|b| b.length_m).collect();
    assert!((lengths[0] - 0.5).abs() < 1e-6 && (lengths[1] - 1.0).abs() < 1e-6);
    assert!((reading.baselines[0].azimuth_deg - 90.0).abs() < 1e-3);
    let south =
        Bench::together("correlator-south", 3, RATE, BLOCK).with_geometry(ArrayGeometry::Ula {
            spacing_m: 0.5,
            axis_deg: 180.0,
        });
    let turned = CorrelatorProcessor::new(&south.ctx(), &params(quick())).expect("build");
    assert!((turned.shapes[2].azimuth_deg - 180.0).abs() < 1e-3);
    assert!((turned.shapes[1].length_m - 1.0).abs() < 1e-6);
}

#[test]
fn correlator_refuses_what_does_not_fit() {
    let bench = Bench::together("correlator-refuse", 16, RATE, BLOCK);
    let wide = CorrelatorParams {
        bins: 8192,
        channels: 1024,
        ..quick()
    };
    assert_eq!(
        refusal(CorrelatorProcessor::new(&bench.ctx(), &params(wide))).as_deref(),
        Some("Channels out of range")
    );
    let spread = Bench::spread("correlator-spread", RATE, &[0.0, 1e6], BLOCK);
    assert_eq!(
        refusal(CorrelatorProcessor::new(&spread.ctx(), &params(quick()))).as_deref(),
        Some("Needs lanes tuned together")
    );
    let stitch = ProcessorParams::Stitch(StitchParams::default());
    assert_eq!(
        refusal(CorrelatorProcessor::new(&bench.ctx(), &stitch)).as_deref(),
        Some("Wrong settings")
    );
    let two = Bench::together("correlator-built", 2, RATE, BLOCK);
    assert!(create_processor(&two.ctx(), &params(quick())).is_ok());
}

#[test]
fn correlator_settings_apply_in_place_or_ask_for_a_rebuild() {
    let bench = Bench::together("correlator-apply", 2, RATE, BLOCK);
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let longer = CorrelatorParams {
        integrate_s: 0.1,
        offset_hz: 100_000.0,
        bandwidth_hz: Some(50_000.0),
        ..quick()
    };
    assert!((DESCRIPTOR.in_place)(&params(quick()), &params(longer)));
    processor.apply(&params(longer)).expect("in place");
    assert_eq!(processor.frames_target, 235);
    let finer = CorrelatorParams {
        bins: 2048,
        ..quick()
    };
    assert!(!(DESCRIPTOR.in_place)(&params(quick()), &params(finer)));
    assert_eq!(
        refusal(processor.apply(&params(finer))).as_deref(),
        Some("Needs a rebuild")
    );
    processor.reset(ResetCause::Gap);
    assert_eq!(processor.faults().resets, 1);
    assert!(processor.action(ProcessorAction::ClearTracks).is_err());
    let moved = Bench::together("correlator-apply", 2, RATE, BLOCK);
    processor.retune(&moved.ctx()).expect("retune");
    let fewer = Bench::together("correlator-apply", 3, RATE, BLOCK);
    assert!(processor.retune(&fewer.ctx()).is_err());
}

#[test]
fn a_lane_mismatch_is_counted_not_silent() {
    let bench = Bench::together("correlator-mismatch", 3, RATE, BLOCK);
    let mut processor = CorrelatorProcessor::new(&bench.ctx(), &params(quick())).expect("build");
    let lane = noise(1_000, 1);
    let short = noise(900, 2);
    let mut sink = sink();
    let tally = sink.run(|out| processor.process(&block(&[&lane, &lane], 0), out));
    assert_eq!(tally, OutputTally::default());
    sink.run(|out| processor.process(&block(&[&lane, &lane, &short], 0), out));
    assert_eq!(processor.faults().lane_mismatch, 2);
}
