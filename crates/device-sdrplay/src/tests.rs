use std::sync::mpsc;

use sdrmm_wire::{ExtraValue, GainValue, StreamSettings};

use super::*;
use crate::testing::FakeApi;

fn driver(api: Arc<FakeApi>) -> SdrplayDriver {
    SdrplayDriver::with_api(api)
}

fn open(api: &Arc<FakeApi>, key: &str) -> Box<dyn SdrDevice> {
    let driver = driver(api.clone());
    let info = driver
        .probe()
        .into_iter()
        .find(|info| info.key == key)
        .unwrap_or_else(|| panic!("{key} is not listed"));
    driver.open(&info).expect("open")
}

#[test]
fn a_receiver_is_listed_once_with_its_model_in_the_label() {
    let api = Arc::new(FakeApi::rsp1a());
    let listed = driver(api).probe();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].driver, DRIVER_ID);
    assert_eq!(listed[0].key, "1234567890");
    assert_eq!(listed[0].label, "RSP1A 1234567890");
    assert_eq!(listed[0].serial.as_deref(), Some("1234567890"));
}

#[test]
fn nothing_is_listed_when_the_vendor_api_is_missing() {
    if shared().is_err() {
        assert!(SdrplayDriver::new().probe().is_empty());
    }
}

#[test]
fn opening_a_key_that_is_gone_reports_not_found() {
    let api = Arc::new(FakeApi::rsp1a());
    let info = DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: "9999999999".to_string(),
        label: "gone".to_string(),
        serial: Some("9999999999".to_string()),
        profile: None,
    };
    assert!(matches!(
        driver(api).open(&info),
        Err(DeviceError::NotFound(_))
    ));
}

#[test]
fn a_second_open_of_the_same_receiver_reports_it_is_in_use() {
    let api = Arc::new(FakeApi::rsp1a());
    let _first = open(&api, "1234567890");
    let driver = driver(api.clone());
    let info = driver.probe().into_iter().next().expect("listed");
    assert!(matches!(driver.open(&info), Err(DeviceError::InUse(_))));
}

#[test]
fn opening_configures_a_coherent_starting_point() {
    let api = Arc::new(FakeApi::rsp1a());
    let device = open(&api, "1234567890");
    assert_eq!(device.settings().sample_rate, Some(2_000_000.0));
    assert_eq!(api.dev_params().fs_freq.fs_hz, 2_000_000.0);
    assert_eq!(api.channel(ffi::TUNER_A).tuner_params.if_type, ffi::IF_ZERO);
    assert_eq!(device.capabilities().rx_streams, 1);
    assert_eq!(device.capabilities().tx_streams, 0);
}

#[test]
fn a_closed_device_is_released_back_to_the_api() {
    let api = Arc::new(FakeApi::rsp1a());
    let device = open(&api, "1234567890");
    assert!(api.is_selected());
    drop(device);
    assert!(!api.is_selected());
}

#[test]
fn settings_applied_before_streaming_reach_the_parameters_without_an_update_call() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    device
        .apply(&DeviceSettings {
            center_hz: Some(99_500_000.0),
            ..DeviceSettings::default()
        })
        .expect("tune");
    assert_eq!(
        api.channel(ffi::TUNER_A).tuner_params.rf_freq.rf_hz,
        99_500_000.0
    );
    assert!(api.updates().is_empty());
    assert_eq!(device.settings().center_hz, Some(99_500_000.0));
}

#[test]
fn settings_applied_while_streaming_are_pushed_with_the_matching_reason() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    device.rx_start(vec![RxSink::new(|_| {})]).expect("start");
    device
        .apply(&DeviceSettings {
            center_hz: Some(145_500_000.0),
            ..DeviceSettings::default()
        })
        .expect("tune");
    let updates = api.updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, ffi::TUNER_A);
    assert_eq!(updates[0].1, ffi::UPDATE_TUNER_FRF);
    device.rx_stop();
}

