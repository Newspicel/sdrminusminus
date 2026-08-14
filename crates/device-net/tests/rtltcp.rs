#![allow(clippy::expect_used)]
//! A harness that cannot bind a loopback socket or clone it has nothing left to assert,
//! so its helpers panic. Clippy exempts `#[test]` functions from this by config, but not
//! the free functions and closures a fake server is built out of.
//! The rtl_tcp backend against a fake server (: no hardware in CI, ever).
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
use sdrmm_device_net::RtlTcpDriver;
use sdrmm_wire::{DeviceSettings, ExtraValue, GainValue};

/// Command bytes a fake server observed, keyed by which connection carried them.
type Observed = Arc<Mutex<HashMap<usize, Vec<(u8, u32)>>>>;

/// Which connection is which. The device dials once to read the greeting and hangs up, so the
/// capture is always the *second* — and a reconnect the third.
const OPENED: usize = 0;
const CAPTURING: usize = 1;
const RECONNECTED: usize = 2;

/// Commands in a replay with the tuner's AGC on: rate, centre, correction, gain mode, the
/// RTL2832U's AGC and the bias tee. A manual gain adds its value behind the mode, making seven.
const REPLAY_IN_AGC: usize = 6;

/// The R820T greeting, which is what all but one of these tests want.
fn greeting(magic: &[u8; 4], tuner: u32, gain_steps: u32) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(magic);
    bytes[4..8].copy_from_slice(&tuner.to_be_bytes());
    bytes[8..].copy_from_slice(&gain_steps.to_be_bytes());
    bytes
}

/// Every sample value the RTL2832U can produce, in order, so a test can name the byte it expects
/// rather than a level.
fn ramp() -> Vec<u8> {
    (0..=255u8).collect()
}

