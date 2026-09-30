use sdrmm_dsp::manifold::Direction;
use sdrmm_dsp::scene::{ArrayScene, SceneSignal, SceneSource};
use sdrmm_dsp::special::wrap_deg;
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, Coherence, DecoderEvent, RdsUpdate, SpatialSpectrumFrame,
    Winding,
};

use super::*;
use crate::array_processor::{
    CalView, CorrectionView, OutputSlots, Pose, create_processor, geometry_of, processor_descriptor,
};

const RATE: f64 = 2_400_000.0;
const CENTER_HZ: f64 = 433.92e6;
const BLOCK: usize = 24_000;
const BLOCKS_PER_SECOND: usize = 100;
const NANOS_PER_BLOCK: u64 = 10_000_000;

fn kraken() -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m: 0.2939,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    }
}

fn tone(azimuth_deg: f64, offset_hz: f64) -> SceneSource {
    SceneSource::new(
        Direction::horizon(azimuth_deg),
        0.0,
        SceneSignal::Tone { offset_hz },
    )
}

fn carriers() -> [SceneSource; 2] {
    [tone(40.0, -300e3), tone(200.0, 500e3)]
}

fn error_deg(got: f64, want: f64) -> f64 {
    wrap_deg(got - want).abs()
}

struct Array {
    geometry: ArrayGeometry,
    positions: Vec<[f64; 3]>,
    center_hz: f64,
    centers: Vec<f64>,
}

impl Array {
    fn new(geometry: ArrayGeometry, lanes: usize, center_hz: f64) -> Self {
        Self {
            positions: geometry.positions(lanes).unwrap(),
            geometry,
            center_hz,
            centers: vec![center_hz; lanes],
        }
    }

