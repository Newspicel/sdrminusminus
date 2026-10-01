use std::{sync::mpsc, time::Duration};

use sdrmm_device::{DeviceDriver, RxSink};
use sdrmm_wire::{DeviceSettings, ExtraValue};

use super::*;

const BURSTS: usize = 5;

fn only_port() -> DeviceInfo {
    if let Ok(port) = std::env::var("SDRMM_ESPSDR_PORT") {
        return device_info(&port);
    }
    let found = EspSdrDriver::new().probe();
    assert_eq!(found.len(), 1, "attach exactly one ESP-SDR: {found:?}");
    found.into_iter().next().expect("one ESP-SDR")
}

fn bursts(settings: DeviceSettings) -> Vec<Vec<sdrmm_device::Sample>> {
    let mut device = EspSdrDriver::new().open(&only_port()).expect("opens");
    device.apply(&settings).expect("valid");
    let (tx, rx) = mpsc::channel();
    device
        .rx_start(vec![RxSink::new(move |samples, _| {
            let _ = tx.send(samples.to_vec());
        })])
        .expect("starts");
    let got = (0..BURSTS)
        .map(|_| rx.recv_timeout(Duration::from_secs(5)).expect("burst"))
        .collect();
    device.rx_stop();
    got
}

fn rms(samples: &[sdrmm_device::Sample]) -> f32 {
    (samples.iter().map(|s| s.norm_sqr()).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
#[ignore = "requires one ESP-SDR"]
fn bursts_carry_live_noise_at_every_rate_and_depth() {
    for (rate, bits) in [(80e6, "8"), (40e6, "10"), (16e6, "8")] {
        let got = bursts(DeviceSettings {
            sample_rate: Some(rate),
            center_hz: Some(2437e6),
            extra: vec![ExtraValue {
                name: caps::BITS.into(),
                value: bits.into(),
            }],
            ..DeviceSettings::default()
        });
        for burst in &got {
            assert_eq!(burst.len(), 4096);
            let level = rms(burst);
            assert!(level > 0.001 && level < 1.5, "{rate} {bits}: rms {level}");
        }
    }
}
