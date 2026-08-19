use super::*;

#[tokio::test]
async fn probes_virtual_device() {
    let engine = virtual_engine();
    assert!(
        engine
            .probe_devices()
            .iter()
            .any(|d| d.id() == "virtual:siggen")
    );
}

#[tokio::test]
async fn one_radio_opens_into_one_device_set() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let refused = engine.create_device_set("virtual:siggen").unwrap_err();
    assert!(
        matches!(&refused, EngineError::DeviceAlreadyOpen(device, held)
            if device == "virtual:siggen" && *held == ds),
        "expected a reopen refusal, got {refused}"
    );
    assert!(refused.is_conflict());
    assert!(!refused.is_bad_request());
    assert_eq!(engine.snapshot().device_sets.len(), 1);

    engine.remove_device_set(ds).unwrap();
    engine.create_device_set("virtual:siggen").unwrap();
}

#[tokio::test]
async fn spectrum_flows_with_a_visible_tone() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let snap = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("spectrum within timeout")
        .expect("snapshot");
    assert_eq!(snap.db.len(), 4096);

    let mut sorted = snap.db.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let peak = *sorted.last().unwrap();
    assert!(
        peak - median > 20.0,
        "expected tone peak above floor (peak {peak}, median {median})"
    );

    engine.remove_device_set(ds).unwrap();
    assert!(engine.snapshot().device_sets.is_empty());
}

#[test]
fn the_builtin_registry_carries_every_backend_this_build_compiled_in() {
    let ids: Vec<&str> = builtin_registry(None)
        .driver_ids()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert!(ids.contains(&"virtual"), "{ids:?}");
    #[cfg(feature = "rtlsdr")]
    assert!(ids.contains(&"rtlsdr"), "{ids:?}");
    #[cfg(feature = "hackrf")]
    assert!(ids.contains(&"hackrf"), "{ids:?}");
    #[cfg(feature = "soapy")]
    assert!(ids.contains(&"soapy"), "{ids:?}");
}

#[test]
fn soapy_hides_exactly_the_radios_this_build_drives_over_usb() {
    let handled = soapy_handled_natively();
    assert_eq!(handled.contains(&"rtlsdr"), cfg!(feature = "rtlsdr"));
    assert_eq!(handled.contains(&"hackrf"), cfg!(feature = "hackrf"));
    assert!(
        !handled.contains(&"sdrplay"),
        "the SDRplay driver reports unique serials and settles its duplicate by priority instead"
    );
}
