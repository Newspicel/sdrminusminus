use super::*;

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12)
}

fn uca(radius_m: f64, first_deg: f64, winding: Winding) -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m,
        first_deg,
        winding,
    }
}

#[test]
fn uca_positions_start_north_and_run_clockwise() {
    let positions = uca(1.0, 0.0, Winding::Clockwise)
        .positions(4)
        .expect("positions");
    assert!(close(positions[0], [0.0, 1.0, 0.0]), "{positions:?}");
    assert!(close(positions[1], [1.0, 0.0, 0.0]), "{positions:?}");
    assert!(close(positions[2], [0.0, -1.0, 0.0]), "{positions:?}");
}

#[test]
fn counter_clockwise_uca_winds_west() {
    let positions = uca(1.0, 0.0, Winding::CounterClockwise)
        .positions(4)
        .expect("positions");
    assert!(close(positions[0], [0.0, 1.0, 0.0]), "{positions:?}");
    assert!(close(positions[1], [-1.0, 0.0, 0.0]), "{positions:?}");
}

#[test]
fn uca_first_deg_rotates_element_zero() {
    let positions = uca(2.0, 90.0, Winding::Clockwise)
        .positions(4)
        .expect("positions");
    assert!(close(positions[0], [2.0, 0.0, 0.0]), "{positions:?}");
    assert!(close(positions[1], [0.0, -2.0, 0.0]), "{positions:?}");
}

#[test]
fn ula_positions_lie_on_the_axis_and_are_centred() {
    let geometry = ArrayGeometry::Ula {
        spacing_m: 0.5,
        axis_deg: 90.0,
    };
    let positions = geometry.positions(3).expect("positions");
    let xs: Vec<f64> = positions.iter().map(|p| p[0]).collect();
    for (x, want) in xs.iter().zip([-0.5, 0.0, 0.5]) {
        assert!((x - want).abs() < 1e-12, "{xs:?}");
    }
    assert!(positions.iter().all(|p| p[1].abs() < 1e-12 && p[2] == 0.0));
}

#[test]
fn explicit_positions_must_match_the_wired_lanes() {
    let geometry = ArrayGeometry::Explicit {
        positions: vec![ArrayElement::default(); 3],
    };
    assert_eq!(
        geometry.positions(4),
        Err(GeometryError::CountMismatch {
            positions: 3,
            lanes: 4
        })
    );
    let placed = ArrayGeometry::Explicit {
        positions: vec![
            ArrayElement {
                x_m: 1.0,
                y_m: 2.0,
                z_m: 3.0,
            },
            ArrayElement {
                x_m: -1.0,
                y_m: 0.0,
                z_m: 0.0,
            },
        ],
    };
    assert_eq!(
        placed.positions(2),
        Ok(vec![[1.0, 2.0, 3.0], [-1.0, 0.0, 0.0]])
    );
}

#[test]
fn geometry_outside_the_extent_is_refused() {
    assert_eq!(
        uca(100.5, 0.0, Winding::Clockwise).positions(4),
        Err(GeometryError::OutOfRange)
    );
    assert_eq!(
        ArrayGeometry::Ula {
            spacing_m: 20.0,
            axis_deg: 0.0
        }
        .positions(16),
        Err(GeometryError::OutOfRange)
    );
    let far = ArrayGeometry::Explicit {
        positions: vec![
            ArrayElement::default(),
            ArrayElement {
                x_m: 0.0,
                y_m: 0.0,
                z_m: 101.0,
            },
        ],
    };
    assert_eq!(far.positions(2), Err(GeometryError::OutOfRange));
    assert_eq!(far.problem(), Some("Positions out of range"));
    assert_eq!(
        uca(f64::NAN, 0.0, Winding::Clockwise).positions(4),
        Err(GeometryError::NotFinite)
    );
    assert_eq!(
        uca(1.0, 0.0, Winding::Clockwise).positions(1),
        Err(GeometryError::TooFewLanes)
    );
    assert_eq!(
        uca(1.0, 0.0, Winding::Clockwise).positions(17),
        Err(GeometryError::TooManyLanes)
    );
    assert_eq!(
        uca(0.0, 0.0, Winding::Clockwise).problem(),
        Some("Radius out of range")
    );
    assert_eq!(
        uca(1.0, f64::INFINITY, Winding::Clockwise).problem(),
        Some("First element out of range")
    );
    assert_eq!(
        ArrayGeometry::Ula {
            spacing_m: -1.0,
            axis_deg: 0.0
        }
        .problem(),
        Some("Spacing out of range")
    );
    assert_eq!(
        ArrayGeometry::Ula {
            spacing_m: 1.0,
            axis_deg: f64::NAN
        }
        .problem(),
        Some("Axis out of range")
    );
}

