use super::beamformer::*;
use super::correlator::*;
use super::df::*;
use super::polarimeter::*;
use super::spatial::*;
use super::stitch::*;
use super::*;
use crate::array::MAX_ARRAY_LANES;
use crate::geo::LatLon;
use crate::radar::{PassiveRadarParams, RadarUpdate};

macro_rules! table_rows {
    (
        processors: [$((
            $variant:ident,
            $type_id:literal,
            $params:ty,
            $reading:ty,
            $node:ident,
            $name:literal,
            $summary:literal,
            [$($port:tt)*]
        )),* $(,)?],
        probe: ($probe:ident, $probe_id:literal, $probe_params:ty) $(,)?
    ) => {
        [$(($type_id, stringify!($node), $name, $summary, stringify!($($port)*))),*]
    };
}

fn all_params() -> Vec<ProcessorParams> {
    vec![
        ProcessorParams::Df(DfParams::default()),
        ProcessorParams::Beamformer(BeamformerParams::default()),
        ProcessorParams::PassiveRadar(PassiveRadarParams::default()),
        ProcessorParams::Stitch(StitchParams::default()),
        ProcessorParams::SpatialSpectrum(SpatialSpectrumParams::default()),
        ProcessorParams::Correlator(CorrelatorParams::default()),
        ProcessorParams::Polarimeter(PolarimeterParams::default()),
    ]
}

#[test]
fn the_table_names_every_processor_once() {
    let rows = super::processors!(table_rows);
    let ids: Vec<&str> = rows.iter().map(|row| row.0).collect();
    assert_eq!(ids, PROCESSOR_TYPE_IDS);
    assert_eq!(
        ids,
        [
            "df",
            "beamformer",
            "passive_radar",
            "stitch",
            "spatial_spectrum",
            "correlator",
            "polarimeter"
        ]
    );
    let names: Vec<(&str, &str)> = rows.iter().map(|row| (row.2, row.3)).collect();
    assert_eq!(
        names,
        [
            ("Direction finder", "Bearings from an array"),
            ("Beamformer", "Steer, null or combine an array"),
            ("Passive radar", "Aircraft echoes of FM, DAB or DVB-T"),
            ("Stitch", "Spread lanes joined into one wide band"),
            ("Spatial spectrum", "Bearing over frequency"),
            ("Correlator", "Baseline visibilities"),
            ("Polarimeter", "Polarisation of two crossed antennas"),
        ]
    );
    let ports: Vec<String> = rows
        .iter()
        .map(|row| format!("{}: {}", row.1, row.4.replace('\n', " ")))
        .collect();
    assert_eq!(
        ports,
        [
            r#"DfNode: array_in, events_out("True bearings")"#,
            r#"BeamformerNode: array_in, steer_in("A direction finder to steer at"), beam_out("The combined lane")"#,
            r#"PassiveRadarNode: array_in, tx_in("GPS node fixed at the transmitter"), adsb_in("ADS-B decoder used as truth"), events_out("Track events")"#,
            r#"StitchNode: array_in, wide_out("Every lane joined")"#,
            "SpatialSpectrumNode: array_in",
            "CorrelatorNode: array_in",
            r#"PolarimeterNode: array_in, beam_out("Matched to the wave")"#,
        ]
    );
}

#[test]
fn processor_defaults_are_valid() {
    for params in all_params() {
        assert!(
            params.valid(),
            "{}: {:?}",
            params.type_id(),
            params.problem()
        );
    }
    assert!(crate::fusion::TriangulationParams::default().valid());
    assert!(crate::hunt::HuntSweepParams::default().valid());
    assert!(crate::array::ArrayNode::default().valid());
}

#[test]
fn type_ids_match_the_serialized_tags() {
    for params in all_params() {
        let json = serde_json::to_value(&params).expect("serialize");
        assert_eq!(json["type"], params.type_id());
        let back: ProcessorParams = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, params);
        let empty = serde_json::json!({ "type": params.type_id(), "settings": {} });
        let defaults: ProcessorParams = serde_json::from_value(empty).expect("defaults");
        assert_eq!(defaults, params);
    }
    for type_id in PROCESSOR_TYPE_IDS {
        let reading = ProcessorReading::empty(type_id).expect("reading");
        assert_eq!(reading.type_id(), *type_id);
        let json = serde_json::to_value(&reading).expect("serialize");
        assert_eq!(json["type"], *type_id);
    }
    assert_eq!(ProcessorReading::empty("probe"), None);
    assert_eq!(ProcessorReading::empty("scope"), None);
}

