use std::sync::{Arc, Mutex, atomic::Ordering};

use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, lock, net::testing::eventually,
};
use sdrmm_wire::{AgcSetting, BandwidthSetting, DeviceSettings, GainKind, GainValue};

use crate::{AntsdrDriver, control::Target, regs::radio, rx::PACKET_SAMPLES, sim::Sim};

#[derive(Default)]
struct Seen {
    samples: Vec<Sample>,
    jumps: Vec<u64>,
    next: u64,
}

fn sink() -> (RxSink, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = seen.clone();
    (
        RxSink::new(move |samples: &[Sample], index| {
            let mut seen = lock(&log);
            if index > seen.next {
                let jump = index - seen.next;
                seen.jumps.push(jump);
            }
            seen.next = index + samples.len() as u64;
            seen.samples.extend_from_slice(samples);
        }),
        seen,
    )
}

fn open(sim: &Sim) -> Box<dyn SdrDevice> {
    let driver = AntsdrDriver::searching([]);
    let info = driver.resolve(&sim.key()).expect("addressable");
    driver.open(&info).expect("opened")
}

fn received(seen: &Arc<Mutex<Seen>>, at_least: usize) {
    eventually("samples", || {
        (lock(seen).samples.len() >= at_least).then_some(())
    });
}

#[test]
fn a_board_opens_calibrated_and_describes_itself() {
    let sim = Sim::start();
    let device = open(&sim);
    let caps = device.capabilities();
    assert_eq!(caps.rx_stream_choices, vec![1, 2]);
    assert_eq!(caps.tx_streams, 2);
    assert!(caps.bandwidth_auto);
    assert_eq!(device.settings().center_hz, Some(100e6));
    assert!(
        sim.chip(|chip| chip.writes_to(0x130)) >= 91,
        "gain table loaded"
    );
    assert_eq!(sim.chip(|chip| chip.state), 0x0a);
    assert_eq!(sim.register(Target::Radio(0), radio::ATR_DISABLE), 0);
    assert_eq!(sim.register(Target::Radio(1), radio::DDC_MUX), 0x0c);
}

#[test]
fn a_stream_delivers_the_tone_at_its_level() {
    let sim = Sim::start();
    let mut device = open(&sim);
    let (sink, seen) = sink();
    device.rx_start(vec![sink]).expect("started");
    received(&seen, 20 * PACKET_SAMPLES);
    assert_eq!(sim.streaming(), [true, false]);
    device.rx_stop();
    assert_eq!(sim.streaming(), [false, false]);
    let seen = lock(&seen);
    let level = seen.samples[..1000].iter().map(|s| s.norm()).sum::<f32>() / 1000.0;
    assert!((level - 0.5).abs() < 0.05, "{level}");
    assert!(seen.jumps.is_empty(), "{:?}", seen.jumps);
}

#[test]
fn a_lost_packet_moves_the_index_by_exactly_its_samples() {
    let sim = Sim::start();
    let mut device = open(&sim);
    let (sink, seen) = sink();
    device.rx_start(vec![sink]).expect("started");
    received(&seen, 5 * PACKET_SAMPLES);
    sim.drop_next.store(1, Ordering::Release);
    eventually("the gap", || (!lock(&seen).jumps.is_empty()).then_some(()));
    device.rx_stop();
    assert_eq!(lock(&seen).jumps, vec![PACKET_SAMPLES as u64]);
}

#[test]
fn an_overflow_restarts_the_stream_and_reports_the_gap() {
    let sim = Sim::start();
    let mut device = open(&sim);
    let (sink, seen) = sink();
    device.rx_start(vec![sink]).expect("started");
    received(&seen, 5 * PACKET_SAMPLES);
    sim.overflow.store(true, Ordering::Release);
    eventually("the gap", || (!lock(&seen).jumps.is_empty()).then_some(()));
    let before = lock(&seen).samples.len();
    received(&seen, before + 10 * PACKET_SAMPLES);
    device.rx_stop();
}