#[test]
fn unambiguous_hz_follows_adjacent_spacing() {
    let ula = ArrayGeometry::Ula {
        spacing_m: 0.5,
        axis_deg: 90.0,
    };
    assert_eq!(ula.adjacent_spacing_m(4), Some(0.5));
    assert_eq!(ula.unambiguous_hz(4), Some(LIGHT_SPEED_M_S));

    let circle = uca(1.0, 0.0, Winding::Clockwise);
    let chord = circle.adjacent_spacing_m(4).expect("chord");
    assert!((chord - std::f64::consts::SQRT_2).abs() < 1e-12, "{chord}");
    let hz = circle.unambiguous_hz(4).expect("hz");
    assert!((hz - LIGHT_SPEED_M_S / (2.0 * chord)).abs() < 1e-3);

    let explicit = ArrayGeometry::Explicit {
        positions: vec![
            ArrayElement::default(),
            ArrayElement {
                x_m: 3.0,
                y_m: 0.0,
                z_m: 0.0,
            },
            ArrayElement {
                x_m: 0.0,
                y_m: 0.0,
                z_m: 0.25,
            },
        ],
    };
    assert_eq!(explicit.adjacent_spacing_m(3), Some(0.25));
    assert_eq!(explicit.adjacent_spacing_m(4), None);
    assert_eq!(circle.unambiguous_hz(1), None);
}

#[test]
fn array_node_defaults_are_valid() {
    let node = ArrayNode::default();
    assert!(node.valid(), "{:?}", node.problem());
    assert_eq!(node.declared, Coherence::TimeSync);
    assert_eq!(node.geometry.positions(5).map(|p| p.len()), Ok(5));
    let parsed: ArrayNode = serde_json::from_str("{}").expect("empty body");
    assert_eq!(parsed, node);
    assert!(ArrayGain::default().problem().is_none());
}

#[test]
fn a_declared_tier_of_none_is_invalid() {
    let node = ArrayNode {
        declared: Coherence::None,
        ..ArrayNode::default()
    };
    assert_eq!(node.problem(), Some("Tier out of range"));
    let azimuth = ArrayNode {
        orientation: ArrayOrientation::Fixed {
            azimuth_deg: f64::NAN,
        },
        ..ArrayNode::default()
    };
    assert_eq!(azimuth.problem(), Some("Azimuth out of range"));
    let mount = ArrayNode {
        orientation: ArrayOrientation::Heading {
            mount_offset_deg: f64::INFINITY,
        },
        ..ArrayNode::default()
    };
    assert_eq!(mount.problem(), Some("Mount out of range"));
}

fn with_source(source: ArrayCalSource) -> ArrayNode {
    ArrayNode {
        cal: ArrayCal {
            source,
            ..ArrayCal::default()
        },
        ..ArrayNode::default()
    }
}

#[test]
fn cal_source_offsets_and_bandwidths_are_bounded() {
    let pilot = |offset_hz, bandwidth_hz| {
        with_source(ArrayCalSource::Pilot {
            offset_hz,
            bandwidth_hz,
        })
        .problem()
    };
    assert_eq!(pilot(MAX_CAL_OFFSET_HZ, MAX_CAL_BANDWIDTH_HZ), None);
    assert_eq!(pilot(-MAX_CAL_OFFSET_HZ, MIN_CAL_BANDWIDTH_HZ), None);
    assert_eq!(
        pilot(MAX_CAL_OFFSET_HZ + 1.0, 1_000.0),
        Some("Offset out of range")
    );
    assert_eq!(pilot(f64::NAN, 1_000.0), Some("Offset out of range"));
    assert_eq!(
        pilot(0.0, MIN_CAL_BANDWIDTH_HZ - 1.0),
        Some("Width out of range")
    );
    assert_eq!(
        pilot(0.0, MAX_CAL_BANDWIDTH_HZ + 1.0),
        Some("Width out of range")
    );
    let emitter = with_source(ArrayCalSource::Emitter {
        offset_hz: 0.0,
        bandwidth_hz: 1_000.0,
        bearing_deg: f64::NAN,
    });
    assert_eq!(emitter.problem(), Some("Bearing out of range"));
    for check_s in [0, MIN_CHECK_S, MAX_CHECK_S] {
        let node = ArrayNode {
            cal: ArrayCal {
                check_s,
                ..ArrayCal::default()
            },
            ..ArrayNode::default()
        };
        assert!(node.valid(), "{check_s}");
    }
    for check_s in [MIN_CHECK_S - 1, MAX_CHECK_S + 1] {
        let node = ArrayNode {
            cal: ArrayCal {
                check_s,
                ..ArrayCal::default()
            },
            ..ArrayNode::default()
        };
        assert_eq!(node.problem(), Some("Check out of range"));
    }
    assert_eq!(
        ArrayGain::Manual {
            db: MAX_ARRAY_GAIN_DB + 0.5
        }
        .problem(),
        Some("Gain out of range")
    );
    assert_eq!(ArrayCalSource::Off.kind(), None);
    assert_eq!(ArrayCalSource::Noise.kind(), Some(CalSourceKind::Noise));
}