#[test]
fn empty_readings_reserve_their_wire_maxima() {
    let lanes = MAX_ARRAY_LANES as usize;
    let Some(ProcessorReading::Df(df)) = ProcessorReading::empty("df") else {
        panic!("df reading");
    };
    assert!(df.at.capacity() >= MAX_AT_LEN);
    assert!(df.peaks.capacity() >= usize::from(MAX_DF_PEAKS));
    assert!(df.pseudospectrum.capacity() >= DF_POINTS);
    assert!(df.likelihood.capacity() >= DF_POINTS);
    assert!(df.eigenvalues_db.capacity() >= lanes);
    assert!(df.peaks.is_empty() && df.pseudospectrum.is_empty());
    let Some(ProcessorReading::Beamformer(beam)) = ProcessorReading::empty("beamformer") else {
        panic!("beamformer reading");
    };
    assert!(beam.weights.capacity() >= lanes);
    assert!(beam.pattern.capacity() >= DF_POINTS);
    assert!(beam.nulls_deg.capacity() >= MAX_BEAM_NULLS);
    assert!(beam.null_depths_db.capacity() >= MAX_BEAM_NULLS);
    let Some(ProcessorReading::SpatialSpectrum(spatial)) =
        ProcessorReading::empty("spatial_spectrum")
    else {
        panic!("spatial reading");
    };
    assert!(spatial.peaks.capacity() >= MAX_SPATIAL_PEAKS);
    let Some(ProcessorReading::Stitch(stitch)) = ProcessorReading::empty("stitch") else {
        panic!("stitch reading");
    };
    assert!(stitch.lanes.capacity() >= lanes);
    let Some(ProcessorReading::Correlator(correlator)) = ProcessorReading::empty("correlator")
    else {
        panic!("correlator reading");
    };
    assert!(correlator.baselines.capacity() >= MAX_BASELINES);
    assert_eq!(MAX_BASELINES, 120);
    let Some(ProcessorReading::Polarimeter(polarimeter)) = ProcessorReading::empty("polarimeter")
    else {
        panic!("polarimeter reading");
    };
    assert!(polarimeter.at.capacity() >= MAX_AT_LEN);
    let Some(ProcessorReading::PassiveRadar(radar)) = ProcessorReading::empty("passive_radar")
    else {
        panic!("radar reading");
    };
    assert_eq!(radar, RadarUpdate::default());
    assert!(radar.detections.capacity() >= crate::radar::MAX_RADAR_DETECTIONS);
}

fn edge<T: Default>(change: impl FnOnce(&mut T)) -> T {
    let mut params = T::default();
    change(&mut params);
    params
}

fn refused<T: std::fmt::Debug>(
    cases: Vec<(T, &str)>,
    problem: impl Fn(&T) -> Option<&'static str>,
) {
    for (params, text) in cases {
        assert_eq!(problem(&params), Some(text), "{params:?}");
    }
}

