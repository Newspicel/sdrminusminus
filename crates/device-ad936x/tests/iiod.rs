#![allow(clippy::expect_used)]
mod common;

use std::sync::{Arc, Mutex};

use common::{FakeIiod, RAMP};
use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, lock, net::testing::eventually,
};
use sdrmm_device_ad936x::Ad936xDriver;
use sdrmm_wire::{Coherence, DcArtifact, DeviceSettings, Duplex, ExtraValue, GainValue};

type Lane = Arc<Mutex<Vec<Sample>>>;

fn open(server: &FakeIiod) -> Box<dyn SdrDevice> {
    let driver = Ad936xDriver::new();
    let info = driver
        .resolve(&server.endpoint())
        .expect("an addressable radio");
    driver.open(&info).expect("opens")
}

fn lane_sink(lane: &Lane) -> RxSink {
    let collected = lane.clone();
    RxSink::new(move |samples: &[Sample], _| lock(&collected).extend_from_slice(samples))
}

fn collected(lane: &Lane, at_least: usize) -> Vec<Sample> {
    eventually("samples from the radio", || {
        let got = lock(lane).clone();
        (got.len() >= at_least).then_some(got)
    })
}

fn wrote(server: &FakeIiod, prefix: &str) -> Vec<String> {
    server
        .commands()
        .into_iter()
        .filter(|command| command.starts_with(prefix))
        .collect()
}

#[test]
fn opening_reads_the_radios_own_limits_rather_than_assuming_them() {
    let server = FakeIiod::spawn(1);
    let device = open(&server);
    let caps = device.capabilities();

    assert_eq!(caps.freq_ranges.len(), 1);
    assert_eq!(caps.freq_ranges[0].min, 70e6);
    assert_eq!(caps.freq_ranges[0].max, 6e9);
    assert_eq!(caps.sample_rate_ranges[0].min, 2_083_333.0);
    assert_eq!(caps.bandwidth_ranges[0].max, 56e6);
    assert_eq!(
        caps.gains
            .iter()
            .map(|stage| (stage.name.as_str(), stage.range.min, stage.range.max))
            .collect::<Vec<_>>(),
        vec![("RX", -3.0, 71.0), ("TX", -89.75, 0.0)]
    );
    assert_eq!(
        caps.antennas,
        vec!["A_BALANCED", "B_BALANCED", "TX_MONITOR1"]
    );
    assert_eq!(caps.duplex, Duplex::Full);
    assert_eq!(caps.rx_streams, 1);
    assert_eq!(caps.tx_streams, 1);
    assert_eq!(caps.coherence, Coherence::None);
    assert_eq!(caps.dc_artifact, DcArtifact::Managed);
    assert!(caps.ppm, "the crystal on this board can be trimmed");

    let names: Vec<&str> = caps.extra.iter().map(|setting| setting.name()).collect();
    assert_eq!(
        names,
        vec![
            "gain_mode",
            "quadrature_tracking",
            "rf_dc_tracking",
            "bb_dc_tracking",
            "fir_filter",
            "tx_port"
        ]
    );
}

#[test]
fn the_settings_that_come_back_are_the_ones_the_radio_is_holding() {
    let server = FakeIiod::spawn(1);
    let device = open(&server);
    let settings = device.settings();
    assert_eq!(settings.center_hz, Some(2_400_000_000.0));
    assert_eq!(settings.sample_rate, Some(2_400_000.0));
    assert_eq!(settings.bandwidth, Some(18_000_000.0));
    assert_eq!(settings.antenna.as_deref(), Some("A_BALANCED"));
    assert_eq!(settings.ppm, Some(0.0));
    assert_eq!(
        settings.gains,
        vec![
            GainValue {
                stage: "RX".to_string(),
                value_db: 40.0
            },
            GainValue {
                stage: "TX".to_string(),
                value_db: -10.0
            },
        ]
    );
    assert!(
        settings
            .extra
            .iter()
            .any(|extra| extra.name == "gain_mode" && extra.value == "manual"),
        "{:?}",
        settings.extra
    );
}

#[test]
fn a_two_by_two_radio_is_recognised_as_one() {
    let server = FakeIiod::spawn(2);
    let device = open(&server);
    let caps = device.capabilities();
    assert_eq!(caps.rx_streams, 2);
    assert_eq!(caps.tx_streams, 2);
    assert_eq!(caps.coherence, Coherence::PhaseCoherent);
    assert!(caps.per_stream.gain);
    assert!(!caps.per_stream.tuning);
}

