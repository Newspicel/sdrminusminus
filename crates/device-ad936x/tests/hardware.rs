#![allow(clippy::expect_used)]

use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, RxSink, Sample, SdrDevice, lock};
use sdrmm_device_ad936x::Ad936xDriver;
use sdrmm_wire::{AgcSetting, DeviceSettings, GainKind, GainValue, StreamSettings};

const RADIO: &str = "SDRMM_AD936X";

struct Radio {
    device: Box<dyn SdrDevice>,
    held: DeviceSettings,
}

impl Deref for Radio {
    type Target = Box<dyn SdrDevice>;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

impl DerefMut for Radio {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.device
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        let held = &self.held;
        let back = DeviceSettings {
            center_hz: held.center_hz,
            sample_rate: held.sample_rate,
            bandwidth: held.bandwidth,
            rx_streams: held.rx_streams,
            antenna: held.antenna.clone(),
            gains: held.gains.clone(),
            agc: held.agc.clone(),
            streams: held
                .streams
                .iter()
                .map(|lane| StreamSettings {
                    stream: lane.stream,
                    gains: lane.gains.clone(),
                    agc: lane.agc.clone(),
                    ..StreamSettings::default()
                })
                .collect(),
            ..DeviceSettings::default()
        };
        self.device.rx_stop();
        if let Err(e) = self.device.apply(&back) {
            eprintln!("the radio did not take back what it held: {e}");
        }
    }
}

fn open() -> Radio {
    let address = std::env::var(RADIO).expect("set SDRMM_AD936X to the radio's address");
    let driver = Ad936xDriver::searching([]);
    let info = driver.resolve(&address).expect("an addressable radio");
    let device = driver.open(&info).expect("opens");
    Radio {
        held: device.settings().clone(),
        device,
    }
}

#[test]
#[ignore = "needs an AD936x radio at SDRMM_AD936X"]
fn a_real_radio_holds_every_setting_it_is_given() {
    let mut device = open();
    let lanes = device.capabilities().rx_streams;

    device
        .apply(&DeviceSettings {
            center_hz: Some(100e6),
            gains: vec![GainValue::new(GainKind::Tuner, 73.0)],
            agc: Some(AgcSetting::off()),
            ..DeviceSettings::default()
        })
        .expect("the low band");
    assert_eq!(device.capabilities().gains[0].range.max, 73.0);
    assert_eq!(device.settings().gains[0].value_db, 73.0);

    device
        .apply(&DeviceSettings {
            center_hz: Some(5.8e9),
            gains: vec![GainValue::new(GainKind::Tuner, 73.0)],
            ..DeviceSettings::default()
        })
        .expect("a gain past the high band's reach is clamped");
    assert_eq!(device.capabilities().gains[0].range.max, 62.0);

    let antennas = device.capabilities().antennas.clone();
    for antenna in &antennas {
        device
            .apply(&DeviceSettings {
                antenna: Some(antenna.clone()),
                ..DeviceSettings::default()
            })
            .expect("every offered input is one the radio takes");
        assert_eq!(device.settings().antenna.as_ref(), Some(antenna));
    }

    if lanes > 1 {
        device
            .apply(&DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    agc: Some(AgcSetting::in_mode(true, "slow_attack")),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            })
            .expect("a lane's own gain loop");
        let gains = device.agc_gains().expect("read back");
        assert_eq!(gains.iter().map(|g| g.stream).collect::<Vec<_>>(), [1]);
    }
}

#[test]
#[ignore = "needs an AD936x radio at SDRMM_AD936X"]
fn a_real_radio_streams_every_lane_at_a_decimated_rate() {
    let mut device = open();
    let rate = 1e6;
    device
        .apply(&DeviceSettings {
            sample_rate: Some(rate),
            ..DeviceSettings::default()
        })
        .expect("a rate below the transceiver");
    let taken = device.settings().sample_rate.expect("a rate");
    assert!((taken - rate).abs() < 2.0, "{taken}");

    let lanes = device.capabilities().rx_streams as usize;
    let counts: Vec<Arc<Mutex<(usize, f32, u64)>>> = (0..lanes).map(|_| Arc::default()).collect();
    let sinks = counts
        .iter()
        .map(|count| {
            let count = count.clone();
            RxSink::new(move |samples: &[Sample], index| {
                let mut count = lock(&count);
                count.0 += samples.len();
                count.2 = index + samples.len() as u64;
                count.1 = samples.iter().fold(count.1, |peak, s| peak.max(s.norm()));
            })
        })
        .collect();
    let started = Instant::now();
    device.rx_start(sinks).expect("streams");
    std::thread::sleep(Duration::from_secs(2));
    device.rx_stop();
    let span = started.elapsed().as_secs_f64();

    for (lane, count) in counts.iter().enumerate() {
        let (samples, peak, reached) = *lock(count);
        assert_eq!(
            reached, samples as u64,
            "lane {lane} reported a loss it did not have"
        );
        let achieved = samples as f64 / span;
        assert!(
            achieved > rate * 0.8 && achieved < rate * 1.1,
            "lane {lane}: {achieved:.0} S/s"
        );
        assert!(peak > 0.0, "lane {lane} carried only zeros");
    }
}

#[test]
#[ignore = "needs an AD936x radio at SDRMM_AD936X"]
fn a_real_radio_faster_than_its_link_reports_the_samples_it_lost() {
    let mut device = open();
    let fastest = device.capabilities().sample_rate_ranges[0].max;
    device
        .apply(&DeviceSettings {
            sample_rate: Some(fastest),
            ..DeviceSettings::default()
        })
        .expect("the fastest rate");
    let lanes = device.capabilities().rx_streams as usize;
    let seen: Vec<Arc<Mutex<(u64, u64)>>> = (0..lanes).map(|_| Arc::default()).collect();
    let sinks = seen
        .iter()
        .map(|seen| {
            let seen = seen.clone();
            RxSink::new(move |samples: &[Sample], index| {
                let mut seen = lock(&seen);
                seen.0 += samples.len() as u64;
                seen.1 = index + samples.len() as u64;
            })
        })
        .collect();
    device.rx_start(sinks).expect("streams");
    std::thread::sleep(Duration::from_secs(3));
    device.rx_stop();

    for (lane, seen) in seen.iter().enumerate() {
        let (delivered, reached) = *lock(seen);
        assert!(
            reached > delivered + delivered / 2,
            "lane {lane}: {delivered} delivered, timeline at {reached}"
        );
    }
}

#[test]
#[ignore = "needs an AD936x radio at SDRMM_AD936X"]
fn a_real_radio_gives_one_lane_the_whole_link() {
    let mut device = open();
    if device.capabilities().rx_stream_choices.is_empty() {
        return;
    }
    let rate = 15e6;
    device
        .apply(&DeviceSettings {
            rx_streams: Some(1),
            sample_rate: Some(rate),
            ..DeviceSettings::default()
        })
        .expect("one lane at the link's full rate");
    let seen: Arc<Mutex<(u64, u64)>> = Arc::default();
    let counted = seen.clone();
    device
        .rx_start(vec![RxSink::new(move |samples: &[Sample], index| {
            let mut seen = lock(&counted);
            seen.0 += samples.len() as u64;
            seen.1 = index + samples.len() as u64;
        })])
        .expect("streams");
    std::thread::sleep(Duration::from_secs(3));
    device.rx_stop();
    let (delivered, reached) = *lock(&seen);
    assert_eq!(delivered, reached, "one lane at {rate} S/s lost samples");
    assert!(delivered as f64 > rate * 2.5, "{delivered}");
}
