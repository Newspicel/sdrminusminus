use num_complex::Complex;
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, OutputSlots, Pose,
    ProcessorOutput, ResetCause, create_processor,
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};
use sdrmm_wire::processor::correlator::MAX_VISIBILITY_CELLS;
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, Coherence, CorrelatorParams, ProcessorParams, ProcessorReading,
    SurfaceFrame, VisibilityOwned,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 5;
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
    (0..len).map(|_| Complex::new(next(), next())).collect()
}

fn ctx<'a>(
    centers: &'a [f64],
    geometry: &'a ArrayGeometry,
    positions: &'a [[f64; 3]],
) -> ArrayCtx<'a> {
    ArrayCtx {
        node: "correlator-alloc",
        lanes: centers.len(),
        sample_rate: RATE,
        center_hz: centers[0],
        lane_centers_hz: centers,
        geometry,
        positions_m: positions,
        manifold: None,
        tier: Coherence::PhaseCoherent,
        tuning: ArrayTuningMode::Together,
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
    surface: Option<SurfaceFrame>,
    reports: u32,
    surfaces: u32,
}

impl Host {
    fn step(
        &mut self,
        processor: &mut dyn ArrayProcessor,
        lanes: &[&[Complex<f32>]],
        unix_ns: u64,
    ) {
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: self.surface.as_mut(),
            events: &mut [],
            lanes: &mut [],
        });
        processor.process(&block(lanes, unix_ns), &mut out);
        self.reports += u32::from(out.tally().report);
        self.surfaces += u32::from(out.tally().surface);
    }
}

mod correlator {
    use super::*;

    #[test]
    fn correlator_process_does_not_allocate() {
        let geometry = ArrayGeometry::default();
        let positions = geometry.positions(LANES).expect("positions");
        let centers = vec![CENTER_HZ; LANES];
        let moved = vec![CENTER_HZ + 1e6; LANES];
        let quick = CorrelatorParams {
            integrate_s: 0.05,
            ..CorrelatorParams::default()
        };
        let params = ProcessorParams::Correlator(quick);
        let longer = ProcessorParams::Correlator(CorrelatorParams {
            integrate_s: 0.06,
            bandwidth_hz: Some(200_000.0),
            ..quick
        });
        let mut processor =
            create_processor(&ctx(&centers, &geometry, &positions), &params).expect("build");
        let owned: Vec<_> = (0..LANES)
            .map(|lane| noise(MAX_BLOCK, lane as u32 + 9))
            .collect();
        let full: Vec<&[Complex<f32>]> = owned.iter().map(Vec::as_slice).collect();
        let short: Vec<&[Complex<f32>]> = owned.iter().map(|lane| &lane[..3_333]).collect();
        let cells = MAX_VISIBILITY_CELLS as usize;
        let mut host = Host {
            report: ProcessorReading::empty("correlator"),
            surface: Some(SurfaceFrame::Visibility(VisibilityOwned {
                amplitude: Vec::with_capacity(cells),
                phase: Vec::with_capacity(cells),
                ..VisibilityOwned::default()
            })),
            reports: 0,
            surfaces: 0,
        };
        for step in 0..10 {
            host.step(processor.as_mut(), &full, step);
        }
        let warm = (host.reports, host.surfaces);
        assert!(warm.0 > 0 && warm.1 > 0);
        assert_no_alloc("correlator", || {
            for step in 0..24 {
                host.step(processor.as_mut(), &full, step);
                host.step(processor.as_mut(), &short, step);
            }
            processor.apply(&longer).expect("in place");
            processor
                .retune(&ctx(&moved, &geometry, &positions))
                .expect("retune");
            processor.reset(ResetCause::Retuned);
            for step in 0..12 {
                host.step(processor.as_mut(), &full, step);
            }
            processor.apply(&params).expect("in place");
        });
        assert!(host.reports > warm.0 + 2, "the measured run must report");
        assert!(host.surfaces > warm.1 + 2, "the measured run must paint");
        let faults = processor.faults();
        assert_eq!((faults.lane_mismatch, faults.truncated), (0, 0));
    }
}
