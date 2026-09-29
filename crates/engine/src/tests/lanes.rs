use std::sync::{Arc, Mutex};

use super::*;

#[derive(Clone, Default)]
struct Started(Arc<Mutex<Vec<usize>>>);

struct LanesDriver(Started);

impl DeviceDriver for LanesDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("lanes", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(LanesDevice {
            capabilities: Capabilities {
                rx_streams: 2,
                per_stream: StreamScope {
                    gain: true,
                    ..StreamScope::default()
                },
                rx_stream_choices: vec![1, 2],
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                center_hz: Some(TEST_CENTER_HZ),
                rx_streams: Some(2),
                ..mock_settings()
            },
            started: self.0.clone(),
        }))
    }
}

struct LanesDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    started: Started,
}

impl SdrDevice for LanesDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(lanes) = settings.rx_streams {
            self.capabilities.rx_streams = lanes;
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        lock(&self.started.0).push(sinks.len());
        for mut sink in sinks {
            sink.push(&[Complex::new(0.0, 0.0); 16]);
            sink.dropped(1_000_000);
            sink.push(&[Complex::new(0.0, 0.0); 16]);
        }
        Ok(())
    }

    fn rx_stop(&mut self) {}
}

fn lanes_engine() -> (Arc<Engine>, Started) {
    let started = Started::default();
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(LanesDriver(started.clone())));
    (Engine::with_registry(registry, None), started)
}

fn lanes(count: u32) -> DeviceSettings {
    DeviceSettings {
        rx_streams: Some(count),
        ..DeviceSettings::default()
    }
}

#[tokio::test]
async fn fewer_lanes_restart_the_capture_with_only_those_lanes() {
    let (engine, started) = lanes_engine();
    let ds = engine.create_device_set("mock:lanes").unwrap();
    engine.patch_device(ds, lanes(1)).expect("one lane");
    assert_eq!(engine.snapshot().device_sets[0].capabilities.rx_streams, 1);
    engine.patch_device(ds, lanes(2)).expect("two lanes");
    assert_eq!(*lock(&started.0), vec![2, 1, 2]);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_lane_that_is_wired_is_not_switched_off() {
    let (engine, started) = lanes_engine();
    let ds = engine.create_device_set("mock:lanes").unwrap();
    hold_tuning(&engine, ds);
    engine
        .add_channel(ds, 1, nfm_settings(0.0))
        .expect("a decoder on iq2");
    let refused = engine.patch_device(ds, lanes(1)).unwrap_err();
    assert!(refused.to_string().contains("iq2"), "{refused}");
    assert_eq!(engine.snapshot().device_sets[0].capabilities.rx_streams, 2);
    assert_eq!(*lock(&started.0), vec![2]);
    assert!(
        engine.patch_device(ds, lanes(3)).is_err(),
        "a count the radio does not offer"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn drops_counted_before_a_lane_restart_do_not_upset_the_next_poll() {
    let (engine, _) = lanes_engine();
    let ds = engine.create_device_set("mock:lanes").unwrap();
    let mut known = None;
    let mut missing_once = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.snapshot().device_sets[0].overruns < 2_000_000 {
        assert!(Instant::now() < deadline, "the gaps were never counted");
        engine.hotplug_tick_for_test(&mut known, &mut missing_once);
        std::thread::sleep(Duration::from_millis(20));
    }
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    engine.patch_device(ds, lanes(1)).expect("one lane");
    std::thread::sleep(Duration::from_millis(50));
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let set = &engine.snapshot().device_sets[0];
    assert!(set.overruns < 2_000_000, "{}", set.overruns);
    engine.remove_device_set(ds).unwrap();
}

#[test]
fn the_share_lost_is_what_went_missing_of_what_the_radio_sampled() {
    assert_eq!(crate::loss_share(0, 1.0, 1e6), None);
    assert_eq!(crate::loss_share(750_000, 1.0, 1e6), Some(0.75));
    assert_eq!(
        crate::loss_share(10, 1.0, 1e6),
        Some(0.01),
        "any loss shows"
    );
    assert_eq!(crate::loss_share(5_000_000, 1.0, 1e6), Some(1.0));
    assert_eq!(crate::loss_share(10, 0.0, 1e6), None);
}
