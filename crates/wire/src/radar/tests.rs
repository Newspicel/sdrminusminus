use super::*;

fn with(change: impl FnOnce(&mut PassiveRadarParams)) -> PassiveRadarParams {
    let mut params = PassiveRadarParams::default();
    change(&mut params);
    params
}

#[test]
fn radar_defaults_are_valid() {
    let params = PassiveRadarParams::default();
    assert!(params.valid(), "{:?}", params.problem());
    assert_eq!(
        serde_json::from_str::<PassiveRadarParams>("{}").expect("empty body"),
        params
    );
}

#[test]
fn every_rule_names_its_problem() {
    let cases: Vec<(PassiveRadarParams, &str)> = vec![
        (
            with(|p| p.cpi_ms = RADAR_MIN_CPI_MS - 1),
            "CPI out of range",
        ),
        (
            with(|p| p.cpi_ms = RADAR_MAX_CPI_MS + 1),
            "CPI out of range",
        ),
        (with(|p| p.overlap = 0.8), "Overlap out of range"),
        (with(|p| p.max_range_km = 0.0), "Range out of range"),
        (with(|p| p.max_range_km = 401.0), "Range out of range"),
        (with(|p| p.max_speed_mps = 9.0), "Speed out of range"),
        (with(|p| p.offset_hz = f64::NAN), "Offset out of range"),
        (with(|p| p.offset_hz = 60e6), "Offset out of range"),
        (
            with(|p| {
                p.illuminator = Illuminator::Custom { bandwidth_hz: 5e3 };
            }),
            "Bandwidth out of range",
        ),
        (
            with(|p| p.reference_element = MAX_ARRAY_LANES),
            "Reference out of range",
        ),
        (
            with(|p| p.surveillance = SurveillanceSet::Mask { mask: 0 }),
            "No surveillance element",
        ),
        (
            with(|p| p.surveillance = SurveillanceSet::Mask { mask: 1 << 16 }),
            "No surveillance element",
        ),
        (
            with(|p| p.surveillance = SurveillanceSet::Mask { mask: 0b11 }),
            "Surveillance holds the reference",
        ),
        (with(|p| p.illuminator = Illuminator::Dab), "CMA needs FM"),
        (
            with(|p| p.reference = ReferenceCleaning::DabRemod),
            "DAB remod needs DAB",
        ),
        (
            with(|p| {
                p.reference = ReferenceCleaning::Cma {
                    taps: 0,
                    step: 1e-3,
                }
            }),
            "CMA taps out of range",
        ),
        (
            with(|p| {
                p.reference = ReferenceCleaning::Cma {
                    taps: 16,
                    step: 0.0,
                }
            }),
            "CMA step out of range",
        ),
        (
            with(|p| p.clutter.reach_km = 0.05),
            "Clutter reach out of range",
        ),
        (
            with(|p| p.clutter.lead = MAX_ECA_LEAD + 1),
            "Lead out of range",
        ),
        (
            with(|p| p.clutter.doppler_taps = MAX_ECA_DOPPLER_TAPS + 1),
            "Doppler taps out of range",
        ),
        (with(|p| p.clutter.batch_ms = 0.5), "Batch out of range"),
        (
            with(|p| p.clutter.extension_ms = 501.0),
            "Extension out of range",
        ),
        (with(|p| p.clutter.step = 0.0), "Step out of range"),
        (with(|p| p.clutter.loading = 1.5), "Loading out of range"),
        (
            with(|p| p.cfar.kind = CfarKind::Os { rank: 0.4 }),
            "Rank out of range",
        ),
        (with(|p| p.cfar.pfa = 0.5), "Pfa out of range"),
        (with(|p| p.cfar.guard_range = 17), "Guard out of range"),
        (with(|p| p.cfar.guard_doppler = 17), "Guard out of range"),
        (with(|p| p.cfar.train_range = 0), "Train out of range"),
        (with(|p| p.cfar.train_doppler = 17), "Train out of range"),
        (
            with(|p| p.cfar.min_doppler_hz = 101.0),
            "Min Doppler out of range",
        ),
        (
            with(|p| p.cfar.min_range_km = 51.0),
            "Min range out of range",
        ),
        (with(|p| p.cfar.min_snr_db = -1.0), "Min SNR out of range"),
        (with(|p| p.tracker.confirm_hits = 0), "M of N out of range"),
        (
            with(|p| {
                p.tracker.confirm_hits = 6;
                p.tracker.confirm_window = 5;
            }),
            "M of N out of range",
        ),
        (
            with(|p| p.tracker.confirm_window = 17),
            "M of N out of range",
        ),
        (with(|p| p.tracker.coast_looks = 101), "Coast out of range"),
        (
            with(|p| p.tracker.max_accel_mps2 = 0.5),
            "Accel out of range",
        ),
        (with(|p| p.tracker.gate = 31.0), "Gate out of range"),
        (with(|p| p.tracker.jerk = 0.0), "Jerk out of range"),
        (
            with(|p| p.assumed_altitude_m = 15_001.0),
            "Altitude out of range",
        ),
    ];
    for (params, text) in cases {
        assert_eq!(params.problem(), Some(text), "{params:?}");
        assert!(!params.valid());
    }
}