#[test]
fn a_setting_that_changes_nothing_is_not_pushed_to_the_hardware() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    device.rx_start(vec![RxSink::new(|_| {})]).expect("start");
    let center = device.settings().center_hz;
    device
        .apply(&DeviceSettings {
            center_hz: center,
            ..DeviceSettings::default()
        })
        .expect("tune");
    assert!(api.updates().is_empty());
    device.rx_stop();
}

#[test]
fn samples_from_the_api_reach_the_sink() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    let (tx, rx) = mpsc::channel();
    device
        .rx_start(vec![RxSink::new(move |samples| {
            tx.send(samples.len()).expect("receiver lives");
        })])
        .expect("start");
    assert!(api.is_streaming());
    api.emit(ffi::TUNER_A, &[(1000, -1000); 64]);
    assert_eq!(rx.try_recv().expect("a block"), 64);
    device.rx_stop();
    assert!(!api.is_streaming());
}

#[test]
fn a_second_start_is_refused_while_one_runs() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    device.rx_start(vec![RxSink::new(|_| {})]).expect("start");
    assert!(matches!(
        device.rx_start(vec![RxSink::new(|_| {})]),
        Err(DeviceError::AlreadyStreaming)
    ));
    device.rx_stop();
}

#[test]
fn the_wrong_number_of_sinks_is_refused_before_the_hardware_is_touched() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    assert!(matches!(
        device.rx_start(vec![RxSink::new(|_| {}), RxSink::new(|_| {})]),
        Err(DeviceError::Unsupported(_))
    ));
    assert!(!api.is_streaming());
}

#[test]
fn an_unplugged_receiver_surfaces_through_the_fatal_handler() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    let (tx, rx) = mpsc::channel();
    device
        .rx_start(vec![RxSink::with_fatal_handler(
            |_| {},
            move |error| tx.send(error.to_string()).expect("receiver lives"),
        )])
        .expect("start");
    api.unplug();
    let reported = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the monitor reports the unplug");
    assert!(reported.contains("unplugged"));
    assert!(!api.is_streaming());
}

#[test]
fn a_dual_tuner_duo_streams_both_tuners_independently() {
    let api = Arc::new(FakeApi::dual_tuner_duo());
    let mut device = open(&api, "1809001DDD@DT");
    assert_eq!(device.capabilities().rx_streams, 2);
    assert!(device.capabilities().per_stream.tuning);
    assert_eq!(api.dev_params().fs_freq.fs_hz, model::DUO_DUAL_TUNER_FS_HZ);
    assert_eq!(
        api.channel(ffi::TUNER_B).tuner_params.if_type,
        ffi::IF_1_620
    );

    device
        .apply(&DeviceSettings {
            streams: vec![
                StreamSettings {
                    stream: 0,
                    center_hz: Some(7_100_000.0),
                    ..StreamSettings::default()
                },
                StreamSettings {
                    stream: 1,
                    center_hz: Some(14_200_000.0),
                    ..StreamSettings::default()
                },
            ],
            ..DeviceSettings::default()
        })
        .expect("tune both tuners");
    assert_eq!(
        api.channel(ffi::TUNER_A).tuner_params.rf_freq.rf_hz,
        7_100_000.0
    );
    assert_eq!(
        api.channel(ffi::TUNER_B).tuner_params.rf_freq.rf_hz,
        14_200_000.0
    );

    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    device
        .rx_start(vec![
            RxSink::new(move |samples| tx_a.send(samples.len()).expect("receiver lives")),
            RxSink::new(move |samples| tx_b.send(samples.len()).expect("receiver lives")),
        ])
        .expect("start");
    api.emit(ffi::TUNER_A, &[(1, 1); 16]);
    api.emit(ffi::TUNER_B, &[(1, 1); 32]);
    assert_eq!(rx_a.try_recv().expect("tuner 1 block"), 16);
    assert_eq!(rx_b.try_recv().expect("tuner 2 block"), 32);
    device.rx_stop();
}