fn df_edges() -> Vec<(DfParams, &'static str)> {
    let long = "s".repeat(MAX_STATION_ID_LEN + 1);
    vec![
        (
            edge(|p: &mut DfParams| p.sources = Some(0)),
            "Sources out of range",
        ),
        (
            edge(|p: &mut DfParams| p.sources = Some(16)),
            "Sources out of range",
        ),
        (
            edge(|p: &mut DfParams| p.max_peaks = 0),
            "Peaks out of range",
        ),
        (
            edge(|p: &mut DfParams| p.max_peaks = 5),
            "Peaks out of range",
        ),
        (
            edge(|p: &mut DfParams| p.smoothing = 9),
            "Smoothing out of range",
        ),
        (
            edge(|p: &mut DfParams| p.offset_hz = 1.1e8),
            "Offset out of range",
        ),
        (
            edge(|p: &mut DfParams| p.bandwidth_hz = 99.0),
            "Bandwidth out of range",
        ),
        (
            edge(|p: &mut DfParams| p.bandwidth_hz = 2.1e7),
            "Bandwidth out of range",
        ),
        (
            edge(|p: &mut DfParams| p.report_ms = 99),
            "Report out of range",
        ),
        (
            edge(|p: &mut DfParams| p.report_ms = 10_001),
            "Report out of range",
        ),
        (
            edge(|p: &mut DfParams| p.carry_over = 1.0),
            "Carry over out of range",
        ),
        (
            edge(|p: &mut DfParams| p.squelch_db = 41.0),
            "Squelch out of range",
        ),
        (
            edge(|p: &mut DfParams| p.loading = 1.5),
            "Loading out of range",
        ),
        (
            edge(|p: &mut DfParams| p.azimuth_step_deg = 0.2),
            "Step out of range",
        ),
        (
            edge(|p: &mut DfParams| p.yaw_gate_dps = 0.5),
            "Yaw gate out of range",
        ),
        (
            edge(|p: &mut DfParams| p.station_id = Some(String::new())),
            "Station out of range",
        ),
        (
            edge(|p: &mut DfParams| p.station_id = Some(long)),
            "Station out of range",
        ),
    ]
}

fn beamformer_edges() -> Vec<(BeamformerParams, &'static str)> {
    let fixed = SteerSource::Fixed {
        azimuth_deg: 0.0,
        elevation_deg: 91.0,
    };
    vec![
        (
            edge(|p: &mut BeamformerParams| p.steer = fixed),
            "Steer out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.nulls_deg = vec![1.0; 4]),
            "Nulls out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.nulls_deg = vec![f64::NAN]),
            "Nulls out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.main_lane = MAX_ARRAY_LANES),
            "Main out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.reference_lanes = vec![MAX_ARRAY_LANES]),
            "References out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.reference_lanes = vec![0]),
            "Lanes must differ",
        ),
        (
            edge(|p: &mut BeamformerParams| p.reference_lanes = vec![1, 1]),
            "Lanes must differ",
        ),
        (
            edge(|p: &mut BeamformerParams| p.taps = 0),
            "Taps out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.taps = MAX_BEAM_TAPS + 1),
            "Taps out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.step = 2.0),
            "Step out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.forget = 0.89),
            "Forget out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.crossfade_ms = 501),
            "Crossfade out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.update_ms = 49),
            "Update out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.carry_over = 0.995),
            "Carry over out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.loading = 10.5),
            "Loading out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.offset_hz = f64::INFINITY),
            "Offset out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.bandwidth_hz = Some(999.0)),
            "Bandwidth out of range",
        ),
        (
            edge(|p: &mut BeamformerParams| p.steer_timeout_ms = 99),
            "Steer timeout out of range",
        ),
    ]
}

fn spatial_edges() -> Vec<(SpatialSpectrumParams, &'static str)> {
    vec![
        (
            edge(|p: &mut SpatialSpectrumParams| p.bins = 1000),
            "Bins out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.bins = 8192),
            "Bins out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.columns = 32),
            "Columns out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| {
                p.bins = 256;
                p.columns = 512;
            }),
            "Columns out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.average_ms = 49),
            "Average out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.report_ms = 2_001),
            "Report out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.azimuth_step_deg = 7.0),
            "Step out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.azimuth_step_deg = 12.0),
            "Step out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.span_db = 9.0),
            "Span out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.offset_hz = -1.1e8),
            "Offset out of range",
        ),
        (
            edge(|p: &mut SpatialSpectrumParams| p.bandwidth_hz = Some(3e7)),
            "Bandwidth out of range",
        ),
    ]
}

fn correlator_edges() -> Vec<(CorrelatorParams, &'static str)> {
    vec![
        (
            edge(|p: &mut CorrelatorParams| p.bins = 32),
            "Bins out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.bins = 1000),
            "Bins out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.integrate_s = 0.01),
            "Integrate out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.integrate_s = 601.0),
            "Integrate out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.offset_hz = f64::NAN),
            "Offset out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.bandwidth_hz = Some(500.0)),
            "Band out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| p.channels = 8),
            "Channels out of range",
        ),
        (
            edge(|p: &mut CorrelatorParams| {
                p.bins = 128;
                p.channels = 256;
            }),
            "Channels out of range",
        ),
    ]
}