#[test]
fn dab_and_dvbt_illuminators_are_valid_with_their_cleaning() {
    let dab = with(|p| {
        p.illuminator = Illuminator::Dab;
        p.reference = ReferenceCleaning::DabRemod;
    });
    assert!(dab.valid(), "{:?}", dab.problem());
    let dvbt = with(|p| {
        p.illuminator = Illuminator::DvbtPartial {
            bandwidth_hz: DVBT_BANDWIDTH_HZ,
        };
        p.reference = ReferenceCleaning::Off;
    });
    assert!(dvbt.valid(), "{:?}", dvbt.problem());
    let custom_cma = with(|p| p.illuminator = Illuminator::Custom { bandwidth_hz: 1e6 });
    assert!(custom_cma.valid());
}

#[test]
fn illuminators_name_their_bandwidth() {
    assert_eq!(Illuminator::Fm.bandwidth_hz(), FM_BANDWIDTH_HZ);
    assert_eq!(Illuminator::Dab.bandwidth_hz(), DAB_BANDWIDTH_HZ);
    assert_eq!(
        Illuminator::DvbtPartial { bandwidth_hz: 1e6 }.bandwidth_hz(),
        1e6
    );
    let labels: Vec<_> = [
        Illuminator::Fm,
        Illuminator::Dab,
        Illuminator::DvbtPartial { bandwidth_hz: 1e6 },
        Illuminator::Custom { bandwidth_hz: 1e6 },
    ]
    .iter()
    .map(Illuminator::label)
    .collect();
    assert_eq!(labels, ["FM", "DAB", "DVB-T", "Custom"]);
}

#[test]
fn surveillance_all_others_skips_the_reference() {
    let all: Vec<u32> = SurveillanceSet::AllOthers.elements(2, 5).collect();
    assert_eq!(all, [0, 1, 3, 4]);
    let mask: Vec<u32> = SurveillanceSet::Mask { mask: 0b1_0110 }
        .elements(1, 4)
        .collect();
    assert_eq!(mask, [2]);
}

#[test]
fn max_eca_order_matches_the_solver_limit() {
    assert_eq!(MAX_ECA_ORDER, 512);
}

