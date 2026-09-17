#![allow(clippy::expect_used)]
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, lock,
    net::testing::{DEADLINE, FakeServer, WebSocketPeer, eventually},
};
use sdrmm_device_sdrconnect::SdrConnectDriver;
use sdrmm_wire::{DeviceSettings, ExtraSetting, ExtraValue};

const CAPTURING: usize = 1;
const RECONNECTED: usize = 2;

const PRIMARY_SENT: i16 = 16_384;
const PRIMARY_EXPECTED: f32 = 0.5;
const SECONDARY_SENT: i16 = 8_192;
const SECONDARY_EXPECTED: f32 = 0.25;

type Observed = Arc<Mutex<HashMap<usize, Vec<String>>>>;

#[derive(Clone, Copy)]
struct Behaviour {
    steerable: bool,
    drop_capture: bool,
    fragment: bool,
    answer_nothing: bool,
    both_tuners: bool,
    push_readouts: bool,
    unsolicited_audio: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            steerable: true,
            drop_capture: false,
            fragment: false,
            answer_nothing: false,
            both_tuners: false,
            push_readouts: false,
            unsolicited_audio: false,
        }
    }
}

fn properties(behaviour: Behaviour) -> BTreeMap<String, String> {
    [
        ("api_version", "1.0.3"),
        (
            "can_control",
            if behaviour.steerable { "true" } else { "false" },
        ),
        ("device_center_frequency", "100000000"),
        ("device_sample_rate", "2000000"),
        ("device_vfo_frequency", "100000000"),
        ("lna_state", "4"),
        ("lna_state_min", "0"),
        ("lna_state_max", "9"),
        ("started", "false"),
        ("overload", "false"),
        ("valid_antennas", "Antenna A,Antenna B"),
        ("active_antenna", "Antenna A"),
        ("valid_devices", "RSP1B 1234,RSPduo 5678"),
        ("active_device", "RSP1B 1234"),
        ("filter_bandwidth", "12500"),
        ("demod_max_bandwidth", "200000"),
        ("demodulator", "NFM"),
        ("am_lowcut_frequency", "100"),
        ("ssb_lowcut_frequency", "100"),
        ("nfm_lowcut_frequency", "100"),
        ("nfm_deemphasis_enable", "true"),
        ("wfm_stereo_enable", "true"),
        ("wfm_stereo", "false"),
        ("rds_enable", "true"),
        ("rds_ps", "BBC R4"),
        ("rds_pi", "49129"),
        ("rds_pty", "3"),
        ("rds_radiotext", "Now playing"),
        ("audio_volume_percent", "80"),
        ("audio_mute", "false"),
        ("audio_limiters", "true"),
        ("audio_filter", "true"),
        ("squelch_enable", "false"),
        ("squelch_threshold", "-70"),
        ("agc_enable", "true"),
        ("agc_threshold", "-30"),
        ("noise_reduction_enable", "false"),
        ("noise_reduction_strength", "50"),
        ("spectrum_ref_level", "-20"),
        ("spectrum_base", "-120"),
        ("signal_power", "-83.5"),
        ("signal_snr", "18.25"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

fn envelope(event: &str, property: &str, value: &str, device: &str) -> String {
    format!(
        r#"{{"event_type":"{event}","property":"{property}","value":"{value}","device":"{device}"}}"#
    )
}

fn iq_message(code: u16, sample: i16) -> Vec<u8> {
    let mut bytes = code.to_le_bytes().to_vec();
    bytes.extend(std::iter::repeat_n(sample.to_le_bytes(), 256).flatten());
    bytes
}

fn field(message: &serde_json::Value, name: &str) -> String {
    message
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn serve(peer: &WebSocketPeer, behaviour: Behaviour, nth: usize, observed: &Observed) {
    let mut values = properties(behaviour);
    let mut streaming = false;
    let started = Instant::now();
    loop {
        while let Some(text) = peer.next_message(Duration::from_millis(2)) {
            lock(observed).entry(nth).or_default().push(text.clone());
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let event = field(&message, "event_type");
            let property = field(&message, "property");
            let value = field(&message, "value");
            let device = field(&message, "device");
            let sent = match event.as_str() {
                "get_property" if behaviour.answer_nothing => Ok(()),
                "get_property" => peer.send_text(&envelope(
                    "get_property_response",
                    &property,
                    values.get(&property).map_or("", String::as_str),
                    &device,
                )),
                "set_property" => {
                    values.insert(property.clone(), value.clone());
                    peer.send_text(&envelope("property_changed", &property, &value, &device))
                }
                "iq_stream_enable" => {
                    streaming = value == "true";
                    Ok(())
                }
                "device_stream_enable" => {
                    values.insert("started".to_string(), value.clone());
                    peer.send_text(&envelope("property_changed", "started", &value, &device))
                }
                _ => Ok(()),
            };
            if sent.is_err() {
                return;
            }
        }
        if streaming {
            if behaviour.unsolicited_audio
                && peer.send_binary(&iq_message(1, PRIMARY_SENT)).is_err()
            {
                return;
            }
            let primary = iq_message(2, PRIMARY_SENT);
            let sent = if behaviour.fragment {
                peer.send_fragmented(&primary, 30)
            } else {
                peer.send_binary(&primary)
            };
            if sent.is_err() {
                return;
            }
            if behaviour.both_tuners && peer.send_binary(&iq_message(5, SECONDARY_SENT)).is_err() {
                return;
            }
            if behaviour.push_readouts {
                for (property, value) in [
                    ("signal_power", "-83.5"),
                    ("signal_snr", "18.25"),
                    ("rds_radiotext", "Now playing"),
                    ("overload", "true"),
                ] {
                    if peer
                        .send_text(&envelope("property_changed", property, value, "primary"))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if behaviour.drop_capture
                && nth == CAPTURING
                && started.elapsed() > Duration::from_millis(100)
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn fake_sdrconnect(behaviour: Behaviour) -> (FakeServer, Observed) {
    let observed: Observed = Arc::new(Mutex::new(HashMap::new()));
    let recorder = observed.clone();
    let server = FakeServer::spawn_websocket(move |peer, nth| {
        serve(&peer, behaviour, nth, &recorder);
    });
    (server, observed)
}

fn open(driver: &SdrConnectDriver, key: &str) -> Result<Box<dyn SdrDevice>, DeviceError> {
    let info = driver.resolve(key).expect("an addressable endpoint");
    driver.open(&info)
}

fn blocking_sink() -> (RxSink, mpsc::Receiver<Vec<Sample>>) {
    let (tx, rx) = mpsc::channel();
    (
        RxSink::new(move |samples: &[Sample], _| {
            let _ = tx.send(samples.to_vec());
        }),
        rx,
    )
}

fn messages(observed: &Observed, connection: usize) -> Vec<String> {
    lock(observed).get(&connection).cloned().unwrap_or_default()
}

fn saw(observed: &Observed, connection: usize, needle: &str) -> bool {
    messages(observed, connection)
        .iter()
        .any(|message| message.contains(needle))
}

#[test]
fn opening_reads_the_capability_set_off_the_properties_it_asked_for() {
    let (server, observed) = fake_sdrconnect(Behaviour::default());
    let driver = SdrConnectDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");

    let caps = device.capabilities();
    assert_eq!(caps.freq_ranges[0].min, 1e3);
    assert_eq!(caps.freq_ranges[0].max, 2e9);
    assert_eq!(caps.antennas, vec!["Antenna A", "Antenna B"]);
    assert!(caps.sample_rates.is_empty(), "the API names no rate menu");
    assert_eq!(caps.sample_rate_ranges[0].max, 10e6);

    let lna = caps
        .extra
        .iter()
        .find(|setting| setting.name() == "lna")
        .expect("the RF gain state");
    let ExtraSetting::Range { range, unit, .. } = lna else {
        panic!("the gain state is a range, not {lna:?}");
    };
    assert_eq!((range.min, range.max), (0.0, 9.0));
    assert_eq!(unit, "state");

    let receivers = caps
        .extra
        .iter()
        .find(|setting| setting.name() == "receiver")
        .expect("the receivers on the host");
    let ExtraSetting::Enum {
        options, default, ..
    } = receivers
    else {
        panic!("the receiver choice is an enum, not {receivers:?}");
    };
    assert_eq!(
        options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(),
        vec!["RSP1B 1234", "RSPduo 5678"]
    );
    assert_eq!(default, "RSP1B 1234");

    assert_eq!(device.settings().center_hz, Some(100e6));
    assert_eq!(device.settings().sample_rate, Some(2e6));
    assert_eq!(device.settings().antenna.as_deref(), Some("Antenna A"));

    assert!(
        saw(
            &observed,
            0,
            r#""event_type":"get_property","property":"api_version""#
        ),
        "the API version is asked for like every other property"
    );
    assert!(
        saw(
            &observed,
            0,
            r#""set_primary_device_enable","property":"","value":"true""#
        ),
        "only the tuner this device is gets to speak"
    );
    assert_eq!(server.connections(), 1, "the interrogation hangs up");
}

#[test]
fn a_receiver_that_will_not_be_steered_reports_only_where_it_already_is() {
    let (server, _) = fake_sdrconnect(Behaviour {
        steerable: false,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");
    let caps = device.capabilities();
    assert_eq!(caps.freq_ranges[0].min, 100e6);
    assert_eq!(caps.freq_ranges[0].max, 100e6);
    assert_eq!(caps.sample_rate_ranges[0].min, 2e6);
    assert!(
        !caps.extra.iter().any(|setting| setting.name() == "lna"),
        "a gain the server will refuse is not offered"
    );
}

#[test]
fn a_server_that_upgrades_but_answers_nothing_is_not_taken_for_a_receiver() {
    let (server, _) = fake_sdrconnect(Behaviour {
        answer_nothing: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let Err(error) = open(&driver, &server.endpoint()) else {
        panic!("a server that answers nothing is not a receiver");
    };
    assert!(error.to_string().contains("not SDRconnect"), "{error}");
}

#[test]
fn nothing_listening_is_an_error_naming_the_endpoint() {
    let driver = SdrConnectDriver::new();
    let Err(error) = open(&driver, "127.0.0.1:1") else {
        panic!("nothing is listening on port 1");
    };
    assert!(error.to_string().contains("127.0.0.1:1"), "{error}");
}

#[test]
fn capturing_sets_the_receiver_up_before_it_turns_the_iq_on_and_samples_arrive() {
    let (server, observed) = fake_sdrconnect(Behaviour::default());
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
    assert_eq!(block.len(), 128, "one message, whole");
    assert!(
        (block[0].re - PRIMARY_EXPECTED).abs() < 1e-6,
        "{:?}",
        block[0]
    );
    assert!(
        (block[0].im - PRIMARY_EXPECTED).abs() < 1e-6,
        "{:?}",
        block[0]
    );

    let sent = messages(&observed, CAPTURING);
    let at = |needle: &str| {
        sent.iter()
            .position(|message| message.contains(needle))
            .unwrap_or_else(|| panic!("{needle} was never sent: {sent:?}"))
    };
    assert!(at(r#""device_sample_rate","value":"2000000""#) < at(r#""iq_stream_enable""#));
    assert!(at(r#""device_center_frequency","value":"100000000""#) < at(r#""iq_stream_enable""#));
    assert!(
        at(r#""device_stream_enable","property":"","value":"true""#) < at(r#""iq_stream_enable""#)
    );
    assert!(
        at(r#""audio_stream_enable","property":"","value":"false""#) < at(r#""iq_stream_enable""#),
        "the link carries IQ and nothing else"
    );
    assert!(at(r#""spectrum_enable","property":"","value":"false""#) < at(r#""iq_stream_enable""#));
    device.rx_stop();
}

#[test]
fn a_message_split_across_frames_is_still_one_block() {
    let (server, _) = fake_sdrconnect(Behaviour {
        fragment: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    for _ in 0..3 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert_eq!(
            block.len(),
            128,
            "the framer held the split message together"
        );
        assert!((block[0].re - PRIMARY_EXPECTED).abs() < 1e-6);
    }
    device.rx_stop();
}

#[test]
fn a_device_takes_only_the_binary_payloads_of_the_tuner_it_is() {
    let (server, _) = fake_sdrconnect(Behaviour {
        both_tuners: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let mut primary = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    primary.rx_start(vec![sink]).expect("streams");
    for _ in 0..3 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert!(
            (block[0].re - PRIMARY_EXPECTED).abs() < 1e-6,
            "the secondary tuner's IQ must not reach the primary device: {:?}",
            block[0]
        );
    }
    primary.rx_stop();

    let mut secondary = open(&driver, &format!("{}/secondary", server.endpoint())).expect("opens");
    let (sink, blocks) = blocking_sink();
    secondary.rx_start(vec![sink]).expect("streams");
    for _ in 0..3 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert!(
            (block[0].re - SECONDARY_EXPECTED).abs() < 1e-6,
            "the second tuner reads payload type 5: {:?}",
            block[0]
        );
    }
    secondary.rx_stop();
}

#[test]
fn a_retune_while_streaming_reaches_the_server() {
    let (server, observed) = fake_sdrconnect(Behaviour::default());
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");

    device
        .apply(&DeviceSettings {
            center_hz: Some(144_800_000.0),
            antenna: Some("Antenna B".to_string()),
            extra: vec![ExtraValue {
                name: "lna".to_string(),
                value: 2.into(),
            }],
            ..DeviceSettings::default()
        })
        .expect("retunes");

    eventually("the retune", || {
        saw(
            &observed,
            CAPTURING,
            r#""device_center_frequency","value":"144800000""#,
        )
        .then_some(())
    });
    eventually("the antenna", || {
        saw(
            &observed,
            CAPTURING,
            r#""active_antenna","value":"Antenna B""#,
        )
        .then_some(())
    });
    eventually("the gain state", || {
        saw(&observed, CAPTURING, r#""lna_state","value":"2""#).then_some(())
    });
    assert_eq!(device.settings().center_hz, Some(144_800_000.0));
    device.rx_stop();
}

#[test]
fn a_dropped_connection_reconnects_and_replays_the_setup() {
    let (server, observed) = fake_sdrconnect(Behaviour {
        drop_capture: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    device
        .apply(&DeviceSettings {
            center_hz: Some(433_920_000.0),
            ..DeviceSettings::default()
        })
        .expect("accepted");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks
        .recv_timeout(DEADLINE)
        .expect("samples before the drop");

    eventually("a reconnect", || {
        (server.connections() > RECONNECTED).then_some(())
    });
    eventually("the replayed setup", || {
        saw(
            &observed,
            RECONNECTED,
            r#""iq_stream_enable","property":"","value":"true""#,
        )
        .then_some(())
    });
    assert!(
        saw(
            &observed,
            RECONNECTED,
            r#""device_center_frequency","value":"433920000""#
        ),
        "the operator's tuning came back with the connection"
    );

    let deadline = Instant::now() + DEADLINE;
    while blocks.recv_timeout(Duration::from_millis(100)).is_err() {
        assert!(Instant::now() < deadline, "the stream never resumed");
    }
    device.rx_stop();
}

#[test]
fn stopping_asks_the_server_for_no_more_iq_and_leaves_the_receiver_as_it_found_it() {
    let (server, observed) = fake_sdrconnect(Behaviour::default());
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");
    device.rx_stop();

    eventually("the IQ to be switched off", || {
        saw(
            &observed,
            CAPTURING,
            r#""iq_stream_enable","property":"","value":"false""#,
        )
        .then_some(())
    });
    assert!(
        saw(
            &observed,
            CAPTURING,
            r#""device_stream_enable","property":"","value":"false""#
        ),
        "a receiver SDR-- started is stopped again"
    );
}

#[test]
fn the_controls_are_the_ones_that_shape_the_iq_and_no_others() {
    let (server, _) = fake_sdrconnect(Behaviour::default());
    let driver = SdrConnectDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");
    assert_eq!(
        device
            .capabilities()
            .extra
            .iter()
            .map(ExtraSetting::name)
            .collect::<Vec<_>>(),
        vec![
            "lna",
            "device_vfo_frequency",
            "filter_bandwidth",
            "receiver",
            "network_mode",
            "device_profile",
            "recording"
        ],
        "this receiver answered for its whole audio chain, and none of that is a control"
    );
}

#[test]
fn audio_the_server_sends_anyway_is_switched_off_and_never_read_as_samples() {
    let (server, observed) = fake_sdrconnect(Behaviour {
        unsolicited_audio: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    for _ in 0..4 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert_eq!(block.len(), 128, "demodulated audio is not a block of IQ");
        assert!(
            (block[0].re - PRIMARY_EXPECTED).abs() < 1e-6,
            "demodulated audio must never be read as IQ: {:?}",
            block[0]
        );
    }
    assert!(
        saw(
            &observed,
            CAPTURING,
            r#""audio_stream_enable","property":"","value":"false""#
        ),
        "the capture asked for the IQ and nothing else"
    );
    device.rx_stop();
}

#[test]
fn what_the_receiver_reports_back_never_disturbs_the_samples() {
    let (server, _) = fake_sdrconnect(Behaviour {
        push_readouts: true,
        ..Behaviour::default()
    });
    let driver = SdrConnectDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    for _ in 0..4 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert_eq!(block.len(), 128, "a readout is not a block");
        assert!((block[0].re - PRIMARY_EXPECTED).abs() < 1e-6);
    }
    device.rx_stop();
}