fn polarimeter_edges() -> Vec<(PolarimeterParams, &'static str)> {
    vec![
        (
            edge(|p: &mut PolarimeterParams| p.h_lane = MAX_ARRAY_LANES),
            "H out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.v_lane = MAX_ARRAY_LANES),
            "V out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.h_lane = 1),
            "Lanes must differ",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.offset_hz = 2e8),
            "Offset out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.bandwidth_hz = 50.0),
            "Bandwidth out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.report_ms = 49),
            "Report out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.average_ms = 10_001),
            "Average out of range",
        ),
        (
            edge(|p: &mut PolarimeterParams| p.crossfade_ms = 501),
            "Crossfade out of range",
        ),
    ]
}

#[test]
fn processor_ranges_refuse_their_edges() {
    refused(df_edges(), DfParams::problem);
    refused(beamformer_edges(), BeamformerParams::problem);
    refused(spatial_edges(), SpatialSpectrumParams::problem);
    refused(correlator_edges(), CorrelatorParams::problem);
    refused(polarimeter_edges(), PolarimeterParams::problem);
    assert!(edge(|p: &mut SpatialSpectrumParams| p.azimuth_step_deg = 2.5).valid());
    assert_eq!(StitchParams::default().problem(), None);
    let bad = ProcessorParams::Polarimeter(edge(|p: &mut PolarimeterParams| p.h_lane = 1));
    assert_eq!(bad.problem(), Some("Lanes must differ"));
    assert!(!bad.valid());
}

fn df_reading() -> DfReading {
    DfReading {
        at: "2026-09-28T12:00:00Z".to_owned(),
        peaks: vec![DfPeak {
            relative_deg: 10.0,
            true_deg: Some(97.0),
            elevation_deg: Some(5.0),
            power_db: -3.0,
            confidence: 0.9,
            sigma_deg: 2.0,
            mirror_deg: Some(170.0),
            mirror_true_deg: Some(257.0),
            fit: 0.95,
        }],
        pseudospectrum: vec![128; DF_POINTS],
        azimuth_deg: Some(87.0),
        station: Some(LatLon {
            lat: 52.5,
            lon: 13.4,
        }),
        sources: 1,
        sources_auto: true,
        squelched: false,
        aliasing: true,
        algorithm: DfAlgorithm::RootMusic,
        freq_hz: 433.92e6,
        heading_sigma_deg: Some(3.0),
        likelihood: vec![7; DF_POINTS],
        likelihood_true: true,
        span_db: 40.0,
        eigenvalues_db: vec![10.0, -20.0],
        eig_ratio_db: 30.0,
        lambda12_db: 30.0,
        snr_db: 20.0,
        snapshots: 1024.0,
        fit: 0.9,
        spacing_ratio: 0.4,
        aperture_wavelengths: 1.2,
        mode_aliasing: true,
        mirror: true,
        rotating: true,
        singular: true,
        table_out_of_range: true,
        gated_blocks: 3,
    }
}

