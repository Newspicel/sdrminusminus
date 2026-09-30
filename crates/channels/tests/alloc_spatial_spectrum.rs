use sdrmm_test_support::CountingAlloc;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[cfg(test)]
mod spatial_spectrum {
    use num_complex::Complex;
    use sdrmm_channels::array_processor::{
        ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, OutputSlots, Pose,
        ProcessorOutput, ResetCause, create_processor, geometry_of,
    };
    use sdrmm_channels::spatial_spectrum::surface_cells;
    use sdrmm_dsp::manifold::Direction;
    use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
    use sdrmm_test_support::assert_no_alloc;
    use sdrmm_wire::{
        ArrayGeometry, ArrayTuningMode, Coherence, ProcessorParams, ProcessorReading,
        SpatialMethod, SpatialSpectrumOwned, SpatialSpectrumParams, SurfaceFrame, Winding,
    };

    const RATE: f64 = 2_400_000.0;
    const CENTER_HZ: f64 = 433.92e6;
    const BLOCK: usize = 24_000;
    const BLOCKS: usize = 10;
    const LANES: usize = 5;

    struct Array {
        geometry: ArrayGeometry,
        positions: Vec<[f64; 3]>,
        center_hz: f64,
        centers: Vec<f64>,
    }

    impl Array {
        fn tuned(center_hz: f64) -> Self {
            let geometry = ArrayGeometry::Uca {
                radius_m: 0.2939,
                first_deg: 0.0,
                winding: Winding::Clockwise,
            };
            Self {
                positions: geometry.positions(LANES).unwrap(),
                geometry,
                center_hz,
                centers: vec![center_hz; LANES],
            }
        }

        fn ctx(&self) -> ArrayCtx<'_> {
            ArrayCtx {
                node: "spatial-alloc",
                lanes: LANES,
                sample_rate: RATE,
                center_hz: self.center_hz,
                lane_centers_hz: &self.centers,
                geometry: &self.geometry,
                positions_m: &self.positions,
                manifold: None,
                tier: Coherence::PhaseCoherent,
                tuning: ArrayTuningMode::Together,
                max_block: BLOCK,
            }
        }
    }

    fn blocks(array: &Array) -> Vec<Vec<Vec<Complex<f32>>>> {
        let geometry = geometry_of(&array.geometry, LANES).unwrap();
        let mut scene = [(40.0, -300e3), (200.0, 500e3)].iter().fold(
            ArrayScene::new(geometry, CENTER_HZ, RATE)
                .with_noise_db(0.0)
                .with_seed(9),
            |scene, &(azimuth, offset_hz)| {
                scene.with_source(SceneSource::new(
                    Direction::horizon(azimuth),
                    0.0,
                    SceneSignal::Tone { offset_hz },
                ))
            },
        );
        (0..BLOCKS).map(|_| scene.render(BLOCK).unwrap()).collect()
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
            centers: &[f64],
        ) {
            let block = ArrayBlock {
                lanes,
                corrected: true,
                correction: CorrectionView::identity(),
                first_index: 0,
                unix_ns: 1_790_000_000_000_000_000,
                generation: 0,
                gap_before: false,
                centers_hz: centers,
                cal: CalView::default(),
                pose: Pose {
                    heading_deg: Some(15.0),
                    ..Pose::default()
                },
            };
            let mut out = ProcessorOutput::new(OutputSlots {
                report: self.report.as_mut(),
                surface: self.surface.as_mut(),
                events: &mut [],
                lanes: &mut [],
            });
            processor.process(&block, &mut out);
            let tally = out.tally();
            self.reports += u32::from(tally.report);
            self.surfaces += u32::from(tally.surface);
        }
    }

    #[test]
    fn spatial_process_does_not_allocate() {
        let array = Array::tuned(CENTER_HZ);
        let retuned = Array::tuned(CENTER_HZ + 3e6);
        let with = |method| {
            ProcessorParams::SpatialSpectrum(SpatialSpectrumParams {
                method,
                ..SpatialSpectrumParams::default()
            })
        };
        let bartlett = with(SpatialMethod::Bartlett);
        let banded = ProcessorParams::SpatialSpectrum(SpatialSpectrumParams {
            offset_hz: 100e3,
            bandwidth_hz: Some(200e3),
            ..SpatialSpectrumParams::default()
        });
        let capon = with(SpatialMethod::Capon);
        let music = with(SpatialMethod::Music);
        let mut processor = create_processor(&array.ctx(), &bartlett).unwrap();
        let mut narrow = create_processor(&array.ctx(), &banded).unwrap();
        let rendered = blocks(&array);
        let views: Vec<Vec<&[Complex<f32>]>> = rendered
            .iter()
            .map(|lanes| lanes.iter().map(Vec::as_slice).collect())
            .collect();
        let cells = surface_cells(&SpatialSpectrumParams::default());
        let mut host = Host {
            report: ProcessorReading::empty("spatial_spectrum"),
            surface: Some(SurfaceFrame::SpatialSpectrum(SpatialSpectrumOwned {
                cells: Vec::with_capacity(cells),
                ..SpatialSpectrumOwned::default()
            })),
            reports: 0,
            surfaces: 0,
        };
        for lanes in &views {
            host.step(processor.as_mut(), lanes, &array.centers);
            host.step(narrow.as_mut(), lanes, &array.centers);
        }
        let (reports, surfaces) = (host.reports, host.surfaces);
        assert_no_alloc("spatial spectrum", || {
            for method in [&capon, &music, &bartlett] {
                processor.apply(method).unwrap();
                for _ in 0..4 {
                    for lanes in &views {
                        host.step(processor.as_mut(), lanes, &array.centers);
                    }
                }
            }
            processor.retune(&retuned.ctx()).unwrap();
            processor.reset(ResetCause::Retuned);
            processor.retune(&array.ctx()).unwrap();
            for _ in 0..(1 + 100 / BLOCKS) {
                for lanes in &views {
                    host.step(processor.as_mut(), lanes, &array.centers);
                    host.step(narrow.as_mut(), lanes, &array.centers);
                }
            }
        });
        assert!(host.surfaces > surfaces, "the measured run must draw");
        assert!(host.reports > reports, "the measured run must report");
        for faults in [processor.faults(), narrow.faults()] {
            assert_eq!(
                (
                    faults.lane_mismatch,
                    faults.truncated,
                    faults.solver_failures
                ),
                (0, 0, 0)
            );
        }
    }
}
