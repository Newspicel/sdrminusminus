#![allow(clippy::expect_used)]
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, lock,
    net::testing::{DEADLINE, FakeServer, WebSocketPeer, eventually},
};
use sdrmm_device_kiwisdr::KiwiSdrDriver;
use sdrmm_wire::{AgcSetting, DeviceSettings, GainKind, GainValue};

const FRAME_PAIRS: usize = 512;
const SENT: i16 = 16_384;

#[derive(Clone, Copy, Default)]
struct Script {
    refuse: bool,
    skip_seq: bool,
    kick_after: Option<u32>,
    hang_up_after: Option<u32>,
    password: Option<&'static str>,
}

fn frame(seq: u32) -> Vec<u8> {
    let mut out = b"SND".to_vec();
    out.push(0x08);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&[0; 12]);
    for _ in 0..FRAME_PAIRS {
        out.extend_from_slice(&SENT.to_be_bytes());
        out.extend_from_slice(&(-SENT).to_be_bytes());
    }
    out
}

fn kiwi(script: Script, heard: Arc<Mutex<Vec<String>>>) -> FakeServer {
    FakeServer::spawn_websocket(move |peer: WebSocketPeer, nth| {
        let Some(login) = peer.next_message(DEADLINE) else {
            return;
        };
        let wanted = format!("SET auth t=kiwi p={}", script.password.unwrap_or(""));
        let admitted = login == wanted;
        lock(&heard).push(login);
        if !admitted {
            let _ = peer.send_binary(b"MSG badp=1");
            return;
        }
        if script.refuse {
            let _ = peer.send_binary(b"MSG too_busy=4");
            return;
        }
        for line in [
            "MSG sample_rate=12000.000",
            "MSG rx_chans=4",
            "MSG version_maj=1 version_min=902 freq_offset=0.000",
            "MSG center_freq=15000000 bandwidth=30000000",
            "MSG audio_init=0 audio_rate=12000",
        ] {
            let _ = peer.send_binary(line.as_bytes());
        }
        let mut seq = 0u32;
        let mut tuned = false;
        loop {
            while let Some(message) = peer.next_message(Duration::from_millis(5)) {
                tuned |= message.starts_with("SET mod=iq");
                lock(&heard).push(message);
            }
            if !tuned {
                continue;
            }
            if nth == 0 && script.hang_up_after == Some(seq) {
                peer.hang_up();
                return;
            }
            if script.kick_after == Some(seq) {
                let _ = peer.send_binary(b"MSG kiwi_kick=1,bye");
                return;
            }
            seq += if script.skip_seq && seq == 3 { 3 } else { 1 };
            if peer.send_binary(&frame(seq)).is_err() {
                return;
            }
        }
    })
}

type Received = mpsc::Receiver<Vec<Sample>>;
type Failure = Arc<Mutex<Option<DeviceError>>>;

fn sink() -> (RxSink, Received, Arc<Mutex<u64>>, Failure) {
    let (tx, rx) = mpsc::channel();
    let dropped = Arc::new(Mutex::new(0u64));
    let failure: Failure = Arc::new(Mutex::new(None));
    let counted = dropped.clone();
    let failed = failure.clone();
    let mut received = 0u64;
    let sink = RxSink::with_fatal_handler(
        move |samples, index| {
            *lock(&counted) = index - received;
            received += samples.len() as u64;
            let _ = tx.send(samples.to_vec());
        },
        move |e| *lock(&failed) = Some(e),
    );
    (sink, rx, dropped, failure)
}

fn open(server: &FakeServer) -> Result<Box<dyn SdrDevice>, DeviceError> {
    open_as(server, "")
}

fn open_as(server: &FakeServer, credentials: &str) -> Result<Box<dyn SdrDevice>, DeviceError> {
    let driver = KiwiSdrDriver::new();
    let info = driver
        .resolve(&format!("{credentials}{}", server.endpoint()))
        .expect("addressable");
    driver.open(&info)
}

fn saw(heard: &Arc<Mutex<Vec<String>>>, needle: &str) -> Option<()> {
    lock(heard)
        .iter()
        .any(|m| m.starts_with(needle))
        .then_some(())
}