#[test]
fn applying_settings_reaches_the_radio_as_the_attributes_it_understands() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    device
        .apply(&DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(4_000_000.0),
            bandwidth: Some(3_000_000.0),
            antenna: Some("B_BALANCED".to_string()),
            gains: vec![GainValue {
                stage: "RX".to_string(),
                value_db: 30.0,
            }],
            extra: vec![ExtraValue {
                name: "gain_mode".to_string(),
                value: serde_json::json!("slow_attack"),
            }],
            ..DeviceSettings::default()
        })
        .expect("the radio took it");

    assert_eq!(
        server.attribute("ad9361-phy/OUTPUT/altvoltage0/frequency"),
        Some("433920000".to_string())
    );
    assert_eq!(
        server.attribute("ad9361-phy/OUTPUT/altvoltage1/frequency"),
        Some("433920000".to_string()),
        "the transmit synthesizer follows the dial"
    );
    assert_eq!(
        server.attribute("ad9361-phy/INPUT/voltage0/sampling_frequency"),
        Some("4000000".to_string())
    );
    assert_eq!(
        server.attribute("ad9361-phy/INPUT/voltage0/rf_bandwidth"),
        Some("3000000".to_string())
    );
    assert_eq!(
        server.attribute("ad9361-phy/INPUT/voltage0/hardwaregain"),
        Some("30.000000".to_string())
    );
    assert_eq!(
        server.attribute("ad9361-phy/INPUT/voltage0/rf_port_select"),
        Some("B_BALANCED".to_string())
    );
    assert_eq!(
        server.attribute("ad9361-phy/INPUT/voltage0/gain_control_mode"),
        Some("slow_attack".to_string())
    );

    let settings = device.settings();
    assert_eq!(settings.center_hz, Some(433_920_000.0));
    assert_eq!(settings.sample_rate, Some(4_000_000.0));
}

#[test]
fn a_setting_the_radio_refuses_surfaces_instead_of_being_believed() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    server.refuse("WRITE", -22);
    let error = device
        .apply(&DeviceSettings {
            center_hz: Some(433_920_000.0),
            ..DeviceSettings::default()
        })
        .expect_err("the radio refused it");
    assert!(matches!(error, DeviceError::Unsupported(_)), "{error}");
    assert!(error.to_string().contains("frequency"), "{error}");
    assert_eq!(
        device.settings().center_hz,
        Some(2_400_000_000.0),
        "a refused setting must not be reported as taken"
    );
}

#[test]
fn a_setting_outside_the_radios_limits_never_reaches_it() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let before = server.commands().len();
    let error = device
        .apply(&DeviceSettings {
            center_hz: Some(10e9),
            ..DeviceSettings::default()
        })
        .expect_err("refused before it is sent");
    assert!(error.to_string().contains("tuning range"), "{error}");
    assert_eq!(server.commands().len(), before);
}

#[test]
fn receiving_delivers_the_samples_the_buffer_carried() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let lane: Lane = Arc::default();
    device.rx_start(vec![lane_sink(&lane)]).expect("starts");

    let samples = collected(&lane, RAMP.len() / 2);
    device.rx_stop();

    assert!((samples[0].re - 0.0).abs() < 1e-6);
    assert!((samples[0].im - 1.0 / 2048.0).abs() < 1e-6);
    assert!((samples[1].re + 1.0 / 2048.0).abs() < 1e-6);
    assert!((samples[1].im - 2047.0 / 2048.0).abs() < 1e-6);
    assert!((samples[2].re + 1.0).abs() < 1e-6);

    let opened = server.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(
        opened[0].starts_with("OPEN cf-ad9361-lpc "),
        "the receive buffer is the converter interface: {opened:?}"
    );
    assert!(
        opened[0].ends_with("00000003"),
        "one lane enables two scan elements: {opened:?}"
    );
    assert!(
        server.connections() >= 2,
        "the buffer gets a conversation of its own"
    );
}

#[test]
fn stopping_gives_the_buffer_back_so_the_next_start_gets_one() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let first: Lane = Arc::default();
    device.rx_start(vec![lane_sink(&first)]).expect("starts");
    collected(&first, 2);
    device.rx_stop();

    let second: Lane = Arc::default();
    device.rx_start(vec![lane_sink(&second)]).expect("restarts");
    collected(&second, 2);
    device.rx_stop();
    assert_eq!(
        server.opened().len(),
        2,
        "each start opens a buffer of its own: {:?}",
        server.opened()
    );
}

