use super::*;

#[tokio::test]
async fn device_fault_surfaces_and_removal_completes() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(DyingDriver));
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:dying").unwrap();

    wait_for_deviceset_event(&mut events, ds).await;

    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0]
            .error
            .as_deref()
            .unwrap()
            .contains("mock stream died"),
        "fault message must surface: {:?}",
        snap.device_sets[0].error
    );
    assert_eq!(snap.device_sets[0].device.label, "Mock dying");
    assert_eq!(snap.device_sets[0].device.serial.as_deref(), Some("MOCK-1"));
    assert!(snap.device_sets[0].device.profile.is_none());

    let removal = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || engine.remove_device_set(ds))
    };
    tokio::time::timeout(Duration::from_secs(5), removal)
        .await
        .expect("removal must not hang on a dead capture thread")
        .expect("join")
        .expect("remove ok");
}

#[tokio::test]
async fn fault_raised_before_insert_still_surfaces() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(InstantFailDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:instafail").unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snap = engine.snapshot();
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("faulted set must stay listed");
        if set.status == DeviceSetStatus::Error {
            assert!(
                set.error
                    .as_deref()
                    .expect("error message")
                    .contains("died at start"),
                "fault message must surface: {:?}",
                set.error
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "set stuck in {:?} without surfacing the fault",
            set.status
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn probe_disappearance_faults_running_set_after_two_misses() {
    let present = Arc::new(AtomicBool::new(true));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(VanishingDriver {
            present: present.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:vanish").unwrap();

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert_eq!(
        engine.snapshot().device_sets[0].status,
        DeviceSetStatus::Running,
        "present device must not be faulted"
    );

    present.store(false, Ordering::SeqCst);
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert_eq!(
        engine.snapshot().device_sets[0].status,
        DeviceSetStatus::Running,
        "one missed probe may be a transient enumerate hiccup"
    );

    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0]
            .error
            .as_deref()
            .unwrap()
            .contains("disappeared from probe"),
        "unplug reason must surface: {:?}",
        snap.device_sets[0].error
    );
    wait_for_deviceset_event(&mut events, ds).await;
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_radio_another_program_holds_is_refused_by_name() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(BusyDriver));
    let engine = Engine::with_registry(registry, None);

    let refused = engine.create_device_set("mock:busy").unwrap_err();
    assert!(refused.is_conflict(), "{refused}");
    assert!(
        refused.to_string().contains("already in use"),
        "the reason must say the radio is taken, not just fail: {refused}"
    );
    assert!(
        engine.snapshot().device_sets.is_empty(),
        "a refused open must leave no half-built set behind"
    );
}

#[tokio::test]
async fn a_quiet_bus_is_enumerated_once_not_on_every_tick() {
    let probes = Arc::new(AtomicUsize::new(0));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(CountingDriver {
            probes: probes.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);

    let mut known = None;
    let mut missing_once = HashSet::new();
    let mut gate = crate::hotplug::ProbeGate::default();
    for _ in 0..5 {
        engine.hotplug_tick(&mut known, &mut missing_once, &mut gate, false);
    }

    assert_eq!(
        probes.load(Ordering::SeqCst),
        1,
        "vendor drivers must not be woken while the USB bus is unchanged"
    );

    engine.hotplug_tick(&mut known, &mut missing_once, &mut gate, true);
    assert_eq!(
        probes.load(Ordering::SeqCst),
        2,
        "a plugged or unplugged radio must be enumerated at once"
    );
}

#[tokio::test]
async fn hotplug_tick_emits_only_on_probe_change() {
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(FlappingDriver {
            probes: AtomicUsize::new(0),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();

    let mut known = None;
    let mut missing_once = HashSet::new();
    assert!(
        !engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "first probe is baseline"
    );
    assert!(
        engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "attach must be detected"
    );

    let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("event within timeout")
        .expect("event");
    assert!(matches!(
        ev,
        ServerEvent::StateChanged {
            scope: StateScope::Devices
        }
    ));

    assert!(
        !engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "steady state stays quiet"
    );
}
