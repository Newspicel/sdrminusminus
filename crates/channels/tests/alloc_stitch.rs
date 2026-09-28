use num_complex::Complex;
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, LaneBuffer, OutputSlots, Pose,
    ProcessorOutput, ResetCause, create_processor, processor_descriptor,
};
use sdrmm_dsp::stitch::auto_offsets;
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, Coherence, ProcessorParams, ProcessorReading, StitchBlend,
    StitchParams,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 3;
const RATE: f64 = 8_192.0;
const MAX_BLOCK: usize = 4_096;
const CENTER_HZ: f64 = 100e6;

fn noise(len: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32 - 0.5
    };
    (0..len).map(|_| Complex::new(next(), next())).collect()
}

fn ctx<'a>(centers: &'a [f64], geometry: &'a ArrayGeometry) -> ArrayCtx<'a> {
    ArrayCtx {
        node: "stitch-alloc",
        lanes: centers.len(),
        sample_rate: RATE,
        center_hz: CENTER_HZ,
        lane_centers_hz: centers,
        geometry,
        positions_m: &[],
        manifold: None,
        tier: Coherence::TimeSync,
        tuning: ArrayTuningMode::Spread,
        max_block: MAX_BLOCK,
    }
}

fn block<'a>(lanes: &'a [&'a [Complex<f32>]], unix_ns: u64) -> ArrayBlock<'a> {
    ArrayBlock {
        lanes,
        corrected: true,
        correction: CorrectionView::identity(),
        first_index: 0,
        unix_ns,
        generation: 0,
        gap_before: false,
        centers_hz: &[],
        cal: CalView::default(),
        pose: Pose::default(),
    }
}

struct Host {
    report: Option<ProcessorReading>,
    wide: [LaneBuffer; 1],
    reports: u32,
}

impl Host {
    fn step(
        &mut self,
        processor: &mut dyn ArrayProcessor,
        lanes: &[&[Complex<f32>]],
        unix_ns: u64,
    ) {
        self.wide[0].clear();
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: None,
            events: &mut [],
            lanes: &mut self.wide,
        });
        processor.process(&block(lanes, unix_ns), &mut out);
        self.reports += u32::from(out.tally().report);
    }
}

mod stitch {
    use super::*;

    #[test]
    fn stitch_process_does_not_allocate() {
        let geometry = ArrayGeometry::default();
        let centers: Vec<f64> = auto_offsets(LANES, RATE)
            .iter()
            .map(|offset| CENTER_HZ + offset)
            .collect();
        let shifted: Vec<f64> = centers.iter().map(|hz| hz + 3.0).collect();
        let params = ProcessorParams::Stitch(StitchParams::default());
        let equal = ProcessorParams::Stitch(StitchParams {
            blend: StitchBlend::Equal,
            ..StitchParams::default()
        });
        let descriptor = processor_descriptor("stitch").expect("stitch");
        let capacity = (descriptor.lane_format)(&params, &ctx(&centers, &geometry), 0).capacity;
        let mut processor = create_processor(&ctx(&centers, &geometry), &params).expect("stitch");
        let owned: Vec<_> = (0..LANES)
            .map(|lane| noise(MAX_BLOCK, lane as u32 + 5))
            .collect();
        let full: Vec<&[Complex<f32>]> = owned.iter().map(Vec::as_slice).collect();
        let short: Vec<&[Complex<f32>]> = owned.iter().map(|lane| &lane[..1_001]).collect();
        let mut host = Host {
            report: ProcessorReading::empty("stitch"),
            wide: [LaneBuffer::new(capacity)],
            reports: 0,
        };
        for second in 0..4 {
            host.step(processor.as_mut(), &full, second);
            host.step(processor.as_mut(), &full, second);
        }
        let warm = host.reports;
        assert_no_alloc("stitch", || {
            for second in 0..8 {
                host.step(processor.as_mut(), &full, second);
                host.step(processor.as_mut(), &short, second);
                host.step(processor.as_mut(), &full, second);
            }
            processor.apply(&equal).expect("in place");
            processor.retune(&ctx(&shifted, &geometry)).expect("retune");
            processor.reset(ResetCause::Retuned);
            host.step(processor.as_mut(), &full, 9);
            processor.apply(&params).expect("in place");
            processor.retune(&ctx(&centers, &geometry)).expect("retune");
        });
        assert!(
            host.reports > warm,
            "the measured run must publish a report"
        );
        assert_eq!(host.wide[0].overflowed(), 0);
        assert_eq!(processor.faults().lane_mismatch, 0);
    }
}
