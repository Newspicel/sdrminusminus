use std::{collections::HashSet, time::Duration};

use sdrmm_device_siggen::{GAP_SETTING, LEVEL_SETTING, SigGenDriver};

use super::*;

fn generator_at(level_db: f64) -> (Arc<Engine>, u32) {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SigGenDriver::new()));
    let engine = Engine::with_registry(registry, None);
    engine.adopt_device("siggen:clip").expect("a generator");
    let ds = engine.create_device_set("siggen:clip").expect("device set");
    engine
        .patch_device(
            ds,
            DeviceSettings {
                extra: vec![
                    sdrmm_wire::ExtraValue {
                        name: LEVEL_SETTING.to_owned(),
                        value: serde_json::json!(level_db),
                    },
                    sdrmm_wire::ExtraValue {
                        name: GAP_SETTING.to_owned(),
                        value: serde_json::json!(0.0),
                    },
                ],
                ..DeviceSettings::default()
            },
        )
        .expect("level");
    (engine, ds)
}

fn clipping_within(engine: &Engine, wait: Duration) -> Vec<u32> {
    let mut known = None;
    let mut missing_once = HashSet::new();
    let deadline = std::time::Instant::now() + wait;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        engine.hotplug_tick_for_test(&mut known, &mut missing_once);
        let clipping = engine.snapshot().device_sets[0].clipping.clone();
        if !clipping.is_empty() {
            return clipping;
        }
    }
    Vec::new()
}

#[tokio::test]
async fn a_signal_at_full_scale_shows_its_lane_clipping() {
    let (engine, ds) = generator_at(0.0);
    assert_eq!(clipping_within(&engine, Duration::from_secs(10)), [0]);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_signal_well_below_full_scale_does_not() {
    let (engine, ds) = generator_at(-12.0);
    assert!(clipping_within(&engine, Duration::from_secs(2)).is_empty());
    engine.remove_device_set(ds).unwrap();
}
