#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    io::Read,
    net::{TcpListener, UdpSocket},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::{Engine, EngineError};
use sdrmm_wire::{DeviceSettings, NetworkExportSettings, NetworkSampleFormat, NetworkTransport};

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn a_stalled_tcp_reader_reports_failure_while_radio_audio_stays_continuous() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let engine = engine();
    let ds = engine.create_device_set("virtual:siggen").expect("source");
    let channel = engine
        .add_channel(
            ds,
            0,
            sdrmm_wire::ChannelSettings {
                frequency_hz: 100_000_000.0 + sdrmm_device_virtual::NFM_CARRIER_OFFSET_HZ,
                squelch: sdrmm_wire::Squelch::Off,
                params: sdrmm_wire::ChannelParams::Nfm(sdrmm_wire::NfmParams::default()),
                audio: Default::default(),
            },
        )
        .expect("channel");
    let mut audio = engine.subscribe_pcm(ds, channel).expect("PCM");
    engine
        .start_network_export(
            ds,
            "stalled".to_owned(),
            0,
            NetworkExportSettings {
                transport: NetworkTransport::Tcp,
                format: NetworkSampleFormat::Cf32Le,
                address: listener.local_addr().expect("address").to_string(),
            },
        )
        .expect("export");
    let (receiver, _) = listener.accept().expect("connected reader");
    let deadline = Instant::now() + WAIT;
    let mut failed_at = None;
    let mut next_frame = None;
    let mut frames = 0;
    loop {
        loop {
            match audio.try_recv() {
                Ok(block) => {
                    if let Some(next) = next_frame {
                        assert_eq!(block.start_frame, next, "audio gap during export failure");
                    }
                    let count = match block.payload {
                        sdrmm_engine::audio::PcmPayload::Silence(frames) => frames,
                        sdrmm_engine::audio::PcmPayload::Samples(samples) => {
                            assert!(samples.iter().all(|sample| sample.is_finite()));
                            samples.len() / usize::from(block.channels)
                        }
                    };
                    next_frame = Some(block.start_frame + count as u64);
                    frames += count;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(error) => panic!("PCM observer failed: {error}"),
            }
        }
        let snapshot = engine.snapshot();
        let set = &snapshot.device_sets[0];
        assert_eq!(set.overruns, 0);
        if set
            .network_export
            .as_ref()
            .is_some_and(|export| export.error.is_some())
        {
            failed_at.get_or_insert_with(Instant::now);
        }
        if failed_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(500)) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "slow destination was not reported"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(frames >= 24_000);
    drop(receiver);
    let status = engine
        .stop_network_export(ds, "stalled")
        .expect("stop failed export");
    assert!(status.error.is_some());
    engine.remove_device_set(ds).expect("remove source");
}

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

#[test]
fn virtual_device_exports_mtu_safe_ci16_udp() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver.set_read_timeout(Some(WAIT)).expect("timeout");
    let engine = engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("virtual set");
    let settings = NetworkExportSettings {
        transport: NetworkTransport::Udp,
        format: NetworkSampleFormat::Ci16Le,
        address: receiver.local_addr().expect("address").to_string(),
    };

    let started = engine
        .start_network_export(ds, "udp".to_owned(), 0, settings.clone())
        .expect("start UDP export");
    assert_eq!(started.node, "udp");
    assert_eq!(started.settings, settings);
    assert_eq!(started.samples, 0);
    let mut datagram = [0u8; 2_048];
    let received = receiver.recv(&mut datagram).expect("IQ datagram");
    assert!(
        received <= 1_400,
        "datagram exceeds the exporter MTU budget"
    );
    assert_eq!(received % settings.format.bytes_per_sample(), 0);
    assert!(
        datagram[..received]
            .as_chunks::<2>()
            .0
            .iter()
            .any(|word| *word != [0, 0]),
        "virtual IQ was encoded as silence"
    );
    let rate_error = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect_err("raw export pins its sample rate");
    assert!(matches!(rate_error, EngineError::NetworkExport(_)));
    assert!(rate_error.to_string().contains("locked"));
    assert!(
        engine.stop_network_export(ds, "another-node").is_err(),
        "one patch node must not stop another node's export"
    );

    let final_status = engine.stop_network_export(ds, "udp").expect("stop export");
    assert!(final_status.samples > 0);
    assert_eq!(
        final_status.bytes,
        final_status.samples * settings.format.bytes_per_sample() as u64
    );
    assert!(final_status.packets > 0);
    assert_eq!(final_status.error, None);
    engine.remove_device_set(ds).expect("remove set");
}

#[test]
fn virtual_device_exports_an_unframed_cf32_tcp_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let (first_tx, first_rx) = mpsc::channel::<[u8; 16]>();
    let reader = std::thread::spawn(move || {
        let deadline = Instant::now() + WAIT;
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "exporter never connected");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept exporter: {error}"),
            }
        };
        stream.set_nonblocking(false).expect("blocking stream");
        stream.set_read_timeout(Some(WAIT)).expect("timeout");
        let mut first = [0u8; 16];
        stream.read_exact(&mut first).expect("two complex samples");
        first_tx.send(first).expect("publish first samples");
        let mut drain = [0u8; 64 * 1_024];
        while matches!(stream.read(&mut drain), Ok(read) if read > 0) {}
    });
    let engine = engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("virtual set");
    engine
        .start_network_export(
            ds,
            "tcp".to_owned(),
            0,
            NetworkExportSettings {
                transport: NetworkTransport::Tcp,
                format: NetworkSampleFormat::Cf32Le,
                address: address.to_string(),
            },
        )
        .expect("start TCP export");

    let bytes = first_rx.recv_timeout(WAIT).expect("two complex samples");
    let components: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect();
    assert!(components.iter().all(|value| value.is_finite()));
    assert!(components.iter().any(|value| *value != 0.0));
    let status = engine.stop_network_export(ds, "tcp").expect("stop export");
    reader.join().expect("reader thread");
    assert_eq!(status.error, None);
    assert!(status.bytes >= 16);
    assert_eq!(status.bytes % 8, 0);
    assert_eq!(status.samples, status.bytes / 8);
    engine.remove_device_set(ds).expect("remove set");
}