#[test]
fn two_lanes_stream_together_with_their_own_phase() {
    let sim = Sim::start();
    let mut device = open(&sim);
    device
        .apply(&DeviceSettings {
            rx_streams: Some(2),
            ..DeviceSettings::default()
        })
        .expect("two lanes");
    assert_eq!(device.capabilities().rx_streams, 2);
    let (first, left) = sink();
    let (second, right) = sink();
    device.rx_start(vec![first, second]).expect("started");
    received(&left, 10 * PACKET_SAMPLES);
    received(&right, 10 * PACKET_SAMPLES);
    device.rx_stop();
    let (left, right) = (lock(&left), lock(&right));
    let phase = (right.samples[0] * left.samples[0].conj()).arg();
    assert!((phase - 1.0).abs() < 0.05, "{phase}");
    assert_eq!(
        sim.chip(|chip| chip.last(0x003).map(|v| v & 0xc0)),
        Some(0x40)
    );
}

#[test]
fn settings_reach_the_transceiver_and_the_fpga() {
    let sim = Sim::start();
    let mut device = open(&sim);
    device
        .apply(&DeviceSettings {
            center_hz: Some(433.92e6),
            sample_rate: Some(1e6),
            bandwidth: Some(BandwidthSetting::Manual { hz: 800e3 }),
            gains: vec![
                GainValue::new(GainKind::Tuner, 20.4),
                GainValue::new(GainKind::Tx, -10.0),
            ],
            ..DeviceSettings::default()
        })
        .expect("applied");
    let settings = device.settings();
    assert_eq!(settings.center_hz, Some(433.92e6));
    assert_eq!(settings.sample_rate, Some(1e6));
    assert_eq!(
        settings.gains,
        vec![
            GainValue::new(GainKind::Tuner, 20.0),
            GainValue::new(GainKind::Tx, -10.0)
        ]
    );
    assert_eq!(sim.chip(|chip| chip.last(0x109)), Some(20));
    assert_eq!(sim.chip(|chip| chip.last(0x073)), Some(40));
    assert_eq!(
        sim.register(Target::Radio(0), radio::DDC_DECIMATION),
        1 << 9 | 1 << 8 | 8
    );
    device
        .apply(&DeviceSettings {
            agc: Some(AgcSetting::in_mode(true, "fast_attack")),
            ..DeviceSettings::default()
        })
        .expect("agc");
    assert_eq!(sim.chip(|chip| chip.last(0x0fa)), Some(0xe5));
    assert_eq!(device.agc_gains().expect("gains").len(), 1);
}

#[test]
fn what_the_board_cannot_do_is_refused() {
    let sim = Sim::start();
    let mut device = open(&sim);
    for delta in [
        DeviceSettings {
            sample_rate: Some(30e6),
            ..DeviceSettings::default()
        },
        DeviceSettings {
            antenna: Some("RX1".to_string()),
            ..DeviceSettings::default()
        },
        DeviceSettings {
            center_hz: Some(10e6),
            ..DeviceSettings::default()
        },
        DeviceSettings {
            rx_streams: Some(3),
            ..DeviceSettings::default()
        },
    ] {
        assert!(
            matches!(device.apply(&delta), Err(DeviceError::Unsupported(_))),
            "{delta:?}"
        );
    }
}

#[test]
fn a_transmission_is_paced_by_the_radio_and_ended_cleanly() {
    let sim = Sim::start();
    let mut device = open(&sim);
    let mut stream = device.tx_start().expect("transmit");
    assert_eq!(sim.register(Target::Radio(0), radio::ATR_TX), 0xe1);
    let burst = vec![Sample::new(0.25, 0.0); 50_000];
    let written = stream
        .write(&burst, std::time::Duration::from_secs(5), false)
        .expect("written");
    assert_eq!(written, burst.len());
    eventually("the radio took the burst", || {
        (lock(&sim.transmitted)[0] >= burst.len()).then_some(())
    });
    stream.stop().expect("stopped");
    eventually("the end of burst", || {
        (sim.bursts_ended.load(Ordering::Acquire) >= 1).then_some(())
    });
    assert_eq!(sim.register(Target::Radio(0), radio::ATR_TX), 0);
}

#[test]
fn two_receive_lanes_and_one_transmitter_cannot_run_together() {
    let sim = Sim::start();
    let mut device = open(&sim);
    device
        .apply(&DeviceSettings {
            rx_streams: Some(2),
            sample_rate: Some(1e6),
            ..DeviceSettings::default()
        })
        .expect("two lanes");
    let (first, _) = sink();
    let (second, _) = sink();
    device.rx_start(vec![first, second]).expect("started");
    assert!(device.tx_start().is_err());
    let both = device.tx_start_channels(&[0, 1]);
    assert!(both.is_ok());
    drop(both);
    device.rx_stop();
}
