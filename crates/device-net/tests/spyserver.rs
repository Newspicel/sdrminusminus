#![allow(clippy::expect_used)]
//! A harness that cannot bind a loopback socket or clone it has nothing left to assert,
//! so its helpers panic. Clippy exempts `#[test]` functions from this by config, but not
//! the free functions and closures a fake server is built out of.
//! The SpyServer backend against a fake server (PLAN §14: no hardware in CI, ever).
//!
//! What only a socket can show: the handshake, the capability set read off it, the settings the
//! server observes, message framing against a body split across reads, and the reconnect.

mod common;

use std::{
    collections::HashMap,
    io::{Read as _, Write as _},
    net::TcpStream,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use common::{DEADLINE, FakeServer, eventually, lock};
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, Sample, SdrDevice};
use sdrmm_device_net::SpyServerDriver;
use sdrmm_wire::{DeviceSettings, ExtraSetting, ExtraValue};

const PROTOCOL_VERSION: u32 = (2 << 24) | 1700;
const MSG_DEVICE_INFO: u16 = 0;
const MSG_CLIENT_SYNC: u16 = 1;
const MSG_INT16_IQ: u16 = 101;
const STREAM_TYPE_STATUS: u32 = 0;
const STREAM_TYPE_IQ: u32 = 1;
const SETTING_STREAMING_ENABLED: u32 = 1;
const SETTING_GAIN: u32 = 2;
const SETTING_IQ_FORMAT: u32 = 100;
const SETTING_IQ_FREQUENCY: u32 = 101;
const SETTING_IQ_DECIMATION: u32 = 102;
const SETTING_IQ_DIGITAL_GAIN: u32 = 103;

/// Settings a fake server observed, keyed by which connection carried them.
type Observed = Arc<Mutex<HashMap<usize, Vec<(u32, u32)>>>>;

/// The device dials once to handshake and hangs up, so the capture is always the second connection
/// and a reconnect the third.
const CAPTURING: usize = 1;
const RECONNECTED: usize = 2;

/// The sample value every IQ message in these tests carries, and what it must arrive as: half of
/// full scale, with no digital gain asked for at the lowest decimation.
const SENT: i16 = 16_384;
const EXPECTED: f32 = 0.5;

fn message(kind: u16, flags: u16, stream_type: u32, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(20 + body.len());
    bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(u32::from(kind) | (u32::from(flags) << 16)).to_le_bytes());
    bytes.extend_from_slice(&stream_type.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(body);
    bytes
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// An RTL-SDR behind a server that will be steered: 8 MS/s, four decimation stages of which the
/// first is mandatory, a 29-step gain table folded into indices 0..=28.
fn device_info() -> Vec<u8> {
    words(&[
        3,
        0x00C0_FFEE,
        8_000_000,
        8_000_000,
        4,
        1,
        28,
        24_000_000,
        1_766_000_000,
        8,
        1,
        0,
    ])
}

fn client_sync(can_control: bool) -> Vec<u8> {
    words(&[
        u32::from(can_control),
        12,
        100_000_000,
        100_000_000,
        100_000_000,
        99_000_000,
        101_000_000,
        99_000_000,
        101_000_000,
    ])
}

/// How a fake server should behave beyond the handshake.
#[derive(Clone, Copy)]
struct Behaviour {
    can_control: bool,
    /// Close the capturing connection after a moment, the way a restarted server would.
    drop_capture: bool,
    /// Write each IQ message in two writes with a pause between, so the framer has to carry a body
    /// across reads.
    split_messages: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            can_control: true,
            drop_capture: false,
            split_messages: false,
        }
    }
}