    fn ctx(&self) -> ArrayCtx<'_> {
        ArrayCtx {
            node: "spatial-1",
            lanes: self.centers.len(),
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

struct Rig {
    array: Array,
    scene: ArrayScene,
    processor: SpatialSpectrumProcessor,
    report: Option<ProcessorReading>,
    surface: Option<SurfaceFrame>,
    heading_deg: Option<f64>,
    unix_ns: u64,
    readings: Vec<SpatialReading>,
    frames: Vec<SpatialSpectrumOwned>,
}

impl Rig {
    fn new(spatial: SpatialSpectrumParams, sources: &[SceneSource]) -> Self {
        let array = Array::new(kraken(), 5, CENTER_HZ);
        let geometry = geometry_of(&array.geometry, 5).unwrap();
        let scene = sources.iter().fold(
            ArrayScene::new(geometry, CENTER_HZ, RATE)
                .with_noise_db(0.0)
                .with_seed(3),
            |scene, source| scene.with_source(*source),
        );
        let params = ProcessorParams::SpatialSpectrum(spatial);
        let processor = SpatialSpectrumProcessor::new(&array.ctx(), &params).unwrap();
        let cells = surface_cells(&spatial);
        Self {
            array,
            scene,
            processor,
            report: ProcessorReading::empty("spatial_spectrum"),
            surface: Some(SurfaceFrame::SpatialSpectrum(SpatialSpectrumOwned {
                cells: Vec::with_capacity(cells),
                ..SpatialSpectrumOwned::default()
            })),
            heading_deg: None,
            unix_ns: 1_790_000_000_000_000_000,
            readings: Vec::new(),
            frames: Vec::new(),
        }
    }

    fn step(&mut self) {
        let rendered = self.scene.render(BLOCK).unwrap();
        let lanes: Vec<&[Complex<f32>]> = rendered.iter().map(Vec::as_slice).collect();
        let block = ArrayBlock {
            lanes: &lanes,
            corrected: true,
            correction: CorrectionView::identity(),
            first_index: 0,
            unix_ns: self.unix_ns,
            generation: 0,
            gap_before: false,
            centers_hz: &self.array.centers,
            cal: CalView::default(),
            pose: Pose {
                heading_deg: self.heading_deg,
                ..Pose::default()
            },
        };
        let mut events = [DecoderEvent::Rds(RdsUpdate::default())];
        let mut out = ProcessorOutput::new(OutputSlots {
            report: self.report.as_mut(),
            surface: self.surface.as_mut(),
            events: &mut events,
            lanes: &mut [],
        });
        self.processor.process(&block, &mut out);
        let tally = out.tally();
        assert_eq!(tally.events, 0);
        if tally.report
            && let Some(ProcessorReading::SpatialSpectrum(reading)) = &self.report
        {
            self.readings.push(reading.clone());
        }
        if tally.surface
            && let Some(SurfaceFrame::SpatialSpectrum(frame)) = &self.surface
        {
            self.frames.push(frame.clone());
        }
        self.unix_ns += NANOS_PER_BLOCK;
    }

    fn run(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.step();
        }
    }

    fn bearing_at(&self, offset_hz: f64) -> f64 {
        let reading = self.readings.last().unwrap();
        let column_hz = RATE / f64::from(SpatialSpectrumParams::default().columns);
        let peak = reading
            .peaks
            .iter()
            .filter(|peak| (peak.freq_hz - (CENTER_HZ + offset_hz)).abs() < 1.5 * column_hz)
            .max_by(|a, b| a.db.total_cmp(&b.db))
            .unwrap_or_else(|| panic!("no peak at {offset_hz}: {:?}", reading.peaks));
        f64::from(peak.bearing_deg)
    }
}

fn column_of(offset_hz: f64, spatial: &SpatialSpectrumParams) -> usize {
    let bin = ((offset_hz / RATE + 0.5) * f64::from(spatial.bins)) as usize;
    bin / (spatial.bins / spatial.columns) as usize
}

fn row_level(frame: &SpatialSpectrumOwned, column: usize, bearing_deg: f64) -> u8 {
    let step = 360.0 / f64::from(frame.bearings);
    let row = (bearing_deg / step).round() as usize % usize::from(frame.bearings);
    frame.cells[row * usize::from(frame.bins) + column]
}

#[test]
fn spatial_spectrum_places_two_carriers_at_their_bearings() {
    let spatial = SpatialSpectrumParams::default();
    let mut rig = Rig::new(spatial, &carriers());
    rig.heading_deg = Some(10.0);
    rig.run(BLOCKS_PER_SECOND + 20);
    assert_eq!(rig.readings.len(), 2);
    for (offset, bearing) in [(-300e3, 40.0), (500e3, 200.0)] {
        let found = rig.bearing_at(offset);
        assert!(error_deg(found, bearing) < 2.0, "{offset}: {found}");
        let frame = rig.frames.last().unwrap();
        let column = column_of(offset, &spatial);
        let at = row_level(frame, column, bearing);
        let away = row_level(frame, column, bearing + 180.0);
        let top = (0..usize::from(frame.bearings))
            .map(|row| frame.cells[row * usize::from(frame.bins) + column])
            .max()
            .unwrap();
        assert!(
            u16::from(at) + 1 >= u16::from(top) && u16::from(away) + 20 < u16::from(at),
            "{offset}: {at} {top} {away}"
        );
    }
    let reading = rig.readings.last().unwrap();
    let strongest = reading.peaks[0];
    let expected = strongest.bearing_deg + 10.0;
    assert!(error_deg(f64::from(strongest.true_deg.unwrap()), f64::from(expected)) < 1e-3);
    assert_eq!(reading.azimuth_deg, Some(10.0));
    assert_eq!(reading.frames, rig.frames.len() as u64 - 1);
    assert_eq!(reading.dropped_frames, 0);
    assert_eq!(rig.processor.faults(), ProcessorFaults::default());
}

#[test]
fn spatial_frame_header_matches_the_grid() {
    let spatial = SpatialSpectrumParams::default();
    let mut rig = Rig::new(spatial, &carriers());
    rig.run(10);
    let frame = rig.frames.last().unwrap();
    assert_eq!(rig.frames.len(), 1);
    assert_eq!((frame.bearings, frame.bins), (180, 256));
    assert_eq!(frame.span_hz, RATE as f32);
    assert_eq!(frame.center_hz, CENTER_HZ);
    assert_eq!(frame.cells.len(), 180 * 256);
    assert!((frame.db_max - frame.db_min - spatial.span_db).abs() < 1e-3);
    assert_eq!(frame.timestamp, rig.unix_ns / NANOS_PER_MILLI - 10);
    let encoded = SurfaceFrame::SpatialSpectrum(frame.clone()).encode(4);
    let decoded = SpatialSpectrumFrame::decode(&encoded).unwrap();
    assert_eq!(decoded.stream_id, 4);
    assert_eq!(decoded.cells.len(), frame.cells.len());
    let descriptor = processor_descriptor("spatial_spectrum").unwrap();
    let format = (descriptor.lane_format)(
        &ProcessorParams::SpatialSpectrum(spatial),
        &rig.array.ctx(),
        0,
    );
    assert_eq!(format.capacity, frame.cells.len());
    assert_eq!(format.sample_rate, RATE);
}

#[test]
fn spatial_groups_follow_the_phase_error_rule() {
    let kraken = Rig::new(SpatialSpectrumParams::default(), &[]);
    assert_eq!(kraken.processor.groups(), 1);
    let hf = Array::new(
        ArrayGeometry::Ula {
            spacing_m: 2.5,
            axis_deg: 90.0,
        },
        5,
        7e6,
    );
    let params = ProcessorParams::SpatialSpectrum(SpatialSpectrumParams::default());
    let processor = SpatialSpectrumProcessor::new(&hf.ctx(), &params).unwrap();
    assert_eq!(processor.groups(), 8);
    assert_eq!(frequency_groups(RATE, 10.0, 4), 4);
    assert_eq!(frequency_groups(1e3, 0.5, 256), 1);
    assert_eq!(group_columns(0, 8, 256), (0, 31));
    assert_eq!(group_columns(7, 8, 256), (224, 255));
    assert_eq!(group_columns(1, 3, 256), (86, 170));
}

#[test]
fn spatial_capon_and_music_run() {
    let mut bearings = Vec::new();
    for method in [SpatialMethod::Capon, SpatialMethod::Music] {
        let spatial = SpatialSpectrumParams {
            method,
            ..SpatialSpectrumParams::default()
        };
        let mut rig = Rig::new(spatial, &carriers());
        rig.run(BLOCKS_PER_SECOND + 20);
        bearings.push([rig.bearing_at(-300e3), rig.bearing_at(500e3)]);
        assert_eq!(rig.processor.faults().solver_failures, 0, "{method:?}");
    }
    for (index, truth) in [40.0, 200.0].into_iter().enumerate() {
        assert!(
            error_deg(bearings[0][index], bearings[1][index]) < 2.0,
            "{bearings:?}"
        );
        assert!(error_deg(bearings[0][index], truth) < 2.0, "{bearings:?}");
    }
}

#[test]
fn spatial_method_change_applies_in_place() {
    let bartlett = ProcessorParams::SpatialSpectrum(SpatialSpectrumParams::default());
    let music = ProcessorParams::SpatialSpectrum(SpatialSpectrumParams {
        method: SpatialMethod::Music,
        span_db: 40.0,
        report_ms: 200,
        ..SpatialSpectrumParams::default()
    });
    let wider = ProcessorParams::SpatialSpectrum(SpatialSpectrumParams {
        bins: 2048,
        ..SpatialSpectrumParams::default()
    });
    let descriptor = processor_descriptor("spatial_spectrum").unwrap();
    assert!((descriptor.in_place)(&bartlett, &music));
    assert!(!(descriptor.in_place)(&bartlett, &wider));
    let mut rig = Rig::new(SpatialSpectrumParams::default(), &carriers());
    rig.run(10);
    rig.processor.apply(&music).unwrap();
    assert!(rig.processor.apply(&wider).is_err());
    rig.run(20);
    assert_eq!(rig.frames.len(), 2);
    let frame = rig.frames.last().unwrap();
    assert!((frame.db_max - frame.db_min - 40.0).abs() < 1e-3);
}

#[test]
fn spatial_counts_frames_it_cannot_deliver() {
    let spatial = SpatialSpectrumParams::default();
    let mut rig = Rig::new(spatial, &carriers());
    rig.surface = None;
    rig.run(10);
    rig.surface = Some(SurfaceFrame::SpatialSpectrum(SpatialSpectrumOwned {
        cells: Vec::with_capacity(16),
        ..SpatialSpectrumOwned::default()
    }));
    rig.run(10);
    assert!(rig.frames.is_empty());
    assert_eq!(rig.processor.faults().truncated, 1);
    assert_eq!(rig.processor.dropped_frames, 2);
    let array = Array::new(kraken(), 5, CENTER_HZ);
    let built = create_processor(
        &array.ctx(),
        &ProcessorParams::SpatialSpectrum(SpatialSpectrumParams::default()),
    );
    assert!(built.is_ok());
}

#[test]
fn spatial_retune_moves_the_rings() {
    let spatial = SpatialSpectrumParams::default();
    let mut rig = Rig::new(spatial, &[tone(120.0, 200e3)]);
    rig.run(10);
    let retuned = 300e6;
    rig.array.center_hz = retuned;
    rig.array.centers.fill(retuned);
    rig.scene.center_hz = retuned;
    rig.processor.retune(&rig.array.ctx()).unwrap();
    let column_hz = RATE / f64::from(spatial.bins);
    assert!((rig.processor.rings[0].freq_hz() - retuned).abs() < column_hz);
    rig.run(BLOCKS_PER_SECOND);
    let frame = rig.frames.last().unwrap();
    assert_eq!(frame.center_hz, retuned);
    let column = column_of(200e3, &spatial);
    let top = (0..usize::from(frame.bearings))
        .map(|row| frame.cells[row * usize::from(frame.bins) + column])
        .max()
        .unwrap();
    assert!(u16::from(row_level(frame, column, 120.0)) + 1 >= u16::from(top));
    assert_eq!(top, 255);
    let reading = rig.readings.last().unwrap();
    assert!((reading.peaks[0].freq_hz - (retuned + 200e3)).abs() < 1.5 * RATE / 256.0);
    assert!(error_deg(f64::from(reading.peaks[0].bearing_deg), 120.0) < 2.0);
    let mut spread = Array::new(kraken(), 5, retuned);
    spread.centers[2] += 1e6;
    assert!(rig.processor.retune(&spread.ctx()).is_err());
}

#[test]
fn spatial_band_waits_for_a_full_frame() {
    let spatial = SpatialSpectrumParams {
        bins: 256,
        columns: 64,
        report_ms: 50,
        offset_hz: 100e3,
        bandwidth_hz: Some(1_000.0),
        ..SpatialSpectrumParams::default()
    };
    let mut rig = Rig::new(spatial, &[tone(75.0, 100e3)]);
    rig.run(10);
    assert!(rig.frames.is_empty() && rig.readings.is_empty());
    rig.run(30);
    let frame = rig.frames.last().unwrap();
    assert_eq!(frame.span_hz, band_rate(RATE, spatial.bandwidth_hz) as f32);
    assert_eq!(frame.center_hz, CENTER_HZ + 100e3);
    assert_eq!((frame.bearings, frame.bins), (180, 64));
    let reading = rig.readings.last().unwrap();
    let strongest = reading.peaks[0];
    assert!(
        (strongest.freq_hz - (CENTER_HZ + 100e3)).abs() < 50.0,
        "{strongest:?}"
    );
    assert!(
        error_deg(f64::from(strongest.bearing_deg), 75.0) < 2.0,
        "{strongest:?}"
    );
    let outside = SpatialSpectrumParams {
        offset_hz: 1.195e6,
        bandwidth_hz: Some(20e3),
        ..SpatialSpectrumParams::default()
    };
    let refused = create_processor(&rig.array.ctx(), &ProcessorParams::SpatialSpectrum(outside));
    assert_eq!(
        refused.err().map(|error| error.to_string()),
        Some("Offset out of range".to_owned())
    );
}
