use sdrmm_test_support::CountingAlloc;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[cfg(test)]
mod df {
    use num_complex::Complex;
    use sdrmm_channels::array_processor::{
        ArrayBlock, ArrayCtx, ArrayProcessor, CalView, CorrectionView, GeoFix, OutputSlots, Pose,
        ProcessorOutput, ResetCause, create_processor, geometry_of,
    };
    use sdrmm_dsp::manifold::Direction;
    use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
    use sdrmm_test_support::assert_no_alloc;
    use sdrmm_wire::processor::df::MAX_DF_PEAKS;
    use sdrmm_wire::{
        ArrayGeometry, ArrayTuningMode, Coherence, DecoderEvent, DfAlgorithm, DfParams,
        ProcessorParams, ProcessorReading, RdsUpdate, Winding,
    };

    const RATE: f64 = 2_400_000.0;
    const CENTER_HZ: f64 = 433.92e6;
    const OFFSET_HZ: f64 = 25_000.0;
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
                node: "df-alloc",
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
        let mut scene = [
            (40.0, -2_000.0),
            (130.0, 1_000.0),
            (220.0, 3_000.0),
            (310.0, -4_000.0),
        ]
        .iter()
        .fold(
            ArrayScene::new(geometry, CENTER_HZ, RATE)
                .with_noise_db(0.0)
                .with_seed(5),
            |scene, &(azimuth, offset)| {
                scene.with_source(SceneSource::new(
                    Direction::horizon(azimuth),
                    0.0,
                    SceneSignal::Tone {
                        offset_hz: OFFSET_HZ + offset,
                    },
                ))
            },
        );
        (0..BLOCKS).map(|_| scene.render(BLOCK).unwrap()).collect()
    }

    struct Host {
        report: Option<ProcessorReading>,
        events: Vec<DecoderEvent>,
        pose: Pose,
        reports: u32,
        bearings: u32,
        others: usize,
    }

    impl Host {
        fn step(
            &mut self,
            processor: &mut dyn ArrayProcessor,
            lanes: &[&[Complex<f32>]],
            centers: &[f64],
            unix_ns: u64,
        ) {
            let block = ArrayBlock {
                lanes,
                corrected: false,
                correction: CorrectionView::identity(),
                first_index: 0,
                unix_ns,
                generation: 0,
                gap_before: false,
                centers_hz: centers,
                cal: CalView::default(),
                pose: self.pose,
            };
            let mut out = ProcessorOutput::new(OutputSlots {
                report: self.report.as_mut(),
                surface: None,
                events: &mut self.events,
                lanes: &mut [],
            });
            processor.process(&block, &mut out);
            let tally = out.tally();
            self.reports += u32::from(tally.report);
            self.bearings += tally.events as u32;
            if let Some(DecoderEvent::Df(bearing)) = self.events.first()
                && tally.events > 0
            {
                self.others = self.others.max(bearing.others.len());
            }
        }
    }

    #[test]
    fn df_process_and_report_do_not_allocate() {
        let array = Array::tuned(CENTER_HZ);
        let retuned = Array::tuned(CENTER_HZ + 2e6);
        let params = DfParams {
            offset_hz: OFFSET_HZ,
            report_ms: 100,
            max_peaks: MAX_DF_PEAKS,
            ..DfParams::default()
        };
        let music = ProcessorParams::Df(params.clone());
        let capon = ProcessorParams::Df(DfParams {
            algorithm: DfAlgorithm::Capon,
            ..params
        });
        let mut processor = create_processor(&array.ctx(), &music).unwrap();
        let rendered = blocks(&array);
        let views: Vec<Vec<&[Complex<f32>]>> = rendered
            .iter()
            .map(|lanes| lanes.iter().map(Vec::as_slice).collect())
            .collect();
        let mut host = Host {
            report: ProcessorReading::empty("df"),
            events: vec![DecoderEvent::Rds(RdsUpdate::default()); 2],
            pose: Pose {
                heading_deg: Some(30.0),
                heading_sigma_deg: 1.0,
                yaw_rate_dps: Some(2.0),
                fix: Some(GeoFix {
                    lat: 48.0,
                    lon: 11.0,
                    accuracy_m: Some(3.0),
                    ..GeoFix::default()
                }),
                moving: false,
                follows: true,
            },
            reports: 0,
            bearings: 0,
            others: 0,
        };
        let mut unix_ns = 1_790_000_000_000_000_000u64;
        for _ in 0..3 {
            for lanes in &views {
                host.step(processor.as_mut(), lanes, &array.centers, unix_ns);
                unix_ns += 10_000_000;
            }
        }
        let (reports, bearings) = (host.reports, host.bearings);
        assert!(bearings > 0, "warm-up must publish a bearing");
        assert_no_alloc("df", || {
            for round in 0..4 {
                for lanes in &views {
                    host.step(processor.as_mut(), lanes, &array.centers, unix_ns);
                    unix_ns += 10_000_000;
                }
                host.pose.yaw_rate_dps = Some(if round == 1 { 90.0 } else { 2.0 });
            }
            processor.apply(&capon).unwrap();
            for lanes in &views {
                host.step(processor.as_mut(), lanes, &array.centers, unix_ns);
            }
            processor.retune(&retuned.ctx()).unwrap();
            processor.reset(ResetCause::Retuned);
            processor.retune(&array.ctx()).unwrap();
            processor.apply(&music).unwrap();
            for lanes in &views {
                host.step(processor.as_mut(), lanes, &array.centers, unix_ns);
            }
        });
        assert!(host.reports > reports, "the measured run must report");
        assert!(host.bearings > bearings, "the measured run must emit");
        assert!(host.others >= 2, "{}", host.others);
        let faults = processor.faults();
        assert_eq!((faults.lane_mismatch, faults.truncated), (0, 0));
    }
}
