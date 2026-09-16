use super::*;

#[test]
fn the_blocker_starts_on_for_hardware_known_to_land_a_dc_term() {
    assert!(dc_block(&managed_caps(), &untouched_settings()));
    let off = DeviceSettings {
        dc_block: Some(false),
        ..untouched_settings()
    };
    assert!(
        !dc_block(&managed_caps(), &off),
        "the operator's choice was overridden"
    );
}

#[test]
fn the_blocker_starts_off_for_hardware_the_engine_does_not_know() {
    assert!(!dc_block(&tuner_caps(), &untouched_settings()));
    let on = DeviceSettings {
        dc_block: Some(true),
        ..untouched_settings()
    };
    assert!(dc_block(&tuner_caps(), &on));
}

#[test]
fn a_source_with_no_front_end_never_blocks_dc() {
    let mut recording = tuner_caps();
    recording.dc_artifact = sdrmm_wire::DcArtifact::None;
    let asked = DeviceSettings {
        dc_block: Some(true),
        ..untouched_settings()
    };
    assert!(
        !dc_block(&recording, &asked),
        "a recording has no receiver term to remove"
    );
}

#[test]
fn a_decoder_on_the_centre_is_not_clear_of_the_dc_term() {
    let on_the_centre = parked(1, 0.0);
    let beside_it = parked(2, 300_000.0);
    assert!(!centre_clears_channels(
        &untouched_settings(),
        &tuner_caps(),
        std::slice::from_ref(&on_the_centre)
    ));
    assert!(centre_clears_channels(
        &untouched_settings(),
        &tuner_caps(),
        std::slice::from_ref(&beside_it)
    ));
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