fn readings() -> Vec<ProcessorReading> {
    vec![
        ProcessorReading::Df(df_reading()),
        ProcessorReading::Beamformer(BeamformerReading {
            at: "2026-09-28T12:00:00Z".to_owned(),
            sinr_gain_db: Some(9.0),
            output_db: -20.0,
            steer_deg: Some(45.0),
            nulls_deg: vec![120.0],
            weights: vec![LaneWeight {
                amplitude_db: -1.0,
                phase_deg: 30.0,
            }],
            cancelled_db: Some(25.0),
            mode: BeamMode::Lcmv,
            snr_db: Some(12.0),
            pattern: vec![255; DF_POINTS],
            null_depths_db: vec![-35.0],
            steer_age_ms: Some(250),
            loading_used: 0.1,
            resets: 2,
            out_center_hz: 433.92e6,
            out_rate: 48_000.0,
            no_steer: true,
            steer_stale: true,
            singular: true,
            diverged: true,
            band_full: true,
        }),
        ProcessorReading::SpatialSpectrum(SpatialReading {
            at: "2026-09-28T12:00:00Z".to_owned(),
            peaks: vec![SpatialPeak {
                freq_hz: 433.9e6,
                bearing_deg: 12.0,
                db: 18.0,
                true_deg: Some(99.0),
            }],
            azimuth_deg: Some(87.0),
            frames: 10,
            dropped_frames: 1,
        }),
        ProcessorReading::Stitch(StitchReading {
            at: "2026-09-28T12:00:00Z".to_owned(),
            center_hz: 100e6,
            span_hz: 10e6,
            lanes: vec![StitchLane {
                lane: 1,
                center_hz: 102e6,
                noise_eq_db: 0.5,
                coherence: Some(0.9),
                phase_deg: 12.0,
                spur_bins: 3,
            }],
            dropped_blocks: 2,
            no_overlap: true,
        }),
        ProcessorReading::Correlator(CorrelatorReading {
            at: "2026-09-28T12:00:00Z".to_owned(),
            integrated_s: 1.0,
            baselines: vec![Baseline {
                a: 0,
                b: 1,
                delay_ns: 1.5,
                coherence: 0.8,
                phase_deg: 45.0,
                length_m: 0.5,
                azimuth_deg: 90.0,
                snr_db: 20.0,
            }],
            frames: 4,
        }),
        ProcessorReading::Polarimeter(PolarimeterReading {
            at: "2026-09-28T12:00:00Z".to_owned(),
            i_db: -30.0,
            q: 0.1,
            u: 0.2,
            v: 0.9,
            degree: 0.95,
            angle_deg: 10.0,
            ellipticity_deg: 40.0,
            hand: Hand::Right,
            snr_db: Some(15.0),
            out_center_hz: 1.0,
            out_rate: 2.0,
        }),
        ProcessorReading::PassiveRadar(RadarUpdate {
            seq: 1,
            at: "2026-09-28T12:00:00Z".to_owned(),
            ..RadarUpdate::default()
        }),
    ]
}

#[test]
fn readings_round_trip_through_json() {
    for reading in readings() {
        let json = serde_json::to_value(&reading).expect("serialize");
        assert_eq!(json["type"], reading.type_id());
        assert!(json["reading"].is_object());
        let back: ProcessorReading = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reading);
    }
    let bare: DfReading = serde_json::from_str(
        r#"{"at":"x","peaks":[],"pseudospectrum":[],"sources":0,"sources_auto":true,"squelched":true,"aliasing":false}"#,
    )
    .expect("minimal df reading");
    assert_eq!(bare.algorithm, DfAlgorithm::Music);
    assert!(bare.likelihood.is_empty());
}

#[test]
fn reading_schema_tags_each_variant_with_its_type_id() {
    let schema = serde_json::to_string(&<ProcessorReading as utoipa::PartialSchema>::schema())
        .expect("schema json");
    for type_id in PROCESSOR_TYPE_IDS {
        assert!(
            schema.contains(&format!("\"{type_id}\"")),
            "{type_id} missing"
        );
    }
    for reading in readings() {
        let variant = format!("{reading:?}");
        let name = variant.split('(').next().unwrap_or_default();
        assert!(
            !schema.contains(&format!("\"{name}\"")),
            "{name} leaks into the schema"
        );
    }
}

#[test]
fn probe_params_exist_only_with_the_feature() {
    let parsed =
        serde_json::from_str::<ProcessorParams>(r#"{"type":"probe","settings":{"phase":true}}"#);
    #[cfg(feature = "probe")]
    {
        let params = parsed.expect("probe params");
        assert_eq!(params.type_id(), "probe");
        assert!(params.valid());
        assert_eq!(
            params,
            ProcessorParams::Probe(ProbeParams {
                phase: true,
                ..ProbeParams::default()
            })
        );
    }
    #[cfg(not(feature = "probe"))]
    assert!(parsed.is_err());
    assert!(!PROCESSOR_TYPE_IDS.contains(&"probe"));
}
