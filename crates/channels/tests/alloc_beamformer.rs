use num_complex::Complex;
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, LaneBuffer, OutputSlots, Pose,
    ProcessorOutput, ResetCause, Steer, create_processor, processor_descriptor,
};
use sdrmm_dsp::manifold::{Direction, Geometry, Winding as DspWinding};
use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};
use sdrmm_wire::{
    Adaptation, ArrayGeometry, ArrayTuningMode, BeamMode, BeamformerParams, Coherence,
    ProcessorParams, ProcessorReading, SteerSource, Winding,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[cfg(test)]
mod beamformer {
    use super::*;

    const LANES: usize = 5;
    const RATE: f64 = 960_000.0;
    const MAX_BLOCK: usize = 16_384;
    const CENTER_HZ: f64 = 433.92e6;
    const RADIUS_M: f64 = 0.2939;

    fn geometry() -> ArrayGeometry {
        ArrayGeometry::Uca {
            radius_m: RADIUS_M,
            first_deg: 0.0,
            winding: Winding::Clockwise,
        }
    }

    fn lanes(len: usize) -> Vec<Vec<Complex<f32>>> {
        let uca = Geometry::uca(RADIUS_M, LANES, 0.0, DspWinding::Clockwise).expect("uca");
        let mut scene = ArrayScene::new(uca, CENTER_HZ, RATE)
            .with_source(SceneSource::new(
                Direction::horizon(137.0),
                0.0,
                SceneSignal::Tone {
                    offset_hz: 20_000.0,
                },
            ))
            .with_source(SceneSource::new(
                Direction::horizon(30.0),
                10.0,
                SceneSignal::Noise {
                    offset_hz: -15_000.0,
                    bandwidth_hz: 20_000.0,
                },
            ))
            .with_noise_db(0.0)
            .with_seed(5);
        scene.render(len).expect("render")
    }

    fn ctx<'a>(centers: &'a [f64], geometry: &'a ArrayGeometry) -> ArrayCtx<'a> {
        ArrayCtx {
            node: "beamformer-alloc",
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

    fn block<'a>(lanes: &'a [&'a [Complex<f32>]], unix_ns: u64) -> ArrayBlock<'a> {
        ArrayBlock {
            lanes,
            corrected: false,
            correction: CorrectionView::identity(),
            first_index: 0,
            unix_ns,
            generation: 0,
            gap_before: false,
            centers_hz: &[],
            cal: CalView::default(),
            pose: Pose {
                heading_deg: Some(15.0),
                ..Pose::default()
            },
        }
    }

    struct Host {
        report: Option<ProcessorReading>,
        beam: [LaneBuffer; 1],
        reports: u32,
        clock: u64,
    }

    impl Host {
        fn step(&mut self, processor: &mut dyn ArrayProcessor, lanes: &[&[Complex<f32>]]) {
            self.beam[0].clear();
            let mut out = ProcessorOutput::new(OutputSlots {
                report: self.report.as_mut(),
                surface: None,
                events: &mut [],
                lanes: &mut self.beam,
            });
            processor.process(&block(lanes, self.clock), &mut out);
            self.reports += u32::from(out.tally().report);
            self.clock += 10_000_000;
        }
    }

    fn steer(wall_ms: u64, relative_deg: f64) -> Steer {
        Steer {
            same_array: true,
            relative_deg,
            true_deg: Some(relative_deg + 15.0),
            others_relative_deg: [30.0, 250.0, 0.0],
            others_true_deg: [Some(45.0), Some(265.0), None],
            others: 2,
            wall_ms,
            ..Steer::default()
        }
    }

    fn exercise(settings: BeamformerParams, tweaked: BeamformerParams) {
        let geometry = geometry();
        let centers = vec![CENTER_HZ; LANES];
        let moved = vec![CENTER_HZ + 1e6; LANES];
        let params = ProcessorParams::Beamformer(settings);
        let tweaked = ProcessorParams::Beamformer(tweaked);
        let descriptor = processor_descriptor("beamformer").expect("beamformer");
        assert!((descriptor.in_place)(&params, &tweaked));
        let capacity = (descriptor.lane_format)(&params, &ctx(&centers, &geometry), 0).capacity;
        let mut processor = create_processor(&ctx(&centers, &geometry), &params).expect("build");
        let owned = lanes(4 * MAX_BLOCK);
        let full: Vec<&[Complex<f32>]> = owned.iter().map(|lane| &lane[..MAX_BLOCK]).collect();
        let later: Vec<&[Complex<f32>]> = owned
            .iter()
            .map(|lane| &lane[MAX_BLOCK..2 * MAX_BLOCK])
            .collect();
        let short: Vec<&[Complex<f32>]> = owned
            .iter()
            .map(|lane| &lane[3 * MAX_BLOCK..3 * MAX_BLOCK + 2_001])
            .collect();
        let mut host = Host {
            report: ProcessorReading::empty("beamformer"),
            beam: [LaneBuffer::new(capacity)],
            reports: 0,
            clock: 0,
        };
        processor.steer(&steer(0, 137.0));
        for _ in 0..8 {
            host.step(processor.as_mut(), &full);
            host.step(processor.as_mut(), &later);
        }
        let warm = host.reports;
        assert!(warm > 0, "{:?}", settings_mode(&params));
        assert_no_alloc("beamformer", || {
            for round in 0..6 {
                processor.steer(&steer(host.clock / 1_000_000, 137.0 + f64::from(round)));
                host.step(processor.as_mut(), &full);
                host.step(processor.as_mut(), &short);
                host.step(processor.as_mut(), &later);
            }
            processor.apply(&tweaked).expect("in place");
            host.step(processor.as_mut(), &full);
            processor.retune(&ctx(&moved, &geometry)).expect("retune");
            processor.reset(ResetCause::Retuned);
            for _ in 0..4 {
                host.step(processor.as_mut(), &later);
            }
            processor.apply(&params).expect("in place");
            host.step(processor.as_mut(), &full);
        });
        assert!(
            host.reports > warm,
            "{:?} must report",
            settings_mode(&params)
        );
        assert_eq!(host.beam[0].overflowed(), 0);
        let faults = processor.faults();
        assert_eq!((faults.lane_mismatch, faults.truncated), (0, 0));
    }

    fn settings_mode(params: &ProcessorParams) -> Option<BeamMode> {
        match params {
            ProcessorParams::Beamformer(settings) => Some(settings.mode),
            _ => None,
        }
    }

    fn base(mode: BeamMode) -> BeamformerParams {
        BeamformerParams {
            mode,
            update_ms: 50,
            ..BeamformerParams::default()
        }
    }

    #[test]
    fn beamformer_process_does_not_allocate() {
        let fixed = SteerSource::Fixed {
            azimuth_deg: 140.0,
            elevation_deg: 0.0,
        };
        let cases = [
            base(BeamMode::Mrc),
            base(BeamMode::Das),
            base(BeamMode::Mvdr),
            BeamformerParams {
                nulls_deg: vec![250.0],
                auto_nulls: true,
                ..base(BeamMode::Lcmv)
            },
            BeamformerParams {
                auto_nulls: true,
                ..base(BeamMode::Gsc)
            },
            base(BeamMode::Canceller),
            BeamformerParams {
                taps: 8,
                ..base(BeamMode::Canceller)
            },
            BeamformerParams {
                taps: 2,
                adaptation: Adaptation::Rls,
                ..base(BeamMode::Canceller)
            },
            base(BeamMode::Cma),
        ];
        for settings in cases {
            let tweaked = BeamformerParams {
                steer: fixed,
                step: 0.02,
                forget: 0.995,
                loading: 0.3,
                crossfade_ms: 5,
                carry_over: 0.5,
                ..settings.clone()
            };
            exercise(settings, tweaked);
        }
    }
}