/// A server that handshakes, records every setting, and streams int16 IQ once it is enabled.
fn fake_spyserver(behaviour: Behaviour) -> (FakeServer, Observed) {
    let observed: Observed = Arc::new(Mutex::new(HashMap::new()));
    let recorder = observed.clone();
    let server = FakeServer::spawn(move |mut stream: TcpStream, nth| {
        // The client's hello: a command header and a body it is free to size.
        let mut header = [0u8; 8];
        if stream.read_exact(&mut header).is_err() {
            return;
        }
        let body_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; body_size];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        if stream
            .write_all(&message(
                MSG_DEVICE_INFO,
                0,
                STREAM_TYPE_STATUS,
                &device_info(),
            ))
            .is_err()
        {
            return;
        }
        if stream
            .write_all(&message(
                MSG_CLIENT_SYNC,
                0,
                STREAM_TYPE_STATUS,
                &client_sync(behaviour.can_control),
            ))
            .is_err()
        {
            return;
        }

        let settings = recorder.clone();
        let streaming = Arc::new(Mutex::new(false));
        let enabled = streaming.clone();
        let mut reader = stream.try_clone().expect("clone for reading");
        std::thread::spawn(move || {
            let mut frame = [0u8; 16];
            while reader.read_exact(&mut frame).is_ok() {
                let target = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
                let value = u32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]);
                lock(&settings)
                    .entry(nth)
                    .or_default()
                    .push((target, value));
                if target == SETTING_STREAMING_ENABLED {
                    *lock(&enabled) = value == 1;
                }
            }
        });

        // 64 samples per message, all at the same level so a test can name what it expects.
        let body: Vec<u8> = std::iter::repeat_n(SENT.to_le_bytes(), 128)
            .flatten()
            .collect();
        let iq = message(MSG_INT16_IQ, 0, STREAM_TYPE_IQ, &body);
        let started = std::time::Instant::now();
        loop {
            if *lock(&streaming) {
                let written = if behaviour.split_messages {
                    let (head, tail) = iq.split_at(30);
                    stream.write_all(head).and_then(|()| {
                        std::thread::sleep(Duration::from_millis(1));
                        stream.write_all(tail)
                    })
                } else {
                    stream.write_all(&iq)
                };
                if written.is_err() {
                    return;
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
    });
    (server, observed)
}

fn open(driver: &SpyServerDriver, endpoint: &str) -> Result<Box<dyn SdrDevice>, DeviceError> {
    let info = driver.resolve(endpoint).expect("an addressable endpoint");
    driver.open(&info)
}

fn blocking_sink() -> (RxSink, mpsc::Receiver<Vec<Sample>>) {
    let (tx, rx) = mpsc::channel();
    (
        RxSink::new(move |samples: &[Sample]| {
            let _ = tx.send(samples.to_vec());
        }),
        rx,
    )
}

fn settings(observed: &Observed, connection: usize) -> Vec<(u32, u32)> {
    lock(observed).get(&connection).cloned().unwrap_or_default()
}

#[test]
fn opening_reads_the_capability_set_off_the_handshake() {
    let (server, _) = fake_spyserver(Behaviour::default());
    let driver = SpyServerDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");

    let caps = device.capabilities();
    assert_eq!(caps.freq_ranges[0].min, 24e6, "the receiver's own tuner");
    assert_eq!(caps.freq_ranges[0].max, 1.766e9);
    assert_eq!(
        caps.sample_rates,
        vec![500_000.0, 1_000_000.0, 2_000_000.0, 4_000_000.0],
        "the maximum halved once per stage, and never the undecimated rate"
    );
    assert!(caps.gains.is_empty(), "this protocol has no gain in dB");
    let gain = caps
        .extra
        .iter()
        .find(|setting| setting.name() == "gain")
        .expect("the gain index");
    let ExtraSetting::Range { range, unit, .. } = gain else {
        panic!("the gain index is a range, not {gain:?}");
    };
    assert_eq!(range.max, 28.0);
    assert_eq!(unit, "index");

    // Opening a receiver somebody else may be listening to must not move it.
    assert_eq!(device.settings().center_hz, Some(100_000_000.0));
    assert_eq!(server.connections(), 1, "the handshake hangs up");
}

/// A locked server's frequency range genuinely *is* the window it will let this client slide
/// inside — and offering a gain control it would refuse would be a knob that does nothing.
#[test]
fn a_server_that_will_not_be_steered_reports_only_what_it_will_move() {
    let (server, _) = fake_spyserver(Behaviour {
        can_control: false,
        ..Behaviour::default()
    });
    let driver = SpyServerDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");
    let caps = device.capabilities();
    assert_eq!(caps.freq_ranges[0].min, 99e6);
    assert_eq!(caps.freq_ranges[0].max, 101e6);
    assert!(!caps.extra.iter().any(|setting| setting.name() == "gain"));
}

