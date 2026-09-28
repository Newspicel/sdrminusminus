use num_complex::Complex;
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, LaneBuffer, OutputSlots, Pose,
    ProcessorOutput, ResetCause, create_processor, processor_descriptor,
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, Coherence, PolarimeterParams, ProcessorParams, ProcessorReading,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 3;
const RATE: f64 = 2_400_000.0;
const MAX_BLOCK: usize = 16_384;
const CENTER_HZ: f64 = 433.92e6;

fn noise(len: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32 - 0.5
    };
    (0..len)
        .map(|n| Complex::new(next(), next()) + Complex::from_polar(2.0, 0.05 * n as f32))
        .collect()
}

fn ctx<'a>(centers: &'a [f64], geometry: &'a ArrayGeometry) -> ArrayCtx<'a> {
    ArrayCtx {
        node: "polarimeter-alloc",
        lanes: centers.len(),
        sample_rate: RATE,
        center_hz: centers[0],
        lane_centers_hz: centers,
        geometry,
        positions_m: &[],
        manifold: None,
        tier: Coherence::PhaseCoherent,
        tuning: ArrayTuningMode::Together,
        max_block: MAX_BLOCK,
    }
}

fn block<'a>(
    lanes: &'a [&'a [Complex<f32>]],
    unix_ns: u64,
    correction: CorrectionView<'a>,
) -> ArrayBlock<'a> {
    ArrayBlock {
        lanes,
        corrected: correction.generation == 0,
        correction,
        first_index: 0,
        unix_ns,
        generation: correction.generation,
        gap_before: false,
        centers_hz: &[],
        cal: CalView::default(),
        pose: Pose::default(),
    }
}

struct Host {
    report: Option<ProcessorReading>,
    beam: [LaneBuffer; 1],
    reports: u32,
}

impl Host {
    fn step(
        &mut self,
        processor: &mut dyn ArrayProcessor,
        lanes: &[&[Complex<f32>]],
        unix_ns: u64,
        correction: CorrectionView<'_>,
    ) {
        self.beam[0].clear();
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: None,
            events: &mut [],
            lanes: &mut self.beam,
        });
        processor.process(&block(lanes, unix_ns, correction), &mut out);
        self.reports += u32::from(out.tally().report);
    }
}

mod polarimeter {
    use super::*;

    #[test]
    fn polarimeter_process_does_not_allocate() {
        let geometry = ArrayGeometry::default();
        let centers = vec![CENTER_HZ; LANES];
        let moved = vec![CENTER_HZ + 2e5; LANES];
        let settings = PolarimeterParams {
            h_lane: 2,
            v_lane: 0,
            offset_hz: 25_000.0,
            report_ms: 50,
            ..PolarimeterParams::default()
        };
        let params = ProcessorParams::Polarimeter(settings);
        let orthogonal = ProcessorParams::Polarimeter(PolarimeterParams {
            matched: false,
            flip_hand: true,
            crossfade_ms: 0,
            ..settings
        });
        let descriptor = processor_descriptor("polarimeter").expect("polarimeter");
        let capacity = (descriptor.lane_format)(&params, &ctx(&centers, &geometry), 0).capacity;
        let mut processor = create_processor(&ctx(&centers, &geometry), &params).expect("build");
        let owned: Vec<_> = (0..LANES)
            .map(|lane| noise(MAX_BLOCK, lane as u32 + 3))
            .collect();
        let full: Vec<&[Complex<f32>]> = owned.iter().map(Vec::as_slice).collect();
        let short: Vec<&[Complex<f32>]> = owned.iter().map(|lane| &lane[..2_001]).collect();
        let spectra: Vec<Vec<Complex<f32>>> = (0..LANES)
            .map(|lane| vec![Complex::from_polar(1.0, 0.4 * lane as f32); 256])
            .collect();
        let first = CorrectionView::new(1, RATE, &spectra);
        let second = CorrectionView::new(2, RATE, &spectra);
        let mut host = Host {
            report: ProcessorReading::empty("polarimeter"),
            beam: [LaneBuffer::new(capacity)],
            reports: 0,
        };
        for step in 0..12 {
            host.step(processor.as_mut(), &full, step, CorrectionView::identity());
        }
        let warm = host.reports;
        assert!(warm > 0);
        assert_no_alloc("polarimeter", || {
            for step in 0..12 {
                host.step(processor.as_mut(), &full, step, first);
                host.step(processor.as_mut(), &short, step, second);
                host.step(processor.as_mut(), &full, step, CorrectionView::identity());
            }
            processor.apply(&orthogonal).expect("in place");
            processor.retune(&ctx(&moved, &geometry)).expect("retune");
            processor.reset(ResetCause::Realigned);
            for step in 0..6 {
                host.step(processor.as_mut(), &full, step, first);
            }
            processor.apply(&params).expect("in place");
        });
        assert!(host.reports > warm, "the measured run must report");
        assert_eq!(host.beam[0].overflowed(), 0);
        let faults = processor.faults();
        assert_eq!((faults.lane_mismatch, faults.solver_failures), (0, 0));
    }
}