#[test]
fn array_status_round_trips_through_json() {
    let status = ArrayStatus {
        node: "array-1".to_owned(),
        lanes: vec![ArrayLaneStatus {
            lane: 1,
            device_set: Some(3),
            stream: 1,
            sync: SyncState::Locked,
            delay_samples: 12.25,
            phase_deg: -41.5,
            gain_db: 0.5,
            coherence: 0.98,
            residual_delay: Some(0.01),
            residual_phase_deg: Some(0.4),
            level_dbfs: -31.0,
            clipping: false,
            gaps: 2,
            gap_samples: 4096,
            uncertain: 16,
        }],
        anchor: Some(3),
        tier: Coherence::PhaseCoherent,
        declared: Coherence::PhaseCoherent,
        tier_capped: false,
        sync: SyncState::Locked,
        cal: CalPhase::Solved,
        phase_ready: true,
        center_hz: 433_920_000.0,
        sample_rate: 2_400_000.0,
        tuning: ArrayTuningMode::Together,
        gain: ArrayGain::Auto,
        gain_db: Some(28.0),
        gain_range_db: Some(Range {
            min: 0.0,
            max: 49.6,
            step: None,
        }),
        generation: 4,
        realigns: 1,
        dropped_samples: 12,
        events_lost: 0,
        drift_ppm: Some(0.2),
        last_solve_at: Some("2026-09-28T12:00:00Z".to_owned()),
        next_check_in_s: Some(42.0),
        azimuth_deg: Some(87.5),
        heading_source: Some(HeadingSource::Fused),
        position: Some(LatLon {
            lat: 52.5,
            lon: 13.4,
        }),
        unambiguous_hz: Some(428_000_000.0),
        failure: Some(ArrayFailure::LaneHeld {
            lane: 2,
            by: "Array 2".to_owned(),
        }),
        processors: vec![ProcessorStatus {
            node: "df-1".to_owned(),
            kind: "df".to_owned(),
            running: true,
            gated: Some(ProcessorGate::Phase),
            gated_samples: 10,
            dropped_samples: 1,
            dropped_reports: 2,
            lane_overflows: 3,
            lane_mismatch: 4,
            solver_failures: 5,
            resets: 6,
            truncated: 7,
            error: Some("Too many sources".to_owned()),
        }],
        recording: Some(ArrayRecordingStatus {
            stem: "array-1".to_owned(),
            started_at: "2026-09-28T12:00:00Z".to_owned(),
            samples: 1,
            dropped: 0,
            error: None,
        }),
    };
    let json = serde_json::to_value(&status).expect("serialize");
    assert_eq!(json["failure"]["kind"], "lane_held");
    assert_eq!(json["gain"]["kind"], "auto");
    assert_eq!(json["heading_source"], "fused");
    let back: ArrayStatus = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, status);

    let bare = ArrayStatus::default();
    let json = serde_json::to_value(&bare).expect("serialize");
    assert!(json.get("processors").is_none());
    assert!(json.get("failure").is_none());
    assert_eq!(
        serde_json::from_value::<ArrayStatus>(json).expect("back"),
        bare
    );
}

#[test]
fn failures_render_their_labels_with_one_based_lanes() {
    let cases = [
        (ArrayFailure::Unwired, "Wire lanes"),
        (ArrayFailure::LaneGap { lane: 1 }, "Lane 2 unwired"),
        (ArrayFailure::DuplicateLane { lane: 0 }, "Lane 1 twice"),
        (
            ArrayFailure::LaneHeld {
                lane: 3,
                by: "Roof".to_owned(),
            },
            "Lane 4 in Roof",
        ),
        (ArrayFailure::DeviceDown { lane: 4 }, "Radio 5 down"),
        (
            ArrayFailure::NoiseClips { lane: 2 },
            "Noise clips, lower gain",
        ),
        (
            ArrayFailure::LowCoherence {
                lane: 5,
                coherence: 0.2,
            },
            "Lane 6 weak",
        ),
        (
            ArrayFailure::ClockDrift { ppm: 1.26 },
            "Clocks drift 1.3 ppm",
        ),
        (
            ArrayFailure::GeometryMismatch {
                positions: 4,
                lanes: 5,
            },
            "Geometry has 4, wired 5",
        ),
        (
            ArrayFailure::Stopped {
                message: "io".to_owned(),
            },
            "Stopped",
        ),
    ];
    for (failure, text) in cases {
        assert_eq!(failure.to_string(), text);
        let json = serde_json::to_value(&failure).expect("serialize");
        assert_eq!(json["kind"], failure.kind());
    }
    assert_eq!(ARRAY_FAILURE_LABELS.len(), 21);
    assert!(
        ARRAY_FAILURE_LABELS
            .iter()
            .all(|(_, text)| !text.is_empty())
    );
}