#[test]
fn radar_problems_name_their_labels() {
    let problems = RadarProblem::ALL;
    let labels: Vec<&str> = problems.iter().map(RadarProblem::label).collect();
    assert_eq!(
        labels,
        [
            "No array",
            "No transmitter",
            "No array position",
            "No heading",
            "Phase unknown",
            "Outside cal table",
            "Overloaded",
            "Reference lost",
            "",
        ]
    );
    let refused = RadarProblem::Refused("CPI too large".to_owned());
    assert_eq!(refused.label(), "CPI too large");
    let json = serde_json::to_value(&refused).expect("serialize");
    assert_eq!(json["kind"], "refused");
    assert_eq!(json["detail"], "CPI too large");
    for problem in RadarProblem::ALL {
        let json = serde_json::to_value(&problem).expect("serialize");
        assert_eq!(json["kind"], problem.kind());
    }
}

fn full_update() -> RadarUpdate {
    let aoa = RadarAoa {
        azimuth_deg: 12.0,
        bearing_deg: Some(99.0),
        sigma_deg: 3.0,
        quality: 0.8,
        mirror_deg: Some(168.0),
    };
    RadarUpdate {
        seq: 42,
        at: "2026-09-28T12:00:00Z".to_owned(),
        axes: RadarAxes {
            sample_rate_hz: 266_666.0,
            carrier_hz: 98.5e6,
            range_step_m: 1_124.0,
            gates: 72,
            doppler_step_hz: 2.0,
            doppler_rows: 255,
            batches: 256,
            cpi_ms: 500.0,
            hop_ms: 500.0,
            lanes: 5,
        },
        detections: vec![RadarDetection {
            range_km: 32.5,
            doppler_hz: 120.0,
            range_rate_mps: -365.0,
            snr_db: 17.0,
            cells: 6,
            aoa: Some(aoa),
            track_id: Some(7),
        }],
        tracks: vec![RadarTrack {
            id: 7,
            state: TrackState::Coasting,
            range_km: 32.5,
            range_rate_mps: -365.0,
            doppler_hz: 120.0,
            accel_mps2: 1.0,
            range_sigma_m: 300.0,
            rate_sigma_mps: 4.0,
            snr_db: 17.0,
            looks: 9,
            misses: 1,
            aoa: Some(aoa),
            fix: Some(RadarFix {
                lat: 52.4,
                lon: 13.2,
                alt_m: 9_000.0,
                alt_from_adsb: true,
                major_m: 2_000.0,
                minor_m: 500.0,
                orientation_deg: 40.0,
            }),
            adsb: Some(AdsbMatch {
                icao: "3C6444".to_owned(),
                callsign: Some("DLH4AB".to_owned()),
            }),
            trail: vec![RadarTrailPoint {
                range_km: 33.0,
                doppler_hz: 118.0,
            }],
        }],
        truth: vec![AdsbTruth {
            icao: "3C6444".to_owned(),
            callsign: Some("DLH4AB".to_owned()),
            range_km: 32.4,
            doppler_hz: 121.0,
            bearing_deg: 98.0,
            lat: 52.4,
            lon: 13.2,
            altitude_m: 9_100.0,
            age_s: 0.5,
            in_view: true,
            track_id: Some(7),
        }],
        health: RadarHealth {
            suppression_db: vec![42.0; 4],
            unsuppressed_groups: 1,
            dropped_samples: 2,
            dropped_cpis: 3,
            discarded_cpis: 4,
            dropped_reports: 5,
            lagged_updates: 6,
            truncated_detections: 7,
            dropped_tracks: 8,
            gpu_failures: 9,
            load: 0.4,
            front_load: 0.2,
            compute_ms: 120.0,
            noise_floor_db: -80.0,
            cfar_looks: 1,
            range_correlation: 1.3,
            gpu: true,
            threads: 4,
            aoa: AoaState::Ready,
            table_out_of_range: true,
            reference: ReferenceHealth {
                mode: ReferenceMode::Cma,
                locked: true,
                quality_db: 20.0,
                fallback_frames: 1,
            },
        },
        geometry: Some(RadarGeometry {
            receiver: RadarSite {
                lat: 52.5,
                lon: 13.4,
                altitude_m: 40.0,
            },
            transmitter: RadarSite {
                lat: 52.52,
                lon: 13.41,
                altitude_m: 368.0,
            },
            baseline_km: 2.3,
            heading_deg: Some(87.0),
        }),
        problems: vec![RadarProblem::NoHeading],
        events: Vec::new(),
    }
}

