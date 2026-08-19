use super::*;

#[test]
fn an_lo_offset_no_channel_sits_on_is_left_where_it_was_asked_for() {
    let placed = plan_front_end(
        &tuner_caps(),
        &offset_settings(250_000.0),
        &[parked(1, 0.0)],
    )
    .lo_offset_hz;
    assert_eq!(placed, 250_000.0);
}

#[test]
fn the_lo_steps_aside_when_a_decoder_is_parked_on_it() {
    let settings = offset_settings(250_000.0);
    let on_the_artifact = parked(1, -250_000.0);
    assert!(
        !artifact_clears_channels(
            250_000.0,
            &settings,
            &tuner_caps(),
            std::slice::from_ref(&on_the_artifact)
        ),
        "the test channel was not actually sitting on the artifact"
    );

    let placed = plan_front_end(
        &tuner_caps(),
        &settings,
        std::slice::from_ref(&on_the_artifact),
    )
    .lo_offset_hz;
    assert_ne!(placed, 250_000.0, "the LO stayed under the decoder");
    assert!(
        artifact_clears_channels(
            placed,
            &settings,
            &tuner_caps(),
            std::slice::from_ref(&on_the_artifact)
        ),
        "the LO moved to another spot that is still inside the decoder"
    );
    assert!(
        placed.abs() <= sdrmm_wire::lo_offset_limit_hz(2_400_000.0),
        "the LO was pushed outside the tuner's flat passband"
    );
}

#[test]
fn the_lo_finds_a_gap_between_several_decoders() {
    let settings = offset_settings(250_000.0);
    let crowded = [
        parked(1, -250_000.0),
        parked(2, 250_000.0),
        parked(3, -187_500.0),
        parked(4, 187_500.0),
    ];
    let placed = plan_front_end(&tuner_caps(), &settings, &crowded).lo_offset_hz;
    assert!(
        artifact_clears_channels(placed, &settings, &tuner_caps(), &crowded),
        "no placement cleared four decoders, settled on {placed} Hz"
    );
}

#[test]
fn an_offset_that_was_never_asked_for_is_not_invented_to_dodge_a_channel() {
    let placed =
        plan_front_end(&tuner_caps(), &offset_settings(0.0), &[parked(1, 0.0)]).lo_offset_hz;
    assert_eq!(placed, 0.0);
}

#[test]
fn a_radio_that_has_not_said_where_it_is_tuned_keeps_its_lo_where_it_is() {
    let untuned = DeviceSettings {
        center_hz: None,
        ..offset_settings(250_000.0)
    };
    assert_eq!(
        plan_front_end(&tuner_caps(), &untuned, &[]).lo_offset_hz,
        0.0,
        "a centre the front end cannot displace was displaced anyway"
    );
}

#[test]
fn managed_hardware_parks_its_artifact_without_being_asked() {
    let plan = plan_front_end(&managed_caps(), &untouched_settings(), &[parked(1, 0.0)]);
    assert_ne!(
        plan.lo_offset_hz, 0.0,
        "the artifact was left under the tune frequency"
    );
    assert!(
        plan.dc_block,
        "the artifact was moved clear but never removed"
    );
    assert!(
        artifact_clears_channels(
            plan.lo_offset_hz,
            &untouched_settings(),
            &managed_caps(),
            &[parked(1, 0.0)]
        ),
        "the artifact landed inside the decoder it was moved to avoid"
    );
}

#[test]
fn managed_hardware_ignores_the_operator_settings_it_does_not_show() {
    let asked = DeviceSettings {
        lo_offset_hz: Some(0.0),
        dc_block: Some(false),
        ..untouched_settings()
    };
    let plan = plan_front_end(&managed_caps(), &asked, &[]);
    assert_ne!(plan.lo_offset_hz, 0.0);
    assert!(plan.dc_block);
}

#[test]
fn the_blocker_stays_off_while_the_artifact_has_nowhere_to_go() {
    let boxed_in = Capabilities {
        freq_ranges: vec![sdrmm_wire::Range {
            min: 100e6,
            max: 100e6,
            step: None,
        }],
        ..managed_caps()
    };
    let plan = plan_front_end(&boxed_in, &untouched_settings(), &[parked(1, 0.0)]);
    assert_eq!(
        plan.lo_offset_hz, 0.0,
        "the LO was displaced to a frequency the tuner cannot reach"
    );
    assert!(
        !plan.dc_block,
        "the blocker notched the centre bin with a decoder sitting on it"
    );
}

#[test]
fn unknown_hardware_still_answers_to_its_settings() {
    let asked = DeviceSettings {
        dc_block: Some(true),
        ..offset_settings(250_000.0)
    };
    let plan = plan_front_end(&tuner_caps(), &asked, &[]);
    assert_eq!(plan.lo_offset_hz, 250_000.0);
    assert!(plan.dc_block);

    let quiet = DeviceSettings {
        dc_block: Some(false),
        ..offset_settings(0.0)
    };
    let plan = plan_front_end(&tuner_caps(), &quiet, &[]);
    assert_eq!(plan.lo_offset_hz, 0.0);
    assert!(!plan.dc_block);
}

#[tokio::test]
async fn a_channel_added_over_the_lo_pushes_it_aside() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    const OFFSET: f64 = 250_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                lo_offset_hz: Some(OFFSET),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();
    let before = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() < 1.0).await;
    assert_eq!(before.center_hz - before.lo_hz, OFFSET);

    engine.add_channel(ds, 0, nfm_settings(-OFFSET)).unwrap();

    let after = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() > 1.0).await;
    assert_eq!(
        after.center_hz, before.center_hz,
        "moving the LO dragged the operator's centre with it"
    );

    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn offset_tuning_moves_the_lo_without_moving_what_is_on_the_air() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let centred = snapshot_once(&mut rx, |s| s.lo_hz == s.center_hz).await;
    let marker = peak_hz(&centred);

    const OFFSET: f64 = 250_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                lo_offset_hz: Some(OFFSET),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let displaced = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() < 1.0).await;
    assert_eq!(
        displaced.center_hz, centred.center_hz,
        "the frequency the operator asked for moved with the LO"
    );

    let bin_hz = f64::from(displaced.span_hz) / displaced.db.len() as f64;
    let moved = marker - peak_hz(&displaced);
    assert!(
        (moved - OFFSET).abs() <= MARKER_SPREAD_HZ + bin_hz,
        "the marker should follow the LO down by {OFFSET} Hz, it moved {moved} Hz"
    );

    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn blocking_dc_leaves_a_carrier_off_centre_alone() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let plain = snapshot_once(&mut rx, |s| !s.db.is_empty()).await;
    let marker = peak_hz(&plain);

    engine
        .patch_device(
            ds,
            DeviceSettings {
                dc_block: Some(true),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let blocked = snapshot_once(&mut rx, |s| s.seq > plain.seq + 4).await;
    let bin_hz = f64::from(blocked.span_hz) / blocked.db.len() as f64;
    assert!(
        (peak_hz(&blocked) - marker).abs() <= MARKER_SPREAD_HZ + bin_hz,
        "the dc blocker moved a carrier that was never at dc"
    );

    engine.remove_device_set(ds).unwrap();
}