#[test]
fn two_lanes_arrive_split_apart_and_sample_aligned() {
    let server = FakeIiod::spawn(2);
    let mut device = open(&server);
    let first: Lane = Arc::default();
    let second: Lane = Arc::default();
    device
        .rx_start(vec![lane_sink(&first), lane_sink(&second)])
        .expect("starts");

    let left = collected(&first, 4);
    let right = collected(&second, 4);
    device.rx_stop();

    // Four scan elements repeat the pattern, so lane 0 takes the first pair of every four counts
    // and lane 1 the second.
    assert!((left[0].re - 0.0).abs() < 1e-6);
    assert!((left[0].im - 1.0 / 2048.0).abs() < 1e-6);
    assert!((right[0].re + 1.0 / 2048.0).abs() < 1e-6);
    assert!((right[0].im - 2047.0 / 2048.0).abs() < 1e-6);
    assert!((left[1].re + 1.0).abs() < 1e-6);

    assert!(
        server.opened()[0].ends_with("0000000f"),
        "two lanes enable four scan elements: {:?}",
        server.opened()
    );
}

#[test]
fn a_radio_that_stops_answering_reports_it_rather_than_going_quiet() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let faults = Arc::new(Mutex::new(Vec::new()));
    let told = faults.clone();
    server.refuse("READBUF", -5);
    device
        .rx_start(vec![RxSink::with_fatal_handler(
            |_, _| {},
            move |error| lock(&told).push(error.to_string()),
        )])
        .expect("starts");

    let fault = eventually("a fault to reach the engine", || {
        lock(&faults).first().cloned()
    });
    device.rx_stop();
    assert!(fault.contains("restart attempts"), "{fault}");
}

#[test]
fn transmitting_hands_the_radio_the_samples_it_was_given() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let mut stream = device.tx_start().expect("a transmit stream");
    let samples = [
        Sample::new(0.5, -0.5),
        Sample::new(0.0, 1.0),
        Sample::new(-1.0, 0.25),
    ];
    let accepted = stream
        .write(&samples, std::time::Duration::from_secs(1), false)
        .expect("the radio took it");
    assert_eq!(accepted, samples.len());
    stream.stop().expect("stops");

    let sent = server.transmitted();
    assert_eq!(sent.len(), samples.len() * 4);
    let words: Vec<i16> = sent
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    assert_eq!(
        words,
        vec![16_384, -16_384, 0, i16::MAX, i16::MIN, 8_192],
        "samples reach the transmitter at the scale they were handed over at"
    );
    assert!(
        server
            .opened()
            .iter()
            .any(|open| open.starts_with("OPEN cf-ad9361-dds-core-lpc ")),
        "{:?}",
        server.opened()
    );
}

#[test]
fn a_radio_can_receive_and_transmit_at_the_same_time() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let lane: Lane = Arc::default();
    device.rx_start(vec![lane_sink(&lane)]).expect("receives");
    let mut stream = device.tx_start().expect("transmits alongside");
    collected(&lane, 2);
    stream.stop().expect("stops");
    device.rx_stop();
    assert!(!wrote(&server, "READBUF").is_empty());
    assert_eq!(wrote(&server, "WRITEBUF").len(), 0, "nothing was sent yet");
}

#[test]
fn asking_for_more_lanes_than_the_radio_has_is_refused() {
    let server = FakeIiod::spawn(1);
    let mut device = open(&server);
    let error = device
        .rx_start(vec![RxSink::new(|_, _| {}), RxSink::new(|_, _| {})])
        .expect_err("refused");
    assert!(error.to_string().contains("1 rx streams"), "{error}");
    let Err(error) = device.tx_start_channels(&[0, 1]) else {
        panic!("a second transmit lane must be refused");
    };
    assert!(error.to_string().contains("1 tx streams"), "{error}");
}

#[test]
fn a_radio_that_is_not_an_ad936x_is_refused_by_name() {
    let xml = "<context name=\"n\" ><device id=\"iio:device0\" name=\"xadc\" >\
               <channel id=\"voltage0\" type=\"input\" >\
               <attribute name=\"raw\" value=\"1\" /></channel></device></context>";
    let server = FakeIiod::with(xml.to_string(), std::collections::HashMap::new());
    let driver = Ad936xDriver::new();
    let info = driver.resolve(&server.endpoint()).expect("addressable");
    let Err(error) = driver.open(&info) else {
        panic!("a radio that is not an AD936x must be refused");
    };
    assert!(matches!(error, DeviceError::Unsupported(_)), "{error}");
    assert!(error.to_string().contains("xadc"), "{error}");
}

#[test]
fn a_search_finds_a_radio_at_an_address_it_was_told_about() {
    let server = FakeIiod::spawn(1);
    let driver = Ad936xDriver::new();
    assert!(
        !driver
            .probe()
            .iter()
            .any(|found| found.key.contains("127.0.0.1"))
    );
    let info = driver.resolve(&server.endpoint()).expect("addressable");
    assert_eq!(info.driver, "ad936x");
    assert!(driver.probe().contains(&info));
    assert!(driver.probe_deep().contains(&info));
}