#[test]
fn state_labels_match_the_table() {
    let sync: Vec<_> = SyncState::ALL.iter().map(|s| s.label()).collect();
    assert_eq!(sync, ["Idle", "Syncing", "Locked", "Drifting", "Lost"]);
    let cal: Vec<_> = CalPhase::ALL.iter().map(|c| c.label()).collect();
    assert_eq!(
        cal,
        [
            "No cal",
            "Waiting",
            "Calibrating",
            "Calibrated",
            "Warm",
            "Stale",
            "Cal failed"
        ]
    );
    let gate: Vec<_> = ProcessorGate::ALL.iter().map(|g| g.label()).collect();
    assert_eq!(
        gate,
        [
            "Syncing",
            "Needs cal",
            "Needs cal",
            "Calibrating",
            "Retuning",
            "Not coherent",
            "Wrong tuning"
        ]
    );
}

#[test]
fn cal_records_and_lanes_round_trip() {
    let record = ArrayCalRecord {
        lanes: vec![LaneKey {
            device: "rtlsdr:1000".to_owned(),
            stream: 0,
        }],
        center_hz: 100e6,
        sample_rate: 2.4e6,
        gain_db: Some(20.0),
        source: CalSourceKind::Noise,
        keeps_phase: false,
        solved_at: "2026-09-28T12:00:00Z".to_owned(),
        solution: vec![LaneSolution {
            delay_samples: 1.5,
            phase_deg: 10.0,
            gain_db: -0.5,
            coherence: 0.99,
            equaliser: vec![[1.0, 0.0]; EQ_POINTS],
        }],
    };
    let json = serde_json::to_value(&record).expect("serialize");
    assert_eq!(json["solution"][0]["equaliser"][0][0], 1.0);
    assert_eq!(
        serde_json::from_value::<ArrayCalRecord>(json).expect("back"),
        record
    );
    let lane = VirtualLane {
        stream: 5,
        node: "beam".to_owned(),
        port: "beam".to_owned(),
        center_hz: 1.0,
        sample_rate: 2.0,
    };
    let json = serde_json::to_string(&lane).expect("serialize");
    assert_eq!(
        serde_json::from_str::<VirtualLane>(&json).expect("back"),
        lane
    );
    let tune: ArrayTuneRequest =
        serde_json::from_str(r#"{"gain":{"kind":"manual","db":12}}"#).expect("tune request");
    assert_eq!(tune.center_hz, None);
    assert_eq!(tune.gain, Some(ArrayGain::Manual { db: 12.0 }));
}

fn every_failure() -> [ArrayFailure; 21] {
    [
        ArrayFailure::Unwired,
        ArrayFailure::LaneGap { lane: 0 },
        ArrayFailure::DuplicateLane { lane: 0 },
        ArrayFailure::TooManyLanes,
        ArrayFailure::LaneHeld {
            lane: 0,
            by: "A".to_owned(),
        },
        ArrayFailure::DeviceDown { lane: 0 },
        ArrayFailure::NotCoherent,
        ArrayFailure::RatesDiffer,
        ArrayFailure::SpreadUnsupported,
        ArrayFailure::NoNoiseSource,
        ArrayFailure::NoiseShared,
        ArrayFailure::NoiseNotSeen,
        ArrayFailure::NoiseClips { lane: 0 },
        ArrayFailure::LowCoherence {
            lane: 0,
            coherence: 0.1,
        },
        ArrayFailure::NoCommonSignal,
        ArrayFailure::ClockDrift { ppm: 2.0 },
        ArrayFailure::SlipsRepeated,
        ArrayFailure::GeometryMismatch {
            positions: 2,
            lanes: 3,
        },
        ArrayFailure::NeedsPosition,
        ArrayFailure::Stopped {
            message: "gone".to_owned(),
        },
        ArrayFailure::Busy,
    ]
}

#[test]
fn every_failure_has_one_label() {
    let failures = every_failure();
    let kinds: Vec<&str> = failures.iter().map(ArrayFailure::kind).collect();
    let listed: Vec<&str> = ARRAY_FAILURE_LABELS.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, listed);
    for failure in &failures {
        let json = serde_json::to_value(failure).expect("serialize");
        assert_eq!(json["kind"], failure.kind());
        assert!(!failure.template().is_empty(), "{failure:?}");
        let text = failure.to_string();
        assert!(!text.is_empty() && !text.contains('{'), "{text}");
        assert_eq!(
            serde_json::from_value::<ArrayFailure>(json).expect("back"),
            *failure
        );
    }
}