/// A server that greets, then streams a ramp forever, recording the commands it is sent.
///
/// `drop_capture` closes the capturing connection after a moment, which is the only way a test can
/// make a healthy stream fail the way a restarted server does.
fn fake_rtl_tcp(
    magic: [u8; 4],
    tuner: u32,
    gain_steps: u32,
    drop_capture: bool,
) -> (FakeServer, Observed) {
    let observed: Observed = Arc::new(Mutex::new(HashMap::new()));
    let recorder = observed.clone();
    let server = FakeServer::spawn(move |mut stream: TcpStream, nth| {
        if stream
            .write_all(&greeting(&magic, tuner, gain_steps))
            .is_err()
        {
            return;
        }
        let commands = recorder.clone();
        let mut reader = stream.try_clone().expect("clone for reading");
        std::thread::spawn(move || {
            let mut frame = [0u8; 5];
            while reader.read_exact(&mut frame).is_ok() {
                let param = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
                lock(&commands)
                    .entry(nth)
                    .or_default()
                    .push((frame[0], param));
            }
        });
        let block = ramp();
        let started = std::time::Instant::now();
        while stream.write_all(&block).is_ok() {
            if drop_capture && nth == CAPTURING && started.elapsed() > Duration::from_millis(100) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    (server, observed)
}

/// Open the device the way the server does: adopt the endpoint, then open the key it canonicalized
/// to.
fn open(driver: &RtlTcpDriver, endpoint: &str) -> Result<Box<dyn SdrDevice>, DeviceError> {
    let info = driver.resolve(endpoint).expect("an addressable endpoint");
    driver.open(&info)
}

/// A sink that hands every block to the test.
fn blocking_sink() -> (RxSink, mpsc::Receiver<Vec<Sample>>) {
    let (tx, rx) = mpsc::channel();
    (
        RxSink::new(move |samples: &[Sample]| {
            let _ = tx.send(samples.to_vec());
        }),
        rx,
    )
}

fn commands(observed: &Observed, connection: usize) -> Vec<(u8, u32)> {
    lock(observed).get(&connection).cloned().unwrap_or_default()
}

#[test]
fn opening_reads_the_greeting_and_reports_that_tuners_capabilities() {
    let (server, _) = fake_rtl_tcp(*b"RTL0", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");

    let caps = device.capabilities();
    assert_eq!(caps.freq_ranges.len(), 1, "the R820T's one range");
    assert_eq!(caps.freq_ranges[0].min, 24e6);
    assert_eq!(caps.gains[0].name, "TUNER");
    assert_eq!(caps.gains[0].range.max, 49.6, "the tuner's own table");
    assert!(caps.sample_rates.contains(&2_048_000.0));

    assert_eq!(server.connections(), OPENED + 1);
    let settings = device.settings();
    assert_eq!(settings.center_hz, Some(100_000_000.0));
    assert_eq!(settings.sample_rate, Some(2_048_000.0));
}

#[test]
fn a_tuner_this_backend_has_no_table_for_still_opens() {
    let (server, _) = fake_rtl_tcp(*b"RTL0", 0, 0, false);
    let driver = RtlTcpDriver::new();
    let device = open(&driver, &server.endpoint()).expect("opens");
    assert!(
        device.capabilities().freq_ranges.is_empty(),
        "an unknown tuner is filtered on nothing rather than on a guess"
    );
}

/// Reading whatever answers the port as IQ would show a plausible noise floor forever.
#[test]
fn a_port_that_is_not_an_rtl_tcp_server_is_refused_by_name() {
    let (server, _) = fake_rtl_tcp(*b"HTTP", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let Err(error) = open(&driver, &server.endpoint()) else {
        panic!("a server that is not rtl_tcp must be refused");
    };
    assert!(error.to_string().contains("RTL0"), "{error}");
}

#[test]
fn nothing_listening_is_an_error_naming_the_endpoint() {
    let driver = RtlTcpDriver::new();
    let Err(error) = open(&driver, "127.0.0.1:1") else {
        panic!("nothing is listening on port 1");
    };
    assert!(error.to_string().contains("127.0.0.1:1"), "{error}");
}

#[test]
fn capturing_replays_every_setting_before_the_first_sample_and_streams() {
    let (server, observed) = fake_rtl_tcp(*b"RTL0", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    device
        .apply(&DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(2_400_000.0),
            gains: vec![GainValue {
                stage: "TUNER".to_string(),
                value_db: 25.4,
            }],
            extra: vec![ExtraValue {
                name: "bias_tee".to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        })
        .expect("accepted while not streaming");

    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
    assert!(!block.is_empty());

    // The whole state, in the order the radio needs it — not just the delta, because a fresh
    // connection is a dongle at its power-on defaults.
    eventually("the replay", || {
        commands(&observed, CAPTURING).len() > REPLAY_IN_AGC
    });
    assert_eq!(
        commands(&observed, CAPTURING),
        vec![
            (0x02, 2_400_000),
            (0x01, 433_920_000),
            (0x05, 0),
            (0x03, 1),
            (0x04, 254),
            (0x08, 0),
            (0x0e, 1),
        ]
    );

    device.rx_stop();
}

#[test]
fn a_retune_while_streaming_reaches_the_server() {
    let (server, observed) = fake_rtl_tcp(*b"RTL0", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");
    eventually("the replay", || {
        commands(&observed, CAPTURING).len() >= REPLAY_IN_AGC
    });

    device
        .apply(&DeviceSettings {
            center_hz: Some(144_800_000.0),
            ..DeviceSettings::default()
        })
        .expect("retunes");
    eventually("the retune", || {
        commands(&observed, CAPTURING).contains(&(0x01, 144_800_000))
    });
    assert_eq!(device.settings().center_hz, Some(144_800_000.0));
    device.rx_stop();
}

/// The samples the far side sent have to arrive as the samples the in-tree RTL-SDR driver would
/// have produced from the same bytes — the transport is the only thing that differs.
#[test]
fn the_bytes_on_the_wire_arrive_as_rtl_sdr_samples() {
    let (server, _) = fake_rtl_tcp(*b"RTL0", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");

    let block = blocks.recv_timeout(DEADLINE).expect("samples arrive");
    let expected = |code: u8| (f32::from(code) - 127.4) / 127.5;
    assert!((block[0].re - expected(0)).abs() < 1e-6, "{:?}", block[0]);
    assert!((block[0].im - expected(1)).abs() < 1e-6, "{:?}", block[0]);
    assert!((block[1].re - expected(2)).abs() < 1e-6, "{:?}", block[1]);
    device.rx_stop();
}

/// The behaviour a remote radio lives or dies by: a server that restarts costs a reconnect, and
/// the reconnect puts the tuning back before a single sample is pushed.
#[test]
fn a_dropped_connection_reconnects_and_replays_the_tuning() {
    let (server, observed) = fake_rtl_tcp(*b"RTL0", 5, 29, true);
    let driver = RtlTcpDriver::new();
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
    eventually("the replayed tuning", || {
        commands(&observed, RECONNECTED).contains(&(0x01, 433_920_000))
    });
    assert_eq!(
        commands(&observed, RECONNECTED).len(),
        REPLAY_IN_AGC,
        "a fresh connection is a dongle at its defaults: every setting goes again"
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

#[test]
fn stopping_closes_the_connection_and_the_device_can_stream_again() {
    let (server, _) = fake_rtl_tcp(*b"RTL0", 5, 29, false);
    let driver = RtlTcpDriver::new();
    let mut device = open(&driver, &server.endpoint()).expect("opens");
    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams");
    blocks.recv_timeout(DEADLINE).expect("samples arrive");
    device.rx_stop();
    device.rx_stop();

    let (sink, blocks) = blocking_sink();
    device.rx_start(vec![sink]).expect("streams again");
    blocks.recv_timeout(DEADLINE).expect("samples arrive again");
    device.rx_stop();
    assert!(server.connections() >= 3);
}