#[test]
fn radar_update_round_trips_json() {
    let update = full_update();
    let json = serde_json::to_value(&update).expect("serialize");
    assert_eq!(json["problems"][0]["kind"], "no_heading");
    assert_eq!(json["tracks"][0]["state"], "coasting");
    assert!(json.get("events").is_none());
    assert_eq!(json["health"]["table_out_of_range"], true);
    let back: RadarUpdate = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, update);
}

#[test]
fn a_health_written_before_the_table_flag_reads_as_covered() {
    let mut json = serde_json::to_value(full_update().health).expect("serialize");
    json.as_object_mut()
        .map(|health| health.remove("table_out_of_range"));
    let health: RadarHealth = serde_json::from_value(json).expect("deserialize");
    assert!(!health.table_out_of_range);
    let problem = serde_json::to_value(RadarProblem::TableOutOfRange).expect("serialize");
    assert_eq!(problem, serde_json::json!({"kind": "table_out_of_range"}));
}

#[test]
fn track_events_never_reach_the_wire() {
    let mut update = full_update();
    update.events.push(RadarTrackEvent {
        track_id: 7,
        change: TrackChange::Confirmed,
        range_km: 32.5,
        range_rate_mps: -365.0,
        doppler_hz: 120.0,
        snr_db: 17.0,
        bearing_deg: None,
        lat: None,
        lon: None,
        icao: None,
    });
    let json = serde_json::to_string(&update).expect("serialize");
    let back: RadarUpdate = serde_json::from_str(&json).expect("deserialize");
    assert!(back.events.is_empty());
}

#[test]
fn a_reserved_update_holds_every_list_at_its_limit() {
    let update = RadarUpdate::reserved();
    assert!(update.detections.capacity() >= MAX_RADAR_DETECTIONS);
    assert!(update.tracks.capacity() >= MAX_RADAR_TRACKS);
    assert!(update.truth.capacity() >= MAX_RADAR_TRUTH);
    assert!(update.events.capacity() >= MAX_RADAR_TRACKS);
    assert!(update.health.suppression_db.capacity() >= MAX_ARRAY_LANES as usize);
    assert!(update.detections.is_empty() && update.tracks.is_empty());
}

#[test]
fn the_first_broken_rule_is_named() {
    let cases = [
        (
            with(|p| {
                p.cpi_ms = 0;
                p.assumed_altitude_m = -1.0;
            }),
            "CPI out of range",
        ),
        (
            with(|p| {
                p.reference_element = MAX_ARRAY_LANES;
                p.surveillance = SurveillanceSet::Mask { mask: 0 };
            }),
            "Reference out of range",
        ),
        (
            with(|p| {
                p.surveillance = SurveillanceSet::Mask { mask: 0b1 };
                p.illuminator = Illuminator::Dab;
            }),
            "Surveillance holds the reference",
        ),
        (
            with(|p| {
                p.illuminator = Illuminator::Dab;
                p.clutter.reach_km = 0.0;
            }),
            "CMA needs FM",
        ),
        (
            with(|p| {
                p.clutter.loading = 2.0;
                p.cfar.pfa = 1.0;
            }),
            "Loading out of range",
        ),
        (
            with(|p| {
                p.cfar.min_snr_db = 41.0;
                p.tracker.gate = 0.0;
            }),
            "Min SNR out of range",
        ),
        (
            with(|p| {
                p.tracker.jerk = f32::NAN;
                p.assumed_altitude_m = f32::NAN;
            }),
            "Jerk out of range",
        ),
    ];
    for (params, text) in cases {
        assert_eq!(params.problem(), Some(text), "{params:?}");
    }
}