#[test]
fn a_kiwi_streams_iq_and_follows_retunes() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let server = kiwi(Script::default(), heard.clone());
    let mut device = open(&server).expect("opens");
    assert_eq!(device.capabilities().sample_rates, vec![12_000.0]);
    assert_eq!(device.settings().center_hz, Some(15e6));

    let (sink, rx, _, _) = sink();
    device.rx_start(vec![sink]).expect("starts");
    let block = rx.recv_timeout(DEADLINE).expect("samples");
    assert!((block[0].re - 0.5).abs() < 1e-6);
    assert!((block[0].im + 0.5).abs() < 1e-6);
    eventually("the auth and audio-rate ack", || {
        saw(&heard, "SET AR OK in=12000")
    });
    eventually("the ident", || saw(&heard, "SET ident_user=SDR--"));
    eventually("the first tune", || {
        saw(
            &heard,
            "SET mod=iq low_cut=-6000 high_cut=6000 freq=15000.000",
        )
    });

    device
        .apply(&DeviceSettings {
            center_hz: Some(7_074_000.0),
            agc: Some(AgcSetting::switched(false)),
            gains: vec![GainValue::new(GainKind::Rf, 30.0)],
            ..DeviceSettings::default()
        })
        .expect("applies");
    eventually("the retune", || {
        saw(
            &heard,
            "SET mod=iq low_cut=-6000 high_cut=6000 freq=7074.000",
        )
    });
    eventually("manual gain", || {
        saw(
            &heard,
            "SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=30",
        )
    });
    assert_eq!(device.settings().center_hz, Some(7_074_000.0));
    device.rx_stop();
}

#[test]
fn a_skipped_frame_is_reported_as_dropped_samples() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let script = Script {
        skip_seq: true,
        ..Script::default()
    };
    let server = kiwi(script, heard);
    let mut device = open(&server).expect("opens");
    let (sink, rx, dropped, _) = sink();
    device.rx_start(vec![sink]).expect("starts");
    eventually("the gap", || {
        while rx.try_recv().is_ok() {}
        (*lock(&dropped) == 2 * FRAME_PAIRS as u64).then_some(())
    });
    device.rx_stop();
}

#[test]
fn a_busy_kiwi_refuses_the_open() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let script = Script {
        refuse: true,
        ..Script::default()
    };
    let server = kiwi(script, heard);
    let err = open(&server).err().expect("refused");
    assert!(
        matches!(&err, DeviceError::InUse(m) if m.contains("busy")),
        "{err}"
    );
}

#[test]
fn a_kick_ends_the_stream_without_reconnecting() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let script = Script {
        kick_after: Some(5),
        ..Script::default()
    };
    let server = kiwi(script, heard);
    let mut device = open(&server).expect("opens");
    let (sink, _rx, _, failure) = sink();
    device.rx_start(vec![sink]).expect("starts");
    let err = eventually("the kick", || lock(&failure).clone());
    assert!(
        matches!(&err, DeviceError::Disconnected(m) if m.contains("admin")),
        "{err}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.connections(), 1);
    device.rx_stop();
}

#[test]
fn a_dropped_connection_reconnects_and_replays_the_tuning() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let script = Script {
        hang_up_after: Some(5),
        ..Script::default()
    };
    let server = kiwi(script, heard.clone());
    let mut device = open(&server).expect("opens");
    device
        .apply(&DeviceSettings {
            center_hz: Some(10e6),
            ..DeviceSettings::default()
        })
        .expect("applies");
    let (sink, rx, _, failure) = sink();
    device.rx_start(vec![sink]).expect("starts");
    eventually("a third connection", || {
        (server.connections() == 2).then_some(())
    });
    while rx.try_recv().is_ok() {}
    rx.recv_timeout(DEADLINE)
        .expect("samples after the reconnect");
    assert!(lock(&failure).is_none());
    let tunes = lock(&heard)
        .iter()
        .filter(|m| m.ends_with("freq=10000.000"))
        .count();
    assert_eq!(tunes, 2);
    device.rx_stop();
}

#[test]
fn a_private_kiwi_needs_its_password() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let script = Script {
        password: Some("s%20cret"),
        ..Script::default()
    };
    let server = kiwi(script, heard);
    let err = open(&server).err().expect("refused");
    assert!(
        matches!(&err, DeviceError::PermissionDenied(m) if m.contains("password")),
        "{err}"
    );
    let mut device = open_as(&server, "s cret@").expect("opens with the password");
    let (sink, rx, _, _) = sink();
    device.rx_start(vec![sink]).expect("starts");
    rx.recv_timeout(DEADLINE).expect("samples");
    device.rx_stop();
}