#[test]
fn a_stream_index_the_device_does_not_have_is_refused() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    assert!(matches!(
        device.apply(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(100e6),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }),
        Err(DeviceError::Unsupported(_))
    ));
}

#[test]
fn a_duo_slave_waits_for_its_master_before_streaming() {
    let api = Arc::new(FakeApi::with_devices(vec![FakeApi::duo(
        "1809001DDD",
        ffi::DUO_MODE_SLAVE,
        ffi::TUNER_B,
    )]));
    api.require_master_before_start(1);
    let mut device = open(&api, "1809001DDD@SLV");
    let master = api.clone();
    let started = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        master.master_started();
    });
    device.rx_start(vec![RxSink::new(|_| {})]).expect("start");
    started.join().expect("the master thread finishes");
    assert!(api.is_streaming());
    device.rx_stop();
}

#[test]
fn a_duo_slave_uses_the_tuner_the_master_left_free() {
    let api = Arc::new(FakeApi::with_devices(vec![FakeApi::duo(
        "1809001DDD",
        ffi::DUO_MODE_SLAVE,
        ffi::TUNER_B,
    )]));
    let mut device = open(&api, "1809001DDD@SLV");
    device.rx_start(vec![RxSink::new(|_| {})]).expect("start");
    device
        .apply(&DeviceSettings {
            center_hz: Some(50_000_000.0),
            ..DeviceSettings::default()
        })
        .expect("tune");
    assert_eq!(api.updates()[0].0, ffi::TUNER_B);
    assert_eq!(
        api.channel(ffi::TUNER_B).tuner_params.rf_freq.rf_hz,
        50_000_000.0
    );
    device.rx_stop();
}

#[test]
fn the_rf_gain_range_follows_the_band_the_receiver_is_tuned_to() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    let rf_max = |device: &dyn SdrDevice| {
        device
            .capabilities()
            .gains
            .iter()
            .find(|stage| stage.name == caps::RF_GAIN_STAGE)
            .expect("rf stage")
            .range
            .max
    };
    let tune = |device: &mut Box<dyn SdrDevice>, center_hz: f64| {
        device
            .apply(&DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            })
            .expect("tune");
    };
    tune(&mut device, 100_000_000.0);
    assert_eq!(rf_max(device.as_ref()), 62.0);
    tune(&mut device, 5_000_000.0);
    assert_eq!(
        rf_max(device.as_ref()),
        61.0,
        "the AM band below 60 MHz has its own gain table"
    );
    tune(&mut device, 500_000_000.0);
    assert_eq!(rf_max(device.as_ref()), 64.0);
    assert!(
        device
            .settings()
            .gains
            .iter()
            .any(|gain| gain.stage == caps::IF_GAIN_STAGE)
    );
}

#[test]
fn an_extra_this_receiver_does_not_have_is_refused_and_changes_nothing() {
    let api = Arc::new(FakeApi::with_devices(vec![FakeApi::device(
        ffi::RSP1_ID,
        "1000000001",
    )]));
    let mut device = open(&api, "1000000001");
    assert!(matches!(
        device.apply(&DeviceSettings {
            extra: vec![ExtraValue {
                name: caps::EXTRA_BIAS_T.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        }),
        Err(DeviceError::Unsupported(_))
    ));
    assert!(api.updates().is_empty());
}

#[test]
fn a_gain_setting_survives_the_round_trip_through_the_hardware_units() {
    let api = Arc::new(FakeApi::rsp1a());
    let mut device = open(&api, "1234567890");
    device
        .apply(&DeviceSettings {
            gains: vec![GainValue {
                stage: caps::IF_GAIN_STAGE.to_string(),
                value_db: 25.0,
            }],
            ..DeviceSettings::default()
        })
        .expect("gain");
    assert_eq!(api.channel(ffi::TUNER_A).tuner_params.gain.gr_db, 34);
    let reported = device
        .settings()
        .gains
        .iter()
        .find(|gain| gain.stage == caps::IF_GAIN_STAGE)
        .expect("if gain")
        .value_db;
    assert_eq!(reported, 25.0);
}