#[test]
fn nothing_listening_is_an_error_naming_the_endpoint() {
    let driver = SpyServerDriver::new();
    let Err(error) = open(&driver, "127.0.0.1:1") else {
        panic!("nothing is listening on port 1");
    };
    assert!(error.to_string().contains("127.0.0.1:1"), "{error}");
}

#[test]
fn capturing_configures_the_stream_before_enabling_it_and_samples_arrive() {
    let (server, observed) = fake_spyserver(Behaviour::default());
    let driver = SpyServerDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
    assert_eq!(block.len(), 64, "one message, whole");
    assert!((block[0].re - EXPECTED).abs() < 1e-6, "{:?}", block[0]);
    assert!((block[0].im - EXPECTED).abs() < 1e-6, "{:?}", block[0]);

    assert_eq!(
        settings(&observed, CAPTURING),
        vec![
            (SETTING_IQ_FORMAT, 2),
            (SETTING_IQ_DECIMATION, 1),
            (SETTING_IQ_FREQUENCY, 100_000_000),
            (0, 1),
            (SETTING_GAIN, 12),
            (SETTING_IQ_DIGITAL_GAIN, 3),
            (SETTING_STREAMING_ENABLED, 1),
        ],
        "the stream is set up, then turned on"
    );
    device.rx_stop();
}

/// A TCP read is entitled to split anything; a framer that assumed a message arrives whole would
/// desynchronise on the first one that does not.
#[test]
fn a_message_split_across_reads_is_still_one_block() {
    let (server, _) = fake_spyserver(Behaviour {
        split_messages: true,
        ..Behaviour::default()
    });
    let driver = SpyServerDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    for _ in 0..3 {
        let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
        assert_eq!(block.len(), 64, "the framer held the split body together");
        assert!((block[0].re - EXPECTED).abs() < 1e-6);
    }
    device.rx_stop();
}

#[test]
fn a_retune_while_streaming_reaches_the_server() {
    let (server, observed) = fake_spyserver(Behaviour::default());
    let driver = SpyServerDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");

    device
        .apply(&DeviceSettings {
            center_hz: Some(144_800_000.0),
            extra: vec![ExtraValue {
                name: "gain".to_string(),
                value: 20.into(),
            }],
            ..DeviceSettings::default()
        })
        .expect("retunes");
    eventually("the retune", || {
        settings(&observed, CAPTURING).contains(&(SETTING_IQ_FREQUENCY, 144_800_000))
    });
    eventually("the gain", || {
        settings(&observed, CAPTURING).contains(&(SETTING_GAIN, 20))
    });
    assert_eq!(device.settings().center_hz, Some(144_800_000.0));
    device.rx_stop();
}

/// The same behaviour a remote radio lives or dies by: the server restarts, and the reconnect sets
/// the whole stream up again rather than resuming into a server that has forgotten this client.
#[test]
fn a_dropped_connection_reconnects_and_replays_the_stream_setup() {
    let (server, observed) = fake_spyserver(Behaviour {
        drop_capture: true,
        ..Behaviour::default()
    });
    let driver = SpyServerDriver::new();
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

    eventually("a reconnect", || server.connections() > RECONNECTED);
    eventually("the replayed setup", || {
        settings(&observed, RECONNECTED).contains(&(SETTING_STREAMING_ENABLED, 1))
    });
    assert!(
        settings(&observed, RECONNECTED).contains(&(SETTING_IQ_FREQUENCY, 433_920_000)),
        "the operator's tuning came back with the connection"
    );

    let deadline = std::time::Instant::now() + DEADLINE;
    while blocks.recv_timeout(Duration::from_millis(100)).is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "the stream never resumed"
        );
    }
    device.rx_stop();
}

/// A shared receiver must not be left producing for a client that has gone.
#[test]
fn stopping_asks_the_server_to_stop_streaming() {
    let (server, observed) = fake_spyserver(Behaviour::default());
    let driver = SpyServerDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");
    device.rx_stop();
    eventually("the streaming-off setting", || {
        settings(&observed, CAPTURING).contains(&(SETTING_STREAMING_ENABLED, 0))
    });
}
